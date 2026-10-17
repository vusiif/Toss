//! Safe, bounded read-only package reader for DOCX archives.
//!
//! Enforces strict resource budgets, streaming decompression accounting,
//! path traversal validation, external relationship sandboxing, and batch part extraction.

use quick_xml::events::Event;
use quick_xml::reader::Reader as XmlReader;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use thiserror::Error;
use zip::ZipArchive;

#[derive(Error, Debug)]
pub enum PackageError {
    #[error("Invalid DOCX file: not a valid ZIP archive ({0})")]
    ZipError(String),
    #[error("Encrypted or password-protected DOCX packages are not supported")]
    EncryptedPackage,
    #[error("Security violation: path traversal or invalid entry name '{0}'")]
    PathTraversal(String),
    #[error("Corrupt DOCX package: missing main document part")]
    MissingMainDocument,
    #[error("Corrupt DOCX package: duplicate main document part '{0}'")]
    DuplicateMainDocument(String),
    #[error("Resource limit exceeded: {0}")]
    BudgetExceeded(String),
    #[error("IO error reading package: {0}")]
    IoError(#[from] std::io::Error),
}

impl From<zip::result::ZipError> for PackageError {
    fn from(err: zip::result::ZipError) -> Self {
        match err {
            zip::result::ZipError::UnsupportedArchive(msg)
                if msg.contains("Password")
                    || msg.contains("password")
                    || msg.contains("Encrypted")
                    || msg.contains("encrypted")
                    || msg.contains("encryption")
                    || msg.contains("AES") =>
            {
                PackageError::EncryptedPackage
            }
            other => PackageError::ZipError(other.to_string()),
        }
    }
}

/// Resource budgets for opening and extracting DOCX packages
#[derive(Debug, Clone)]
pub struct PackageLimits {
    /// Maximum number of files in the ZIP archive
    pub max_entries: usize,
    /// Maximum uncompressed bytes for the main document XML (word/document.xml)
    pub max_document_xml_size: u64,
    /// Maximum uncompressed bytes for metadata parts (styles, numbering, rels)
    pub max_metadata_xml_size: u64,
    /// Maximum uncompressed bytes for a single image file
    pub max_single_image_size: u64,
    /// Maximum total uncompressed payload across all extracted parts
    pub max_total_uncompressed_size: u64,
    /// Maximum number of embedded images to extract
    pub max_image_count: usize,
}

impl Default for PackageLimits {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_document_xml_size: 16 * 1024 * 1024,       // 16 MiB
            max_metadata_xml_size: 2 * 1024 * 1024,        // 2 MiB
            max_single_image_size: 20 * 1024 * 1024,       // 20 MiB
            max_total_uncompressed_size: 64 * 1024 * 1024, // 64 MiB
            max_image_count: 500,
        }
    }
}

/// A parsed relationship item from a .rels part
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocxRelationship {
    pub id: String,
    pub rel_type: String,
    pub target: String,
    pub is_external: bool,
}

/// Safe, in-memory representation of an extracted DOCX package
#[derive(Debug)]
pub struct DocxPackage {
    pub main_document_path: String,
    pub document_xml: Vec<u8>,
    pub document_rels: HashMap<String, DocxRelationship>,
    pub media_files: HashMap<String, Vec<u8>>,
    pub styles_xml: Option<Vec<u8>>,
    pub numbering_xml: Option<Vec<u8>>,
    pub has_doc_rels: bool,
}

impl DocxPackage {
    /// Open and validate a DOCX package from disk under strict resource limits
    pub fn open(path: &Path) -> Result<Self, PackageError> {
        Self::open_with_limits(path, &PackageLimits::default())
    }

    /// Open and validate a DOCX package from disk with custom resource limits
    pub fn open_with_limits(path: &Path, limits: &PackageLimits) -> Result<Self, PackageError> {
        let file = File::open(path)?;
        let file_len = file.metadata()?.len();

        // Check minimal zip file size
        if file_len < 22 {
            return Err(PackageError::ZipError("File too small to be a valid ZIP archive".to_string()));
        }

        let mut archive = ZipArchive::new(file)
            .map_err(|e| PackageError::ZipError(e.to_string()))?;

        Self::parse_archive(&mut archive, limits)
    }

