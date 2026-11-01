use anyhow::Result;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use crate::document::package::DocxPackage;

// Type aliases to simplify complex return types
type ImageList<'a> = Vec<(&'a String, &'a PathBuf)>;
type ExtractedImages = Vec<(String, PathBuf)>;

/// Manages extraction and lifecycle of images from DOCX files.
/// Backed by an isolated RAII temporary directory that is automatically
/// deleted when dropped.
#[derive(Debug)]
pub struct ImageExtractor {
    temp_dir: TempDir,
    extracted_images: HashMap<String, PathBuf>, // relationship_id -> temp_file_path
    dimensions: HashMap<String, (u32, u32)>,    // relationship_id -> (width, height)
}

#[derive(Debug, Clone)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Bmp,
    Tiff,
}

impl ImageFormat {
    pub fn from_filename(filename: &str) -> Option<Self> {
        let extension = Path::new(filename).extension()?.to_str()?.to_lowercase();

        match extension.as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "gif" => Some(Self::Gif),
            "bmp" => Some(Self::Bmp),
            "tiff" | "tif" => Some(Self::Tiff),
            _ => None,
        }
    }

    pub fn to_extension(&self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
        }
    }
}

impl ImageExtractor {
    /// Create a new image extractor with an isolated, unique temporary directory
    pub fn new() -> Result<Self> {
        let temp_dir = tempfile::Builder::new().prefix("toss_docx_").tempdir()?;

        Ok(Self {
            temp_dir,
            extracted_images: HashMap::new(),
            dimensions: HashMap::new(),
        })
    }

    /// Store a caller-budgeted embedded image without allowing its identifier
    /// or document-supplied name to become a filesystem path.
    pub fn store_embedded_image(
        &mut self,
        id: &str,
        bytes: &[u8],
        extension: &str,
    ) -> Result<PathBuf> {
        if !matches!(extension, "png" | "jpg") {
            anyhow::bail!("unsupported embedded image extension");
        }
        if self.extracted_images.contains_key(id) {
            anyhow::bail!("duplicate embedded image identifier");
        }
        let path = self.temp_dir.path().join(format!(
            "embedded_{}.{}",
            self.extracted_images.len(),
            extension
        ));
        let mut file = File::create_new(&path)?;
        file.write_all(bytes)?;
        self.extracted_images.insert(id.to_owned(), path.clone());
        Ok(path)
    }

    /// Extract images from an already parsed and validated DocxPackage.
    /// Maps images stably by relationship ID (rId) and stores pixel dimensions.
    pub fn from_package(pkg: &DocxPackage) -> Result<Self> {
        Self::extract_package(pkg, true, true)
    }

    /// Drawing dimensions come from Word XML. Defer header checks to budgeted decode.
    pub fn for_document(pkg: &DocxPackage) -> Result<Self> {
        Self::extract_package(pkg, false, false)
    }

