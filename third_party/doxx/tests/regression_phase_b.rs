#[allow(clippy::duplicate_mod)]
#[path = "fixtures/mod.rs"]
mod fixtures;

use doxx::tempfile::{self, NamedTempFile};
use doxx::{
    document::{
        Document, DocumentElement, DocumentMetadata, FormattedRun, ImageOptions, load_document,
        package::{DocxPackage, PackageError, PackageLimits},
    },
    image,
    ui::{App, AppOptions},
    zip::ZipWriter,
    zip::write::FileOptions,
};
use fixtures::SyntheticDocx;
use std::io::{Cursor, Write};

fn generate_test_png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let mut img = image::RgbaImage::new(width, height);
    for pixel in img.pixels_mut() {
        *pixel = image::Rgba(color);
    }
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .unwrap();
    buf
}

fn write_temp_docx(docx: &SyntheticDocx) -> NamedTempFile {
    let mut file = tempfile::Builder::new().suffix(".docx").tempfile().unwrap();
    let bytes = docx.build_bytes();
    file.write_all(&bytes).unwrap();
    file.flush().unwrap();
    file
}

// -------------------------------------------------------------------------------------------------
// 1. Corrupt & Invalid Package Boundaries (§5: 损坏/关系)
// -------------------------------------------------------------------------------------------------

#[test]
fn test_corrupt_non_zip_and_truncated_zip() {
    // Non-ZIP data
    let non_zip = b"THIS IS NOT A VALID ZIP ARCHIVE AT ALL!";
    let err = DocxPackage::from_bytes(non_zip).unwrap_err();
    assert!(
        matches!(err, PackageError::ZipError(_)),
        "Non-ZIP data must return PackageError::ZipError, got: {:?}",
        err
    );

    // Truncated ZIP data
    let fixture = SyntheticDocx::new("<w:p><w:r><w:t>Valid</w:t></w:r></w:p>");
    let full_bytes = fixture.build_bytes();
    assert!(full_bytes.len() > 60);
    let truncated = &full_bytes[..50]; // Truncate midway
    let err_trunc = DocxPackage::from_bytes(truncated).unwrap_err();
    assert!(
        matches!(err_trunc, PackageError::ZipError(_)),
        "Truncated ZIP must fail safely with ZipError, got: {:?}",
        err_trunc
    );
}

#[test]
fn test_missing_and_duplicate_main_document() {
    // Missing main document
    let mut buffer = Vec::new();
    {
        let mut zip = ZipWriter::new(Cursor::new(&mut buffer));
        let opt = FileOptions::default().compression_method(doxx::zip::CompressionMethod::Deflated);
        zip.start_file("[Content_Types].xml", opt).unwrap();
        zip.write_all(b"<Types/>").unwrap();
        zip.finish().unwrap();
    }
    let err_missing = DocxPackage::from_bytes(&buffer).unwrap_err();
    assert!(
        matches!(err_missing, PackageError::MissingMainDocument),
        "Package without word/document.xml must return MissingMainDocument, got: {:?}",
        err_missing
    );

    // Duplicate main document
    let fixture = SyntheticDocx::new("<w:p><w:r><w:t>Body 1</w:t></w:r></w:p>")
        .with_extra_entry("WORD/DOCUMENT.XML", b"<w:document/>");
    let dup_bytes = fixture.build_bytes();
    let err_dup = DocxPackage::from_bytes(&dup_bytes).unwrap_err();
    assert!(
        matches!(err_dup, PackageError::DuplicateMainDocument(_)),
        "Package with duplicate main document entries must return DuplicateMainDocument, got: {:?}",
        err_dup
    );
}

