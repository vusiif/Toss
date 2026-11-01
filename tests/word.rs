//! Word (.docx) document viewing, inspection, and dispatch integration tests (§4.C, §5).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch directory that cleans itself up.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("toss-word-test-{}-{name}", std::process::id()));
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
        file.write_all(&(name_bytes.len() as u16).to_le_bytes())
            .unwrap();
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
    file.write_all(&(cd_records.len() as u16).to_le_bytes())
        .unwrap();
    file.write_all(&(cd_records.len() as u16).to_le_bytes())
        .unwrap();
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
    assert!(
        output.stderr.is_empty(),
        "stderr should be empty on success"
    );

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
    std::fs::write(
        &corrupt_path,
        b"PK\x03\x04not a valid zip package content at all",
    )
    .unwrap();

    let output = toss(&[corrupt_path.as_os_str()]);
    assert_eq!(
        output.status.code(),
        Some(6),
        "corrupt docx must exit with code 6 (CorruptInput): {:?}",
        output
    );
}

#[test]
#[cfg(all(unix, feature = "word"))]
fn word_unreadable_file_reports_exit_code_5() {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid is a POSIX libc function with no preconditions and no side effects.
    let is_root = unsafe { geteuid() } == 0;
    if is_root {
        eprintln!(
            "SKIPPING word_unreadable_file_reports_exit_code_5: running as root bypasses mode 000"
        );
        return;
    }

    let scratch = Scratch::new("unreadable");
    let doc_path = scratch.path().join("unreadable.docx");
    create_test_docx(
        &doc_path,
        r#"<w:p><w:r><w:t>Secret Content</w:t></w:r></w:p>"#,
    );

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&doc_path, std::fs::Permissions::from_mode(0o000)).unwrap();

    // 1. Automatic mode
    let output_auto = toss(&[doc_path.as_os_str()]);
    assert_eq!(
        output_auto.status.code(),
        Some(5),
        "unreadable docx in automatic mode must exit with code 5 (PermissionDenied): {:?}",
        output_auto
    );

    // 2. Explicit view verb
    let output_view = toss(&["view".as_ref(), doc_path.as_os_str()]);
    assert_eq!(
        output_view.status.code(),
        Some(5),
        "unreadable docx in explicit view mode must exit with code 5 (PermissionDenied): {:?}",
        output_view
    );

    // Restore permissions so scratch cleanup succeeds
    let _ = std::fs::set_permissions(&doc_path, std::fs::Permissions::from_mode(0o644));
}

#[test]
#[cfg(all(unix, feature = "word"))]
fn test_validate_docx_file_preserves_package_error_type() {
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid is a POSIX libc function with no preconditions and no side effects.
    let is_root = unsafe { geteuid() } == 0;
    if is_root {
        eprintln!(
            "SKIPPING test_validate_docx_file_preserves_package_error_type: running as root bypasses mode 000"
        );
        return;
    }

    let scratch = Scratch::new("typed_err");
    let doc_path = scratch.path().join("typed_unreadable.docx");
    create_test_docx(
        &doc_path,
        r#"<w:p><w:r><w:t>Typed Error Content</w:t></w:r></w:p>"#,
    );

    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&doc_path, std::fs::Permissions::from_mode(0o000)).unwrap();

    let err = doxx::document::load_document(&doc_path, doxx::document::ImageOptions::default())
        .expect_err("load_document on mode 000 file must fail");

    // Precise regression assertion: PackageError must be discoverable via downcast
    let pkg_err = err.downcast_ref::<doxx::document::PackageError>().expect(
        "error must be discoverable as doxx::document::PackageError before top-level mapping",
    );

    match pkg_err {
        doxx::document::PackageError::IoError(io_err) => {
            assert_eq!(
                io_err.kind(),
                std::io::ErrorKind::PermissionDenied,
                "PackageError must contain std::io::ErrorKind::PermissionDenied"
            );
        }
        other => panic!("expected PackageError::IoError, got: {:?}", other),
    }

    let _ = std::fs::set_permissions(&doc_path, std::fs::Permissions::from_mode(0o644));
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
    assert!(
        output.status.success(),
        "must handle unicode path: {:?}",
        output
    );
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
    assert!(
        output.status.success(),
        "sequential docs must succeed: {:?}",
        output
    );
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

