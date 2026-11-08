//! Legacy Word 97–2003 adapter. No Office automation or helper executable.

use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;

use doxx::document::*;

use crate::core::error::TossError;

const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_STREAMS: usize = 10_000;
const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;
const MAX_NESTING: usize = 32;

fn corrupt(path: &Path, message: impl Into<String>) -> TossError {
    TossError::CorruptInput {
        path: path.to_path_buf(),
        context: message.into(),
    }
}

pub(super) fn load(path: &Path, image_options: ImageOptions) -> Result<Document, TossError> {
    load_with_retention(path, image_options, true)
}

pub(super) fn load_for_text(
    path: &Path,
    image_options: ImageOptions,
) -> Result<Document, TossError> {
    load_with_retention(path, image_options, false)
}

fn load_with_retention(
    path: &Path,
    image_options: ImageOptions,
    retain_images: bool,
) -> Result<Document, TossError> {
    let file = File::open(path).map_err(|e| TossError::from_io(path, e))?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| TossError::from_io(path, e))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(corrupt(path, "legacy DOC file exceeds 64 MiB input limit"));
    }
    // Bound the declared streams before the parser reads them into memory.
    // This is an input/stream budget, not a bound on total process RSS.
    let compound = cfb::CompoundFile::open(Cursor::new(bytes.as_slice()))
        .map_err(|e| corrupt(path, format!("invalid legacy DOC container: {e}")))?;
    let mut total = 0u64;
    for (index, entry) in compound.walk().enumerate() {
        total = total.saturating_add(entry.len());
        if index >= MAX_STREAMS || total > MAX_FILE_BYTES {
            return Err(corrupt(path, "legacy DOC stream budget exceeded"));
        }
    }
    drop(compound);
    let source = rwml::Document::open(&bytes).map_err(|e| match e {
        rwml::Error::UnsupportedVersion(_) | rwml::Error::Encrypted { .. } => {
            TossError::other(format!("cannot read this legacy DOC: {e}"))
        }
        _ => corrupt(path, format!("invalid legacy DOC: {e}")),
    })?;
    let model = source.model();
    let mut converter = Converter::new(image_options.enabled, retain_images);
    let mut elements = Vec::new();
    for (index, block) in model.blocks.iter().enumerate() {
        for region in model
            .regions
            .iter()
            .filter(|r| r.block_start == index && r.block_end > index)
        {
            let label = match region.kind {
                rwml::SourceRegionKind::Main => None,
                rwml::SourceRegionKind::Footnote => Some("Footnotes"),
                rwml::SourceRegionKind::HeaderFooter => Some("Headers / footers"),
                rwml::SourceRegionKind::Annotation => Some("Comments"),
                rwml::SourceRegionKind::Endnote => Some("Endnotes"),
                rwml::SourceRegionKind::TextBox => Some("Text boxes"),
            };
            if let Some(label) = label {
                elements.push(note(&format!("[{label}]")));
            }
        }
        elements.extend(
            converter
                .blocks(std::slice::from_ref(block), 0)
                .map_err(|e| e.into_toss(path))?,
        );
    }
    let word_count = source.text().split_whitespace().count();
    Ok(Document {
        title: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        metadata: DocumentMetadata {
            file_path: path.to_string_lossy().into_owned(),
            file_size: bytes.len() as u64,
            word_count,
            page_count: 1,
            created: None,
            modified: None,
            author: None,
        },
        elements,
        image_options,
        image_session: converter.images.map(std::sync::Arc::new),
    })
}

fn note(text: &str) -> DocumentElement {
    DocumentElement::Paragraph {
        runs: vec![FormattedRun {
            text: text.into(),
            formatting: TextFormatting::default(),
        }],
    }
}

fn formatting(props: &rwml::CharProps) -> TextFormatting {
    TextFormatting {
        bold: props.bold,
        italic: props.italic,
        underline: props.underline,
        strikethrough: props.strike,
        font_size: props.size_half_pt.map(|n| n as f32 / 2.0),
        color: props
            .color
            .as_ref()
            .map(|c| format!("{:02X}{:02X}{:02X}", c.r, c.g, c.b)),
    }
}

struct Converter {
    text_remaining: usize,
    images_enabled: bool,
    retain_images: bool,
    images: Option<doxx::image_extractor::ImageExtractor>,
    image_count: usize,
    image_bytes: usize,
    limits: PackageLimits,
}