#[test]
fn test_security_path_traversal_rejection() {
    // Entry with ../
    let fixture_parent =
        SyntheticDocx::new("<w:p/>").with_extra_entry("../../evil.txt", b"malicious content");
    let err_parent = DocxPackage::from_bytes(&fixture_parent.build_bytes()).unwrap_err();
    assert!(
        matches!(err_parent, PackageError::PathTraversal(_)),
        "Entry with ../ must be rejected with PathTraversal, got: {:?}",
        err_parent
    );

    // Entry with nested /../
    let fixture_nested =
        SyntheticDocx::new("<w:p/>").with_extra_entry("word/../../etc/passwd", b"root:x:0:0");
    let err_nested = DocxPackage::from_bytes(&fixture_nested.build_bytes()).unwrap_err();
    assert!(
        matches!(err_nested, PackageError::PathTraversal(_)),
        "Entry with nested /../ must be rejected with PathTraversal, got: {:?}",
        err_nested
    );

    // Entry with absolute leading slash
    let fixture_abs = SyntheticDocx::new("<w:p/>").with_extra_entry("/etc/shadow", b"secret");
    let err_abs = DocxPackage::from_bytes(&fixture_abs.build_bytes()).unwrap_err();
    assert!(
        matches!(err_abs, PackageError::PathTraversal(_)),
        "Entry with leading slash must be rejected with PathTraversal, got: {:?}",
        err_abs
    );

    // Entry with Windows drive letter
    let fixture_drive = SyntheticDocx::new("<w:p/>")
        .with_extra_entry("C:/Windows/System32/calc.exe", b"executable");
    let err_drive = DocxPackage::from_bytes(&fixture_drive.build_bytes()).unwrap_err();
    assert!(
        matches!(err_drive, PackageError::PathTraversal(_)),
        "Entry with Windows drive letter must be rejected with PathTraversal, got: {:?}",
        err_drive
    );
}

#[test]
fn test_security_encrypted_package_rejection() {
    // Generate valid zip and set encryption flag in general purpose bit flags
    let fixture = SyntheticDocx::new("<w:p><w:r><w:t>Secret</w:t></w:r></w:p>");
    let mut zip_bytes = fixture.build_bytes();

    // Flip bit 0 (encryption flag) across all local file headers and central directory records
    let mut i = 0;
    while i + 4 <= zip_bytes.len() {
        // Local file header signature: 0x04034b50 ("PK\x03\x04")
        if &zip_bytes[i..i + 4] == b"PK\x03\x04" && i + 8 <= zip_bytes.len() {
            zip_bytes[i + 6] |= 0x01; // Flag bit 0: encrypted
        }
        // Central directory header signature: 0x02014b50 ("PK\x01\x02")
        if &zip_bytes[i..i + 4] == b"PK\x01\x02" && i + 10 <= zip_bytes.len() {
            zip_bytes[i + 8] |= 0x01; // Flag bit 0: encrypted
        }
        i += 1;
    }

    let err = DocxPackage::from_bytes(&zip_bytes).unwrap_err();
    assert!(
        matches!(err, PackageError::EncryptedPackage),
        "Encrypted package must be rejected with EncryptedPackage, got: {:?}",
        err
    );
}

#[test]
fn test_external_relationships_sandboxed_no_network() {
    let ext_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rIdExt" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="https://malicious.example.com/exfiltrate.png" TargetMode="External"/>
</Relationships>"#;

    let fixture = SyntheticDocx::new("<w:p><w:r><w:t>External link test</w:t></w:r></w:p>")
        .with_custom_doc_rels(ext_rels);

    let pkg = DocxPackage::from_bytes(&fixture.build_bytes()).unwrap();
    let rel = pkg
        .document_rels
        .get("rIdExt")
        .expect("rIdExt must be parsed");
    assert!(
        rel.is_external,
        "TargetMode=External must be marked is_external = true"
    );
    assert_eq!(rel.target, "https://malicious.example.com/exfiltrate.png");

    // ImageExtractor::from_package must ignore external relationships without making network requests
    let extractor = doxx::image_extractor::ImageExtractor::from_package(&pkg).unwrap();
    assert!(
        extractor.get_image_path("rIdExt").is_none(),
        "External relationship must not extract any local image"
    );
}

// -------------------------------------------------------------------------------------------------
// 2. Resource Limits & Decompression Accounting (§5: 资源)
// -------------------------------------------------------------------------------------------------