    /// Parse directly from in-memory byte buffer
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, PackageError> {
        Self::from_bytes_with_limits(bytes, &PackageLimits::default())
    }

    /// Parse directly from in-memory byte buffer with custom resource limits
    pub fn from_bytes_with_limits(bytes: &[u8], limits: &PackageLimits) -> Result<Self, PackageError> {
        if bytes.len() < 22 {
            return Err(PackageError::ZipError("Data too small to be a valid ZIP archive".to_string()));
        }

        let cur = Cursor::new(bytes);
        let mut archive = ZipArchive::new(cur)
            .map_err(|e| PackageError::ZipError(e.to_string()))?;

        Self::parse_archive(&mut archive, limits)
    }

    fn parse_archive<R: Read + std::io::Seek>(
        archive: &mut ZipArchive<R>,
        limits: &PackageLimits,
    ) -> Result<Self, PackageError> {
        if archive.len() > limits.max_entries {
            return Err(PackageError::BudgetExceeded(format!(
                "Archive entry count {} exceeds limit of {}",
                archive.len(),
                limits.max_entries
            )));
        }

        // Pass 1: Catalog entries, validate paths, and detect duplicate/malicious targets
        let mut entry_names = Vec::with_capacity(archive.len());
        let mut seen_names = HashMap::new();
        let mut main_doc_count = 0;

        for i in 0..archive.len() {
            let raw_name = {
                let file = archive.by_index(i).map_err(PackageError::from)?;
                file.name().to_string()
            };

            let sanitized = sanitize_zip_path(&raw_name)?;

            let lower = sanitized.to_ascii_lowercase();
            if lower == "word/document.xml" {
                main_doc_count += 1;
                if main_doc_count > 1 {
                    return Err(PackageError::DuplicateMainDocument(sanitized));
                }
            }

            if seen_names.contains_key(&lower) {
                return Err(PackageError::ZipError(format!(
                    "Duplicate entry name in package: '{sanitized}'"
                )));
            }
            seen_names.insert(lower, i);
            entry_names.push(sanitized);
        }

        // Pass 2: Stream-extract required components with real-time budget accounting
        let mut total_decompressed: u64 = 0;

        // Discover main document path from _rels/.rels if available
        let main_document_path = if let Some(&rels_idx) = seen_names.get("_rels/.rels") {
            let mut file = archive.by_index(rels_idx)
                .map_err(|e| PackageError::ZipError(e.to_string()))?;
            let rels_bytes = read_bounded(
                &mut file,
                limits.max_metadata_xml_size,
                &mut total_decompressed,
                limits.max_total_uncompressed_size,
            )?;
            parse_root_rels_target(&rels_bytes).unwrap_or_else(|| "word/document.xml".to_string())
        } else {
            "word/document.xml".to_string()
        };

        // Extract main document.xml
        let doc_idx = seen_names
            .get(&main_document_path.to_ascii_lowercase())
            .copied()
            .ok_or(PackageError::MissingMainDocument)?;

        let document_xml = {
            let mut doc_file = archive.by_index(doc_idx)
                .map_err(|e| PackageError::ZipError(e.to_string()))?;
            read_bounded(
                &mut doc_file,
                limits.max_document_xml_size,
                &mut total_decompressed,
                limits.max_total_uncompressed_size,
            )?
        };

        // Extract document relationships if present
        let doc_rels_path = get_doc_rels_path(&main_document_path);
        let mut document_rels = HashMap::new();
        let mut has_doc_rels = false;

        if let Some(&doc_rels_idx) = seen_names.get(&doc_rels_path.to_ascii_lowercase()) {
            has_doc_rels = true;
            let doc_rels_bytes = {
                let mut rels_file = archive.by_index(doc_rels_idx)
                    .map_err(|e| PackageError::ZipError(e.to_string()))?;
                read_bounded(
                    &mut rels_file,
                    limits.max_metadata_xml_size,
                    &mut total_decompressed,
                    limits.max_total_uncompressed_size,
                )?
            };
            document_rels = parse_document_rels(&doc_rels_bytes, &main_document_path)?;
        }

        // Extract styles.xml if present
        let mut styles_xml = None;
        if let Some(&styles_idx) = seen_names.get("word/styles.xml") {
            let bytes = {
                let mut styles_file = archive.by_index(styles_idx)
                    .map_err(|e| PackageError::ZipError(e.to_string()))?;
                read_bounded(
                    &mut styles_file,
                    limits.max_metadata_xml_size,
                    &mut total_decompressed,
                    limits.max_total_uncompressed_size,
                )?
            };
            styles_xml = Some(bytes);
        }

        // Extract numbering.xml if present
        let mut numbering_xml = None;
        if let Some(&num_idx) = seen_names.get("word/numbering.xml") {
            let bytes = {
                let mut num_file = archive.by_index(num_idx)
                    .map_err(|e| PackageError::ZipError(e.to_string()))?;
                read_bounded(
                    &mut num_file,
                    limits.max_metadata_xml_size,
                    &mut total_decompressed,
                    limits.max_total_uncompressed_size,
                )?
            };
            numbering_xml = Some(bytes);
        }

        // Extract media files (batch scan, bounded count and sizes)
        let mut media_files = HashMap::new();
        let mut image_count = 0;

        for (lower_name, &idx) in &seen_names {
            if lower_name.starts_with("word/media/") {
                image_count += 1;
                if image_count > limits.max_image_count {
                    return Err(PackageError::BudgetExceeded(format!(
                        "Image count in package exceeds limit of {}",
                        limits.max_image_count
                    )));
                }

                let bytes = {
                    let mut media_file = archive.by_index(idx)
                        .map_err(|e| PackageError::ZipError(e.to_string()))?;
                    read_bounded(
                        &mut media_file,
                        limits.max_single_image_size,
                        &mut total_decompressed,
                        limits.max_total_uncompressed_size,
                    )?
                };
                media_files.insert(entry_names[idx].clone(), bytes);
            }
        }

        Ok(Self {
            main_document_path,
            document_xml,
            document_rels,
            media_files,
            styles_xml,
            numbering_xml,
            has_doc_rels,
        })
    }
}