#[cfg(feature = "word")]
#[path = "../third_party/doxx/tests/regression_phase_a.rs"]
mod regression_phase_a;

#[cfg(feature = "word")]
#[path = "../third_party/doxx/tests/regression_phase_b.rs"]
mod regression_phase_b;

#[cfg(feature = "word")]
#[path = "../third_party/doxx/tests/regression_image_budget.rs"]
mod regression_image_budget;

#[cfg(feature = "word")]
#[allow(clippy::duplicate_mod)]
#[path = "../third_party/doxx/tests/fixtures/mod.rs"]
mod media_export_fixtures;

#[cfg(feature = "word")]
#[test]
fn redirected_word_keeps_image_markers_without_writing_temp_images() {
    let scratch = Scratch::new("metadata-only-export");
    let path = scratch.path().join("picture.docx");
    let mut png = Vec::new();
    doxx::image::RgbaImage::new(2, 2)
        .write_to(
            &mut std::io::Cursor::new(&mut png),
            doxx::image::ImageFormat::Png,
        )
        .unwrap();
    let fixture =
        media_export_fixtures::SyntheticDocx::new(media_export_fixtures::PICTURE_PARAGRAPH)
            .with_image("rIdPhoto", "word/media/photo.png", &png);
    std::fs::write(&path, fixture.build_bytes()).unwrap();
    let not_a_directory = scratch.path().join("temp-is-a-file");
    std::fs::write(&not_a_directory, b"not a directory").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_toss"))
        .arg(&path)
        .env("TEMP", &not_a_directory)
        .env("TMP", &not_a_directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("[Image]"));
    assert!(text.contains("After the image is a complete ordinary sentence."));
}

#[cfg(feature = "word")]
#[test]
fn pdf_preview_key_is_scoped_and_preserves_reading_state() {
    use doxx::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use doxx::{App, AppOptions, ViewMode};
    let scratch = Scratch::new("pdf-key-scope");
    let path = scratch.path().join("preview.docx");
    create_test_docx(&path, "<w:p><w:r><w:t>Preview</w:t></w:r></w:p>");
    let doc =
        doxx::document::load_document(&path, doxx::document::ImageOptions::default()).unwrap();
    let mut app = App::new(doc, &AppOptions::default());
    let key = KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE);
    app.handle_key_event(key);
    assert!(!app.pdf_open_requested);
    assert!(
        app.status_message
            .as_deref()
            .unwrap()
            .contains("unavailable in this build")
    );
    app.document.metadata.file_path = "legacy.doc".into();
    app.handle_key_event(key);
    assert!(!app.pdf_open_requested);
    let message = app.status_message.as_deref().unwrap();
    assert!(message.contains("DOCX only") && message.contains("renaming is not enough"));
    app.pdf_available = true;
    app.page_line_offset = 3;
    app.handle_key_event(key);
    assert!(std::mem::take(&mut app.pdf_open_requested));
    assert_eq!(app.page_line_offset, 3);
    assert_eq!(app.scroll_offset, 0);
    app.current_view = ViewMode::Search;
    app.handle_key_event(key);
    app.handle_key_event(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
    assert!(!app.pdf_open_requested);
    assert_eq!(app.search_query, "p");
    app.current_view = ViewMode::Document;
    app.show_help = true;
    app.handle_key_event(key);
    assert!(!app.pdf_open_requested);
}