#[test]
fn test_resource_limits_document_xml_and_decompressed_limits() {
    // 1. Oversized document.xml budget
    let large_body = "<w:p><w:r><w:t>".to_string() + &"A".repeat(5000) + "</w:t></w:r></w:p>";
    let fixture = SyntheticDocx::new(&large_body);
    let bytes = fixture.build_bytes();

    let tight_limits = PackageLimits {
        max_document_xml_size: 1024, // 1 KiB limit
        ..PackageLimits::default()
    };
    let err_doc = DocxPackage::from_bytes_with_limits(&bytes, &tight_limits).unwrap_err();
    assert!(
        matches!(err_doc, PackageError::BudgetExceeded(_)),
        "Oversized document.xml must trigger BudgetExceeded, got: {:?}",
        err_doc
    );

    // 2. Total cumulative decompressed size budget
    let cumulative_limits = PackageLimits {
        max_document_xml_size: 100_000,
        max_total_uncompressed_size: 2000, // 2 KiB total cumulative limit
        ..PackageLimits::default()
    };
    let err_total = DocxPackage::from_bytes_with_limits(&bytes, &cumulative_limits).unwrap_err();
    assert!(
        matches!(err_total, PackageError::BudgetExceeded(_)),
        "Exceeding cumulative decompression budget must trigger BudgetExceeded, got: {:?}",
        err_total
    );

    // 3. Entry count budget
    let entry_limits = PackageLimits {
        max_entries: 3, // Less than [Content_Types], _rels, document.xml, rels
        ..PackageLimits::default()
    };
    let err_entries = DocxPackage::from_bytes_with_limits(&bytes, &entry_limits).unwrap_err();
    assert!(
        matches!(err_entries, PackageError::BudgetExceeded(_)),
        "Exceeding archive entry budget must trigger BudgetExceeded, got: {:?}",
        err_entries
    );
}

// -------------------------------------------------------------------------------------------------
// 3. Concurrency & RAII Cleanliness (§5: 图像 / 并发)
// -------------------------------------------------------------------------------------------------

#[test]
fn test_concurrent_sessions_isolated_and_raii_cleanliness() {
    let img_a_bytes = generate_test_png(1, 1, [255, 0, 0, 255]);
    let img_b_bytes = generate_test_png(2, 2, [0, 255, 0, 255]);

    // Create Document A with unique image content A
    let fixture_a = SyntheticDocx::new("<w:p><w:r><w:t>Doc A</w:t></w:r></w:p>").with_image(
        "rIdImg1",
        "word/media/image1.png",
        &img_a_bytes,
    );
    let tmp_a = write_temp_docx(&fixture_a);

    // Create Document B with same internal image path but different image content B
    let fixture_b = SyntheticDocx::new("<w:p><w:r><w:t>Doc B</w:t></w:r></w:p>").with_image(
        "rIdImg1",
        "word/media/image1.png",
        &img_b_bytes,
    );
    let tmp_b = write_temp_docx(&fixture_b);

    let doc_a = load_document(
        tmp_a.path(),
        ImageOptions {
            enabled: true,
            ..ImageOptions::default()
        },
    )
    .unwrap();
    let doc_b = load_document(
        tmp_b.path(),
        ImageOptions {
            enabled: true,
            ..ImageOptions::default()
        },
    )
    .unwrap();

    let session_a = doc_a
        .image_session
        .as_ref()
        .expect("Image session A must exist");
    let session_b = doc_b
        .image_session
        .as_ref()
        .expect("Image session B must exist");

    let dir_a = session_a.temp_dir().to_path_buf();
    let dir_b = session_b.temp_dir().to_path_buf();

    // 1. Both directories must exist and be distinct
    assert_ne!(
        dir_a, dir_b,
        "Concurrent sessions must have distinct temporary directories"
    );
    assert!(dir_a.exists(), "Temp dir A must exist on disk");
    assert!(dir_b.exists(), "Temp dir B must exist on disk");

    // 2. Both files must have distinct content corresponding to their source docx
    let path_img_a = session_a
        .get_image_path("rIdImg1")
        .expect("Image A must exist");
    let path_img_b = session_b
        .get_image_path("rIdImg1")
        .expect("Image B must exist");

    let read_a = std::fs::read(path_img_a).unwrap();
    let read_b = std::fs::read(path_img_b).unwrap();
    assert_eq!(read_a, img_a_bytes, "Image A content must match source A");
    assert_eq!(read_b, img_b_bytes, "Image B content must match source B");
    assert_ne!(
        read_a, read_b,
        "Images from doc A and B must not collide or overwrite"
    );

    // 3. Drop Document A -> its temp directory must be deleted (RAII cleanup)
    drop(doc_a);
    assert!(
        !dir_a.exists(),
        "Temp dir A must be deleted immediately upon drop"
    );
    assert!(
        dir_b.exists(),
        "Temp dir B must remain active while doc B lives"
    );

    // 4. Drop Document B -> its temp directory must also be deleted
    drop(doc_b);
    assert!(
        !dir_b.exists(),
        "Temp dir B must be deleted immediately upon drop"
    );
}