#[derive(Debug)]
enum ConversionError {
    Policy(String),
    Storage(doxx::document::DocumentError),
}

impl From<&str> for ConversionError {
    fn from(message: &str) -> Self {
        Self::Policy(message.into())
    }
}

impl ConversionError {
    fn into_toss(self, path: &Path) -> TossError {
        match self {
            Self::Policy(message) => corrupt(path, message),
            Self::Storage(error) => {
                TossError::other(format!("cannot store embedded Word image: {error}"))
            }
        }
    }
}

impl Converter {
    fn new(images_enabled: bool, retain_images: bool) -> Self {
        Self {
            text_remaining: MAX_TEXT_BYTES,
            images_enabled,
            retain_images,
            images: None,
            image_count: 0,
            image_bytes: 0,
            limits: PackageLimits::default(),
        }
    }

    fn charge(&mut self, text: &str) -> Result<(), ConversionError> {
        self.text_remaining = self
            .text_remaining
            .checked_sub(text.len())
            .ok_or("legacy DOC text budget exceeded")?;
        Ok(())
    }

    fn image(&mut self, image: &rwml::Image) -> Result<DocumentElement, ConversionError> {
        let ordinal = self.image_count;
        self.image_count += 1;
        let unavailable = |reason: &str| note(&format!("[DOC image {}: {reason}]", ordinal + 1));
        if !self.images_enabled {
            return Ok(unavailable("images disabled"));
        }
        if ordinal >= self.limits.max_image_count {
            return Ok(unavailable("image count limit exceeded"));
        }
        let Some(bytes) = image.bytes.as_deref() else {
            return Ok(unavailable(
                "unsupported picture, external reference, or unavailable data",
            ));
        };
        let extension = match image.mime.as_deref() {
            Some("image/png") => "png",
            Some("image/jpeg") => "jpg",
            _ => return Ok(unavailable("unsupported image format")),
        };
        if bytes.len() as u64 > self.limits.max_single_image_size {
            return Ok(unavailable("encoded image byte limit exceeded"));
        }
        if self.image_bytes.saturating_add(bytes.len()) as u64
            > self.limits.max_total_uncompressed_size
        {
            return Ok(unavailable("encoded image total limit exceeded"));
        }
        self.image_bytes += bytes.len();
        if !self.retain_images {
            return Ok(DocumentElement::Image {
                description: format!("DOC image {}", ordinal + 1),
                width: image.width_px,
                height: image.height_px,
                relationship_id: None,
                image_path: None,
            });
        }
        if self.images.is_none() {
            self.images = Some(
                doxx::image_extractor::ImageExtractor::new().map_err(ConversionError::Storage)?,
            );
        }
        let id = format!("doc-image-{ordinal}");
        let extractor = self.images.as_mut().ok_or("missing image session")?;
        let path = extractor
            .store_embedded_image(&id, bytes, extension)
            .map_err(ConversionError::Storage)?;
        Ok(DocumentElement::Image {
            description: format!("DOC image {}", ordinal + 1),
            width: image.width_px,
            height: image.height_px,
            relationship_id: Some(id),
            image_path: Some(path),
        })
    }

    fn paragraph(&self, p: &rwml::Paragraph, mut runs: Vec<FormattedRun>) -> DocumentElement {
        if let Some(level) = p.props.heading_level {
            DocumentElement::Heading {
                level,
                text: runs.iter().map(|r| r.text.as_str()).collect(),
                number: None,
            }
        } else if let Some(list) = &p.props.list {
            // Preserve Word's actual numbering (including a non-one start and
            // custom labels), rather than restarting each paragraph at "1.".
            if !list.label.is_empty() {
                runs.insert(
                    0,
                    FormattedRun {
                        text: format!("{}{} ", "  ".repeat(list.level as usize), list.label.trim()),
                        formatting: TextFormatting::default(),
                    },
                );
                DocumentElement::Paragraph { runs }
            } else {
                DocumentElement::List {
                    items: vec![ListItem {
                        runs,
                        level: list.level,
                    }],
                    ordered: list.ordered,
                }
            }
        } else {
            DocumentElement::Paragraph { runs }
        }
    }