/// Sanitize and validate archive entry path against path traversal and forbidden characters
pub fn sanitize_zip_path(raw: &str) -> Result<String, PackageError> {
    let normalized = raw.replace('\\', "/");

    // Prohibit drive letters (e.g. C:)
    if normalized.contains(':') {
        return Err(PackageError::PathTraversal(raw.to_string()));
    }

    // Prohibit absolute paths
    if normalized.starts_with('/') {
        return Err(PackageError::PathTraversal(raw.to_string()));
    }

    // Prohibit path traversal segments
    for segment in normalized.split('/') {
        if segment == ".." {
            return Err(PackageError::PathTraversal(raw.to_string()));
        }
    }

    let clean = normalized.trim_start_matches("./").to_string();
    if clean.is_empty() {
        return Err(PackageError::PathTraversal(raw.to_string()));
    }

    Ok(clean)
}

/// Read entry stream under part budget and cumulative total budget
fn read_bounded<R: Read>(
    reader: &mut R,
    max_part_size: u64,
    total_decompressed: &mut u64,
    max_total_size: u64,
) -> Result<Vec<u8>, PackageError> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut part_decompressed: u64 = 0;

    loop {
        let bytes_read = reader.read(&mut chunk)?;
        if bytes_read == 0 {
            break;
        }

        part_decompressed += bytes_read as u64;
        *total_decompressed += bytes_read as u64;

        if part_decompressed > max_part_size {
            return Err(PackageError::BudgetExceeded(format!(
                "Decompressed size of part ({part_decompressed} B) exceeds single part limit ({max_part_size} B)"
            )));
        }

        if *total_decompressed > max_total_size {
            return Err(PackageError::BudgetExceeded(format!(
                "Cumulative package decompressed size ({total_decompressed} B) exceeds total limit ({max_total_size} B)"
            )));
        }

        buffer.extend_from_slice(&chunk[..bytes_read]);
    }

    Ok(buffer)
}