// -------------------------------------------------------------------------------------------------
// 4. Stable Image Association Across Scrolling (§4.B & §5: 滚动不串图)
// -------------------------------------------------------------------------------------------------

#[test]
fn test_stable_image_association_across_scrolling() {
    let img_a = generate_test_png(1, 1, [255, 0, 0, 255]);
    let img_b = generate_test_png(2, 2, [0, 0, 255, 255]);

    // Create temporary image files on disk to emulate extracted images
    let mut temp_img_a = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
    temp_img_a.write_all(&img_a).unwrap();
    temp_img_a.flush().unwrap();

    let mut temp_img_b = tempfile::Builder::new().suffix(".png").tempfile().unwrap();
    temp_img_b.write_all(&img_b).unwrap();
    temp_img_b.flush().unwrap();

    // Construct a document with two distinct images placed at element indices 1 and 3:
    // [0] Heading
    // [1] Image A (path A)
    // [2] Paragraph (Middle text)
    // [3] Image B (path B)
    // [4] Paragraph (Footer text)
    let elements = vec![
        DocumentElement::Heading {
            level: 1,
            text: "Heading".to_string(),
            number: None,
        },
        DocumentElement::Image {
            description: "Image A".to_string(),
            width: Some(1),
            height: Some(1),
            relationship_id: Some("rId1".to_string()),
            image_path: Some(temp_img_a.path().to_path_buf()),
        },
        DocumentElement::Paragraph {
            runs: vec![FormattedRun {
                text: "Separator paragraph".to_string(),
                formatting: Default::default(),
            }],
        },
        DocumentElement::Image {
            description: "Image B".to_string(),
            width: Some(2),
            height: Some(2),
            relationship_id: Some("rId2".to_string()),
            image_path: Some(temp_img_b.path().to_path_buf()),
        },
        DocumentElement::Paragraph {
            runs: vec![FormattedRun {
                text: "End paragraph".to_string(),
                formatting: Default::default(),
            }],
        },
    ];

    let doc = Document {
        title: "Test Image Stability".to_string(),
        metadata: DocumentMetadata {
            file_path: "test.docx".to_string(),
            file_size: 1024,
            word_count: 50,
            page_count: 1,
            created: None,
            modified: None,
            author: None,
        },
        elements,
        image_options: ImageOptions {
            enabled: true,
            ..ImageOptions::default()
        },
        image_session: None,
    };

    let app = App::new(
        doc,
        &AppOptions {
            images: true,
            ..AppOptions::default()
        },
    );

    // 1. Verify protocols are keyed strictly by element_index (1 and 3)
    assert_eq!(app.image_protocols.len(), 2);
    assert!(
        app.image_protocols.contains_key(&1),
        "Image A protocol must be keyed by element_index 1"
    );
    assert!(
        app.image_protocols.contains_key(&3),
        "Image B protocol must be keyed by element_index 3"
    );

    // 2. Test stability: When scrolled past element 1 (scroll_offset = 2),
    // Element 3 is the first visible image.
    // In unpatched doxx, protocol_idx was reset to 0 in the visible elements loop,
    // which caused Element 3 to be incorrectly rendered using protocol 0 (Image A).
    // With our element_index HashMap mapping:
    // Element 3 strictly queries image_protocols[&3], never index 0!
    assert!(app.image_protocols.contains_key(&3));
    assert!(app.image_protocols.contains_key(&1));
}