    fn blocks(
        &mut self,
        blocks: &[rwml::Block],
        depth: usize,
    ) -> Result<Vec<DocumentElement>, ConversionError> {
        if depth > MAX_NESTING {
            return Err("legacy DOC table nesting limit exceeded".into());
        }
        let mut out = Vec::new();
        for block in blocks {
            match block {
                rwml::Block::Paragraph(p) => {
                    if p.props.page_break_before {
                        out.push(DocumentElement::PageBreak);
                    }
                    let mut runs = Vec::new();
                    let mut had_image = false;
                    for run in &p.runs {
                        self.charge(&run.text)?;
                        if !run.text.is_empty() {
                            runs.push(FormattedRun {
                                text: run.text.clone(),
                                formatting: formatting(&run.props),
                            });
                        }
                        if let Some(image) = &run.image {
                            had_image = true;
                            if !runs.is_empty() {
                                out.push(self.paragraph(p, std::mem::take(&mut runs)));
                            }
                            out.push(self.image(image)?);
                        }
                    }
                    if !runs.is_empty() || !had_image {
                        out.push(self.paragraph(p, runs));
                    }
                }
                rwml::Block::Table(table) => {
                    let mut rows = Vec::new();
                    let mut gallery = Vec::new();
                    for row in &table.rows {
                        let mut cells = Vec::new();
                        for cell in &row.cells {
                            let nested = self.blocks(&cell.blocks, depth + 1)?;
                            // Cell text includes an image marker; the original
                            // image remains selectable immediately after the table.
                            gallery.extend(
                                nested
                                    .iter()
                                    .filter(|e| matches!(e, DocumentElement::Image { .. }))
                                    .cloned(),
                            );
                            let (align, format) = cell
                                .blocks
                                .iter()
                                .find_map(|b| {
                                    if let rwml::Block::Paragraph(p) = b {
                                        Some((
                                            match p.props.align {
                                                rwml::Align::Left => TextAlignment::Left,
                                                rwml::Align::Center => TextAlignment::Center,
                                                rwml::Align::Right => TextAlignment::Right,
                                                rwml::Align::Justify => TextAlignment::Justify,
                                            },
                                            p.runs
                                                .first()
                                                .map(|r| formatting(&r.props))
                                                .unwrap_or_default(),
                                        ))
                                    } else {
                                        None
                                    }
                                })
                                .unwrap_or_default();
                            cells.push(TableCell {
                                content: plain_elements(&nested),
                                alignment: align,
                                formatting: format,
                                data_type: CellDataType::Text,
                            });
                        }
                        rows.push(cells);
                    }
                    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
                    let has_headers = table.header_rows > 0 && !rows.is_empty();
                    let headers = if has_headers {
                        rows.remove(0)
                    } else {
                        Vec::new()
                    };
                    out.push(DocumentElement::Table {
                        table: TableData {
                            metadata: TableMetadata {
                                column_count: columns,
                                row_count: rows.len(),
                                has_headers,
                                column_widths: vec![],
                                column_alignments: vec![],
                                title: None,
                            },
                            headers,
                            rows,
                        },
                    });
                    out.extend(gallery);
                }
                rwml::Block::Image(image) => out.push(self.image(image)?),
                rwml::Block::Chart(_) => out.push(note("[DOC chart: display unavailable]")),
                rwml::Block::PageBreak | rwml::Block::SectionBreak(_) => {
                    out.push(DocumentElement::PageBreak)
                }
            }
        }
        Ok(out)
    }
}

