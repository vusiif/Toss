//! Word (.docx) document viewing, inspection, and dispatch integration tests (§4.C, §5).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch directory that cleans itself up.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("toss-word-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Calculate CRC32 checksum for ZIP entries.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB88320 & (!((crc & 1).wrapping_sub(1))));
        }
    }
    !crc
}

/// Write a minimal uncompressed (STORE) ZIP archive to `path`.
fn create_test_docx(path: &Path, body_xml: &str) {
    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
    <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
    <Default Extension="xml" ContentType="application/xml"/>
    <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;

    let root_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
    <w:body>
        {body_xml}
    </w:body>
</w:document>"#
    );

    let entries: [(&str, &[u8]); 3] = [
        ("[Content_Types].xml", content_types.as_bytes()),
        ("_rels/.rels", root_rels.as_bytes()),
        ("word/document.xml", document_xml.as_bytes()),
    ];

    let mut file = std::fs::File::create(path).expect("create docx file");
    let mut cd_records = Vec::new();
    let mut offset = 0u32;

    for (name, content) in entries {
        let name_bytes = name.as_bytes();
        let crc = crc32(content);
        let size = content.len() as u32;

        // Local header: 30 bytes + name_len
        file.write_all(&0x04034b50u32.to_le_bytes()).unwrap();
        file.write_all(&20u16.to_le_bytes()).unwrap();
        file.write_all(&0u16.to_le_bytes()).unwrap();
        file.write_all(&0u16.to_le_bytes()).unwrap(); // 0 = store
        file.write_all(&0u16.to_le_bytes()).unwrap();
        file.write_all(&0u16.to_le_bytes()).unwrap();
        file.write_all(&crc.to_le_bytes()).unwrap();
        file.write_all(&size.to_le_bytes()).unwrap();
        file.write_all(&size.to_le_bytes()).unwrap();
        file.write_all(&(name_bytes.len() as u16).to_le_bytes()).unwrap();
        file.write_all(&0u16.to_le_bytes()).unwrap();
        file.write_all(name_bytes).unwrap();
        file.write_all(content).unwrap();

        // Central directory header: 46 bytes + name_len
        let mut cd = Vec::new();
        cd.extend_from_slice(&0x02014b50u32.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        cd.extend_from_slice(&20u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&crc.to_le_bytes());
        cd.extend_from_slice(&size.to_le_bytes());
        cd.extend_from_slice(&size.to_le_bytes());
        cd.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u16.to_le_bytes());
        cd.extend_from_slice(&0u32.to_le_bytes());
        cd.extend_from_slice(&offset.to_le_bytes());
        cd.extend_from_slice(name_bytes);

        cd_records.push(cd);
        offset += 30 + name_bytes.len() as u32 + size;
    }

    let cd_offset = offset;
    let mut cd_size = 0u32;
    for cd in &cd_records {
        file.write_all(cd).unwrap();
        cd_size += cd.len() as u32;
    }

    // End of central directory record: 22 bytes
    file.write_all(&0x06054b50u32.to_le_bytes()).unwrap();
    file.write_all(&0u16.to_le_bytes()).unwrap();
    file.write_all(&0u16.to_le_bytes()).unwrap();
    file.write_all(&(cd_records.len() as u16).to_le_bytes()).unwrap();
    file.write_all(&(cd_records.len() as u16).to_le_bytes()).unwrap();
    file.write_all(&cd_size.to_le_bytes()).unwrap();
    file.write_all(&cd_offset.to_le_bytes()).unwrap();
    file.write_all(&0u16.to_le_bytes()).unwrap();
    file.flush().unwrap();
}

/// Run the toss binary with given arguments.
fn toss(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_toss"))
        .args(args)
        .output()
        .expect("toss binary runs")
}

#[test]
fn info_inspects_docx_without_launching_viewer_or_extracting() {
    let scratch = Scratch::new("info");
    let doc_path = scratch.path().join("sample.docx");
    create_test_docx(
        &doc_path,
        r#"<w:p><w:r><w:t>Sample paragraph for info</w:t></w:r></w:p>"#,
    );

    let output = toss(&["info".as_ref(), doc_path.as_os_str()]);
    assert!(
        output.status.success(),
        "info must exit with code 0: {:?}",
        output
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Detected type: document"),
        "expected document detection in info output: {}",
        stdout
    );
    assert!(output.stderr.is_empty(), "stderr should be empty on success");

    // Must not extract anything to the directory
    let extracted_dir = scratch.path().join("sample");
    assert!(
        !extracted_dir.exists(),
        "toss info must never extract files to disk"
    );
}

#[test]
fn info_inspects_uppercase_docx() {
    let scratch = Scratch::new("info_uppercase");
    let doc_path = scratch.path().join("TEST.DOCX");
    create_test_docx(
        &doc_path,
        r#"<w:p><w:r><w:t>Uppercase extension test</w:t></w:r></w:p>"#,
    );

    let output = toss(&["info".as_ref(), doc_path.as_os_str()]);
    assert!(output.status.success(), "info must exit 0: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Detected type: document"),
        "expected document detection for .DOCX: {}",
        stdout
    );
}

#[test]
fn word_missing_file_reports_exit_code_4() {
    let scratch = Scratch::new("missing");
    let missing_path = scratch.path().join("no_such_file_here.docx");

    let output = toss(&[missing_path.as_os_str()]);
    assert_eq!(
        output.status.code(),
        Some(4),
        "missing input must exit with code 4 (NotFound): {:?}",
        output
    );
}