#[cfg(feature = "word")]
#[test]
fn cached_screen_layout_matches_fresh_layout_across_paging_search_and_resize() {
    use doxx::ratatui::{Terminal, backend::TestBackend, layout::Rect};
    use doxx::{App, AppOptions, widgets::LayoutCache};

    let scratch = Scratch::new("layout-cache");
    let path = scratch.path().join("table.docx");
    let mut body = String::from(
        "<w:p><w:r><w:rPr><w:color w:val=\"FF0000\"/></w:rPr><w:t>NEEDLE 中文 🙂 colored paragraph</w:t></w:r></w:p><w:tbl>",
    );
    for row in 0..70 {
        body.push_str(&format!("<w:tr><w:tc><w:p><w:r><w:t>ROW-{row} 中文表格 NEEDLE</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Long cell 🙂 with enough words to wrap at narrow widths {row}</w:t></w:r></w:p></w:tc></w:tr>"));
    }
    body.push_str("</w:tbl><w:p><w:r><w:t>TABLE-END</w:t></w:r></w:p>");
    create_test_docx(&path, &body);
    let document =
        doxx::document::load_document(&path, doxx::document::ImageOptions::default()).unwrap();
    let table_index = document
        .elements
        .iter()
        .position(|element| matches!(element, doxx::DocumentElement::Table { .. }))
        .unwrap();
    let options = AppOptions {
        color: true,
        ..Default::default()
    };
    let mut warm = App::new(document.clone(), &options);
    let mut cold = App::new(document, &options);
    let mut warm_terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let mut cold_terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for step in 0..40 {
        for app in [&mut warm, &mut cold] {
            match step {
                1..=10 | 26..=30 => app.turn_page(true),
                11..=20 | 31..=35 => app.turn_page(false),
                21 => {
                    app.scroll_offset = 0;
                    app.page_line_offset = 0;
                    app.search_results = doxx::document::search_document(&app.document, "NEEDLE");
                }
                22 => app.search_results.clear(),
                23 => app.color_enabled = false,
                24 => app.color_enabled = true,
                _ => {}
            }
        }
        if step == 25 || step == 36 {
            let (width, height) = if step == 25 { (47, 17) } else { (100, 30) };
            for terminal in [&mut warm_terminal, &mut cold_terminal] {
                terminal.backend_mut().resize(width, height);
                terminal.resize(Rect::new(0, 0, width, height)).unwrap();
            }
        }
        cold.layout_cache = LayoutCache::new();
        warm_terminal
            .draw(|f| doxx::ui::render_document(f, f.area(), &mut warm))
            .unwrap();
        cold_terminal
            .draw(|f| doxx::ui::render_document(f, f.area(), &mut cold))
            .unwrap();
        assert_eq!(
            warm_terminal.backend().buffer(),
            cold_terminal.backend().buffer(),
            "frame {step}"
        );
        assert_eq!(
            (
                warm.scroll_offset,
                warm.page_line_offset,
                warm.next_page_position
            ),
            (
                cold.scroll_offset,
                cold.page_line_offset,
                cold.next_page_position
            ),
            "position {step}"
        );
        if step == 0 {
            assert!(
                warm.layout_cache
                    .get(table_index, warm.page_size.0)
                    .is_some()
            );
        }
    }
}

#[cfg(feature = "word")]
#[test]
fn layout_cache_bounds_entries_bytes_and_invalidates_style() {
    use doxx::{ratatui::text::Line, widgets::LayoutCache};
    use std::borrow::Cow;

    let mut cache = LayoutCache::new();
    cache.check_layout(80, true);
    cache.get_or_insert_with(0, 80, || vec![Line::from("retained")]);
    assert!(matches!(
        cache.get_or_insert_with(0, 80, || panic!("cache hit must borrow")),
        Cow::Borrowed(_)
    ));
    cache.check_layout(80, false);
    assert!(cache.get(0, 80).is_none());
    for index in 0..300 {
        cache.insert(index, 80, vec![Line::from("small line")]);
    }
    assert!(cache.get(0, 80).is_none());
    assert!(cache.get(299, 80).is_some());
    cache.check_width(40);
    assert!(cache.get(299, 80).is_none());
    let oversized =
        cache.get_or_insert_with(400, 40, || vec![Line::from("x".repeat(17 * 1024 * 1024))]);
    assert!(matches!(oversized, Cow::Owned(_)));
    drop(oversized);
    assert!(cache.get(400, 40).is_none());
    for index in 0..3 {
        cache.insert(index, 40, vec![Line::from("x".repeat(6 * 1024 * 1024))]);
    }
    assert!(cache.get(0, 40).is_none());
    assert!(cache.get(1, 40).is_some() && cache.get(2, 40).is_some());
    for _ in 0..300 {
        cache.insert(2, 40, vec![Line::from("replacement")]);
    }
    assert!(cache.get(1, 40).is_some());
}