/// Preserve all nested cell text, rather than silently dropping inner tables.
fn plain_elements(elements: &[DocumentElement]) -> String {
    elements
        .iter()
        .map(|element| match element {
            DocumentElement::Paragraph { runs } => runs.iter().map(|r| r.text.as_str()).collect(),
            DocumentElement::Heading { text, .. } => text.clone(),
            DocumentElement::List { items, .. } => items
                .iter()
                .map(|item| {
                    item.runs
                        .iter()
                        .map(|r| r.text.as_str())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n"),
            DocumentElement::Table { table } => std::iter::once(&table.headers)
                .chain(table.rows.iter())
                .map(|row| {
                    row.iter()
                        .map(|c| c.content.as_str())
                        .collect::<Vec<_>>()
                        .join("\t")
                })
                .collect::<Vec<_>>()
                .join("\n"),
            DocumentElement::Image { description, .. } => format!("[{description}]"),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use doxx::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use doxx::{AppOptions, image, tempfile};

    mod fixtures {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/legacy_doc.rs"
        ));
    }

    fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn jpeg() -> Vec<u8> {
        let image = image::RgbImage::from_pixel(4, 4, image::Rgb([0, 0, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
            .unwrap();
        bytes
    }

    fn input(bytes: &[u8]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join(".sandbox-test/word-direct-20261027/tests");
        std::fs::create_dir_all(&dir).unwrap();
        let mut file = tempfile::Builder::new()
            .suffix(".doc")
            .tempfile_in(dir)
            .unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
        file
    }

    fn load_images(bytes: &[u8]) -> Document {
        let input = input(bytes);
        load(
            input.path(),
            ImageOptions {
                enabled: true,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn real_doc_png_jpeg_originals_keep_order_and_session_lifetime() {
        let png = png(4, 4, [255, 0, 0, 255]);
        let jpg = jpeg();
        let bytes = fixtures::images_doc(&[png.clone(), jpg.clone()]);
        // Preserve one small, reproducible manual-test fixture with the report.
        let evidence = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".sandbox-test/word-direct-20261027/doc-completion");
        std::fs::create_dir_all(&evidence).unwrap();
        std::fs::write(evidence.join("two-pictures.doc"), &bytes).unwrap();
        assert_eq!(
            rwml::Document::open(&bytes).unwrap().images().len(),
            2,
            "binary fixture must contain two parsed pictures"
        );
        let document = load_images(&bytes);
        let text = plain_elements(&document.elements);
        let expected = [
            "BEFORE",
            "[DOC image 1]",
            "AFTER_0",
            "[DOC image 2]",
            "AFTER_1",
        ];
        let mut previous = 0;
        for marker in expected {
            let position = text.find(marker).expect(marker);
            assert!(position >= previous, "picture/text order changed: {text}");
            previous = position;
        }
        let mut app = doxx::ui::App::new(
            document,
            &AppOptions {
                images: true,
                ..Default::default()
            },
        );
        assert_eq!(app.image_protocols.len(), 2);
        for _ in 0..2 {
            app.handle_key_event(KeyEvent::new(KeyCode::Char('>'), KeyModifiers::SHIFT));
        }
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let request = app.take_image_request().unwrap();
        assert_eq!(request.index, 1);
        assert_eq!(std::fs::read(&request.images[0]).unwrap(), png);
        assert_eq!(std::fs::read(&request.images[1]).unwrap(), jpg);
        let paths = request.images;
        drop(app);
        assert!(
            paths.iter().all(|p| !p.exists()),
            "temporary images must follow document lifetime"
        );
    }

    #[test]
    fn doc_images_do_not_collide_between_documents_and_can_be_disabled() {
        let bytes = fixtures::images_doc(&[png(2, 2, [255, 0, 0, 255])]);
        let first = load_images(&bytes);
        let second = load_images(&bytes);
        let a = first
            .image_session
            .as_ref()
            .unwrap()
            .get_image_path("doc-image-0")
            .unwrap()
            .clone();
        let b = second
            .image_session
            .as_ref()
            .unwrap()
            .get_image_path("doc-image-0")
            .unwrap()
            .clone();
        assert_ne!(a, b);
        drop(first);
        assert!(!a.exists() && b.exists());
        let file = input(&bytes);
        let disabled = load(file.path(), ImageOptions::default()).unwrap();
        assert!(disabled.image_session.is_none());
        assert!(plain_elements(&disabled.elements).contains("images disabled"));
    }

    #[test]
    fn text_loading_keeps_placeholders_without_an_image_session() {
        let bytes = fixtures::images_doc(&[png(2, 2, [1, 2, 3, 255]), jpeg()]);
        let file = input(&bytes);
        let options = ImageOptions {
            enabled: true,
            ..Default::default()
        };
        let interactive = load(file.path(), options.clone()).unwrap();
        let text = load_for_text(file.path(), options).unwrap();
        assert!(text.image_session.is_none());
        assert_eq!(
            plain_elements(&text.elements),
            plain_elements(&interactive.elements)
        );
        assert_eq!(
            text.elements
                .iter()
                .filter(|element| matches!(
                    element,
                    DocumentElement::Image {
                        image_path: None,
                        ..
                    }
                ))
                .count(),
            2
        );
    }

    #[test]
    fn unsupported_corrupt_and_oversized_pictures_preserve_doc_text() {
        for bytes in [
            b"unsupported WMF picture".to_vec(),
            b"\x89PNG\r\n\x1a\nbad".to_vec(),
            png(9000, 1, [255, 0, 0, 255]),
        ] {
            let doc = load_images(&fixtures::images_doc(&[bytes]));
            let text = plain_elements(&doc.elements);
            assert!(text.contains("BEFORE") && text.contains("AFTER_0"));
            let mut app = doxx::ui::App::new(
                doc,
                &AppOptions {
                    images: true,
                    ..Default::default()
                },
            );
            assert!(app.image_protocols.is_empty());
            app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            assert!(app.take_image_request().is_none());
        }
    }

    #[test]
    fn text_box_subdocument_is_present_once_without_side_table_duplication() {
        let doc = load_images(&fixtures::text_box_doc());
        let text = plain_elements(&doc.elements);
        assert_eq!(text.matches("MAIN_BODY").count(), 1);
        assert_eq!(text.matches("TEXT_BOX_ONLY").count(), 1);
        assert!(text.contains("[Text boxes]"), "{text}");
    }

    #[test]
    fn table_images_remain_selectable_and_numbering_color_are_preserved() {
        let image = rwml::Image {
            bytes: Some(png(2, 2, [255, 0, 0, 255])),
            mime: Some("image/png".into()),
            ..Default::default()
        };
        let paragraph = rwml::Paragraph {
            runs: vec![rwml::Run {
                text: "CELL_TEXT".into(),
                image: Some(image),
                ..Default::default()
            }],
            ..Default::default()
        };
        let table = rwml::Table {
            rows: vec![rwml::Row {
                cells: vec![rwml::Cell {
                    blocks: vec![rwml::Block::Paragraph(paragraph)],
                    ..Default::default()
                }],
            }],
            ..Default::default()
        };
        let mut converter = Converter::new(true, true);
        let elements = converter.blocks(&[rwml::Block::Table(table)], 0).unwrap();
        assert_eq!(
            elements
                .iter()
                .filter(|e| matches!(e, DocumentElement::Image { .. }))
                .count(),
            1
        );
        assert!(plain_elements(&elements).contains("CELL_TEXT"));
        let mut p = rwml::Paragraph::default();
        p.props.list = Some(rwml::ListInfo {
            level: 0,
            ordered: true,
            label: "7.".into(),
        });
        p.runs.push(rwml::Run {
            text: "NUMBERED".into(),
            props: rwml::CharProps {
                bold: true,
                color: Some(rwml::Color {
                    r: 0x12,
                    g: 0x34,
                    b: 0x56,
                }),
                ..Default::default()
            },
            ..Default::default()
        });
        let elements = converter.blocks(&[rwml::Block::Paragraph(p)], 0).unwrap();
        assert_eq!(plain_elements(&elements), "7. NUMBERED");
        let DocumentElement::Paragraph { runs } = &elements[0] else {
            panic!("numbered paragraph expected")
        };
        assert!(runs[1].formatting.bold);
        assert_eq!(runs[1].formatting.color.as_deref(), Some("123456"));
    }

    #[test]
    fn encoded_image_limits_reject_before_writing_and_do_not_charge_failures() {
        let picture = rwml::Image {
            bytes: Some(png(2, 2, [255, 0, 0, 255])),
            mime: Some("image/png".into()),
            ..Default::default()
        };
        let size = picture.bytes.as_ref().unwrap().len();
        let mut converter = Converter::new(true, true);
        converter.limits.max_single_image_size = (size - 1) as u64;
        let rejected = converter.image(&picture).unwrap();
        assert!(plain_elements(&[rejected]).contains("byte limit exceeded"));
        assert!(converter.images.is_none());
        assert_eq!(converter.image_bytes, 0);
        converter.limits.max_single_image_size = size as u64;
        converter.limits.max_total_uncompressed_size = size as u64;
        assert!(matches!(
            converter.image(&picture).unwrap(),
            DocumentElement::Image { .. }
        ));
        assert_eq!(converter.image_bytes, size);
        assert!(
            plain_elements(&[converter.image(&picture).unwrap()]).contains("total limit exceeded")
        );
        assert_eq!(converter.image_bytes, size);
        converter.limits.max_image_count = converter.image_count;
        assert!(
            plain_elements(&[converter.image(&picture).unwrap()]).contains("count limit exceeded")
        );
        assert_eq!(converter.images.as_ref().unwrap().list_images().len(), 1);
    }
}