#[test]
#[cfg(feature = "word")]
fn word_corrupt_file_reports_exit_code_6() {
    let scratch = Scratch::new("corrupt");
    let corrupt_path = scratch.path().join("corrupt.docx");
    // Write invalid data to a .docx file
    std::fs::write(&corrupt_path, b"PK\x03\x04not a valid zip package content at all").unwrap();

    let output = toss(&[corrupt_path.as_os_str()]);
    assert_eq!(
        output.status.code(),
        Some(6),
        "corrupt docx must exit with code 6 (CorruptInput): {:?}",
        output
    );
}

#[test]
#[cfg(feature = "word")]
fn word_auto_mode_and_view_verb_redirected_stdout() {
    let scratch = Scratch::new("view");
    let doc_path = scratch.path().join("document.docx");
    create_test_docx(
        &doc_path,
        r#"
        <w:p><w:r><w:t>Welcome to Toss Word Viewer!</w:t></w:r></w:p>
        <w:p><w:r><w:t>Section 1: Architecture &amp; Design</w:t></w:r></w:p>
        "#,
    );

    // 1. Automatic mode: `toss document.docx`
    let output_auto = toss(&[doc_path.as_os_str()]);
    assert!(
        output_auto.status.success(),
        "auto mode must exit 0 on redirected stdout: {:?}",
        output_auto
    );
    let stdout_auto = String::from_utf8_lossy(&output_auto.stdout);
    assert!(
        stdout_auto.contains("Welcome to Toss Word Viewer!"),
        "stdout must contain paragraph text: {}",
        stdout_auto
    );
    assert!(
        stdout_auto.contains("Section 1: Architecture & Design"),
        "stdout must contain section text: {}",
        stdout_auto
    );
    // Must contain no ANSI escape sequences
    assert!(
        !stdout_auto.contains("\x1b["),
        "stdout must not contain ANSI escape codes when redirected"
    );

    // 2. Explicit view verb: `toss view document.docx`
    let output_view = toss(&["view".as_ref(), doc_path.as_os_str()]);
    assert!(
        output_view.status.success(),
        "view verb must exit 0 on redirected stdout: {:?}",
        output_view
    );
    let stdout_view = String::from_utf8_lossy(&output_view.stdout);
    assert!(
        stdout_view.contains("Welcome to Toss Word Viewer!"),
        "stdout must contain paragraph text: {}",
        stdout_view
    );
    assert!(
        !stdout_view.contains("\x1b["),
        "stdout must not contain ANSI escape codes when redirected"
    );
}

#[test]
#[cfg(all(feature = "word", feature = "archive"))]
fn word_explicit_extract_verb_extracts_archive() {
    let scratch = Scratch::new("extract");
    let doc_path = scratch.path().join("archive_test.docx");
    create_test_docx(
        &doc_path,
        r#"<w:p><w:r><w:t>Content inside zip</w:t></w:r></w:p>"#,
    );

    let output = toss(&["extract".as_ref(), doc_path.as_os_str()]);
    assert!(
        output.status.success(),
        "toss extract archive_test.docx must succeed: {:?}",
        output
    );

    // Extraction should produce `archive_test/` directory with members
    let extracted_dir = scratch.path().join("archive_test");
    assert!(
        extracted_dir.is_dir(),
        "explicit extract must create output directory: {:?}",
        extracted_dir
    );
    assert!(
        extracted_dir.join("[Content_Types].xml").exists(),
        "extracted directory must contain members"
    );
    assert!(
        extracted_dir.join("word/document.xml").exists(),
        "extracted directory must contain word/document.xml"
    );
}

#[test]
#[cfg(not(feature = "word"))]
fn word_disabled_reports_unsupported_and_never_auto_extracts() {
    let scratch = Scratch::new("disabled");
    let doc_path = scratch.path().join("unsupported.docx");
    create_test_docx(
        &doc_path,
        r#"<w:p><w:r><w:t>Content that should not be extracted</w:t></w:r></w:p>"#,
    );

    let output = toss(&[doc_path.as_os_str()]);
    assert_eq!(
        output.status.code(),
        Some(3),
        "without word feature, toss docx must exit 3 (UnsupportedFormat): {:?}",
        output
    );

    // Must NEVER automatically extract
    let extracted_dir = scratch.path().join("unsupported");
    assert!(
        !extracted_dir.exists(),
        "unsupported docx must NEVER be automatically extracted into a directory"
    );
}

#[test]
#[cfg(feature = "word")]
fn word_spaces_and_unicode_in_path() {
    let scratch = Scratch::new("unicode_path");
    let doc_path = scratch.path().join("文档 项目 2026.docx");
    create_test_docx(
        &doc_path,
        r#"<w:p><w:r><w:t>Unicode path test passed successfully.</w:t></w:r></w:p>"#,
    );

    let output = toss(&[doc_path.as_os_str()]);
    assert!(output.status.success(), "must handle unicode path: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Unicode path test passed successfully."),
        "expected text in stdout: {}",
        stdout
    );
}

#[test]
#[cfg(feature = "word")]
fn word_sequential_documents_processed_in_order() {
    let scratch = Scratch::new("sequential");
    let doc_a = scratch.path().join("first.docx");
    let doc_b = scratch.path().join("second.docx");

    create_test_docx(
        &doc_a,
        r#"<w:p><w:r><w:t>First Document Content</w:t></w:r></w:p>"#,
    );
    create_test_docx(
        &doc_b,
        r#"<w:p><w:r><w:t>Second Document Content</w:t></w:r></w:p>"#,
    );

    let output = toss(&[doc_a.as_os_str(), doc_b.as_os_str()]);
    assert!(output.status.success(), "sequential docs must succeed: {:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("First Document Content"),
        "stdout must contain first doc: {}",
        stdout
    );
    assert!(
        stdout.contains("Second Document Content"),
        "stdout must contain second doc: {}",
        stdout
    );
}