    fn extract_package(
        pkg: &DocxPackage,
        include_orphans: bool,
        inspect_dimensions: bool,
    ) -> Result<Self> {
        let mut extractor = Self::new()?;
        let temp_path = extractor.temp_dir.path().to_path_buf();

        // 1. First extract by explicit relationship ID in document_rels
        let mut written_targets: HashMap<String, PathBuf> = HashMap::new();
        let mut written_dimensions = HashMap::new();

        for (rel_id, rel) in &pkg.document_rels {
            if rel.is_external {
                continue;
            }

            if let Some(image_bytes) = pkg.media_files.get(&rel.target) {
                let file_path = if let Some(existing_path) = written_targets.get(&rel.target) {
                    existing_path.clone()
                } else {
                    let filename = Path::new(&rel.target)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("image.png");

                    // Relationship IDs are untrusted XML identifiers, not file names.
                    let safe_name = format!("image_{}_{filename}", written_targets.len());
                    let target_path = temp_path.join(safe_name);

                    let mut file = File::create_new(&target_path)?;
                    file.write_all(image_bytes)?;

                    if inspect_dimensions {
                        if let Ok(dims) = image::ImageReader::new(std::io::Cursor::new(image_bytes))
                            .with_guessed_format()
                            .and_then(|reader| {
                                reader.into_dimensions().map_err(std::io::Error::other)
                            })
                        {
                            written_dimensions.insert(rel.target.clone(), dims);
                        }
                    }

                    written_targets.insert(rel.target.clone(), target_path.clone());
                    target_path
                };

                if let Some(&dims) = written_dimensions.get(&rel.target) {
                    extractor.dimensions.insert(rel_id.clone(), dims);
                }
                extractor.extracted_images.insert(rel_id.clone(), file_path);
            }
        }

        // 2. Also map any media files not referenced in document_rels by filename
        for (media_path, image_bytes) in &pkg.media_files {
            if include_orphans && !written_targets.contains_key(media_path) {
                let filename = Path::new(media_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("image.png");

                let target_path =
                    temp_path.join(format!("image_{}_{filename}", written_targets.len()));
                let mut file = File::create_new(&target_path)?;
                file.write_all(image_bytes)?;

                if let Ok(reader) =
                    image::ImageReader::new(std::io::Cursor::new(image_bytes)).with_guessed_format()
                {
                    if let Ok(dims) = reader.into_dimensions() {
                        extractor.dimensions.insert(filename.to_string(), dims);
                    }
                }

                written_targets.insert(media_path.clone(), target_path.clone());
                extractor
                    .extracted_images
                    .insert(filename.to_string(), target_path);
            }
        }

        Ok(extractor)
    }

    /// Extract all images from a DOCX file using DocxPackage
    pub fn extract_images_from_docx(&mut self, docx_path: &Path) -> Result<()> {
        let pkg = DocxPackage::open(docx_path)?;
        let extracted = Self::from_package(&pkg)?;

        self.extracted_images = extracted.extracted_images;
        self.dimensions = extracted.dimensions;
        self.temp_dir = extracted.temp_dir;

        Ok(())
    }

    /// Path to this session's isolated temporary directory
    pub fn temp_dir(&self) -> &Path {
        self.temp_dir.path()
    }

    /// Get image data by relationship ID
    pub fn get_image_data(&self, rel_id: &str) -> Result<Vec<u8>> {
        if let Some(path) = self.extracted_images.get(rel_id) {
            Ok(fs::read(path)?)
        } else {
            anyhow::bail!("Image not found: {}", rel_id)
        }
    }

    /// Get the path to an extracted image by relationship ID
    pub fn get_image_path(&self, rel_id: &str) -> Option<&PathBuf> {
        self.extracted_images.get(rel_id)
    }

    /// Get pixel dimensions for an image by relationship ID
    pub fn get_dimensions(&self, rel_id: &str) -> Option<(u32, u32)> {
        self.dimensions.get(rel_id).copied()
    }

    /// List all extracted images
    pub fn list_images(&self) -> ImageList<'_> {
        self.extracted_images.iter().collect()
    }

    /// Get all extracted images as a vector of (rel_id, path) pairs
    pub fn get_extracted_images(&self) -> ExtractedImages {
        self.extracted_images
            .iter()
            .map(|(rel_id, path)| (rel_id.clone(), path.clone()))
            .collect()
    }

    /// Get all extracted images sorted by key for consistent ordering
    pub fn get_extracted_images_sorted(&self) -> ExtractedImages {
        let mut images: ExtractedImages = self
            .extracted_images
            .iter()
            .map(|(rel_id, path)| (rel_id.clone(), path.clone()))
            .collect();

        images.sort_by(|a, b| a.0.cmp(&b.0));
        images
    }

    /// Clean up temporary files (normally handled automatically on Drop)
    pub fn cleanup(&mut self) -> Result<()> {
        let path = self.temp_dir.path().to_path_buf();
        if path.exists() {
            let _ = fs::remove_dir_all(&path);
        }
        Ok(())
    }

    /// Check if a file is an image based on its extension
    pub fn is_image_file(&self, filename: &str) -> bool {
        ImageFormat::from_filename(filename).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_format_detection() {
        assert!(matches!(
            ImageFormat::from_filename("image.png"),
            Some(ImageFormat::Png)
        ));
        assert!(matches!(
            ImageFormat::from_filename("photo.jpg"),
            Some(ImageFormat::Jpeg)
        ));
        assert!(matches!(
            ImageFormat::from_filename("photo.jpeg"),
            Some(ImageFormat::Jpeg)
        ));
        assert!(matches!(
            ImageFormat::from_filename("animation.gif"),
            Some(ImageFormat::Gif)
        ));
        assert!(matches!(
            ImageFormat::from_filename("bitmap.bmp"),
            Some(ImageFormat::Bmp)
        ));
        assert!(matches!(
            ImageFormat::from_filename("scan.tiff"),
            Some(ImageFormat::Tiff)
        ));
        assert!(ImageFormat::from_filename("document.txt").is_none());
    }

    #[test]
    fn test_image_extractor_creation() {
        let extractor = ImageExtractor::new().unwrap();
        assert!(extractor.temp_dir().exists());
        assert!(extractor.extracted_images.is_empty());
    }
}