/// Computes the relative document relationships path from main document path
fn get_doc_rels_path(main_doc_path: &str) -> String {
    let parent = Path::new(main_doc_path).parent();
    let filename = Path::new(main_doc_path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("document.xml");

    let rels_filename = format!("{filename}.rels");
    if let Some(p) = parent {
        if p.as_os_str().is_empty() {
            format!("_rels/{rels_filename}")
        } else {
            let p_str = p.to_string_lossy().replace('\\', "/");
            format!("{p_str}/_rels/{rels_filename}")
        }
    } else {
        format!("_rels/{rels_filename}")
    }
}

/// Parse root `_rels/.rels` to discover main document part target
fn parse_root_rels_target(xml_bytes: &[u8]) -> Option<String> {
    const OFFICE_DOC_REL: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";

    let mut reader = XmlReader::from_reader(xml_bytes);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e))
                if e.local_name().as_ref() == b"Relationship" =>
            {
                let mut is_doc = false;
                let mut target = None;
                for attr in e.attributes().flatten() {
                    if attr.key.local_name().as_ref() == b"Type"
                        && attr.value.as_ref() == OFFICE_DOC_REL.as_bytes()
                    {
                        is_doc = true;
                    } else if attr.key.local_name().as_ref() == b"Target" {
                        target = String::from_utf8(attr.value.into_owned()).ok();
                    }
                }
                if is_doc {
                    if let Some(t) = target {
                        let clean = t.trim_start_matches('/').to_string();
                        return Some(clean);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    None
}

/// Parse `word/_rels/document.xml.rels` into safe DocxRelationship records
fn parse_document_rels(
    xml_bytes: &[u8],
    doc_path: &str,
) -> Result<HashMap<String, DocxRelationship>, PackageError> {
    let mut map = HashMap::new();
    let mut reader = XmlReader::from_reader(xml_bytes);
    reader.config_mut().trim_text(true);

    let base_dir = Path::new(doc_path).parent();

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e))
                if e.local_name().as_ref() == b"Relationship" =>
            {
                let mut id = None;
                let mut rel_type = None;
                let mut target = None;
                let mut is_external = false;

                for attr in e.attributes().flatten() {
                    match attr.key.local_name().as_ref() {
                        b"Id" => id = String::from_utf8(attr.value.into_owned()).ok(),
                        b"Type" => rel_type = String::from_utf8(attr.value.into_owned()).ok(),
                        b"Target" => target = String::from_utf8(attr.value.into_owned()).ok(),
                        b"TargetMode" => {
                            if attr.value.as_ref() == b"External" {
                                is_external = true;
                            }
                        }
                        _ => {}
                    }
                }

                if let (Some(id), Some(rel_type), Some(raw_target)) = (id, rel_type, target) {
                    let resolved_target = if is_external {
                        // Keep external URL untouched, flagged as external
                        raw_target
                    } else {
                        // Resolve relative to document path directory
                        let normalized = raw_target.replace('\\', "/");
                        if let Some(b) = base_dir {
                            let b_str = b.to_string_lossy().replace('\\', "/");
                            let clean_normalized = normalized.trim_start_matches('/');
                            if !b_str.is_empty() && !clean_normalized.starts_with(&format!("{b_str}/")) {
                                let combined = b.join(&normalized).to_string_lossy().replace('\\', "/");
                                sanitize_zip_path(&combined)?
                            } else {
                                sanitize_zip_path(clean_normalized)?
                            }
                        } else {
                            sanitize_zip_path(normalized.trim_start_matches('/'))?
                        }
                    };

                    map.insert(
                        id.clone(),
                        DocxRelationship {
                            id,
                            rel_type,
                            target: resolved_target,
                            is_external,
                        },
                    );
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(PackageError::ZipError(format!("XML error parsing relationships: {e}"))),
            _ => {}
        }
        buf.clear();
    }

    Ok(map)
}

