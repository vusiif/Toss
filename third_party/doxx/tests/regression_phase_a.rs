#[allow(clippy::duplicate_mod)]
#[path = "fixtures/mod.rs"]
mod fixtures;

use doxx::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use doxx::tempfile::{self, NamedTempFile};
use doxx::{
    App, AppOptions, DocumentElement, ViewMode,
    document::{ImageOptions, load_document},
    widgets::document::render_element_to_text,
};
use fixtures::SyntheticDocx;
use std::io::Write;

fn write_temp_docx(docx: &SyntheticDocx) -> NamedTempFile {
    let mut file = tempfile::Builder::new().suffix(".docx").tempfile().unwrap();
    let bytes = docx.build_bytes();
    file.write_all(&bytes).unwrap();
    file.flush().unwrap();
    file
}

// -------------------------------------------------------------------------------------------------
// Bug 1: Search Input Interception & Key Handling
// -------------------------------------------------------------------------------------------------
#[test]
fn test_search_input_not_intercepted_by_global_hotkeys() {
    let doc_xml = r#"
        <w:p><w:r><w:t>Annual revenue report for qoph division.</w:t></w:r></w:p>
    "#;
    let fixture = SyntheticDocx::new(doc_xml);
    let tmp = write_temp_docx(&fixture);

    let doc = load_document(tmp.path(), ImageOptions::default()).unwrap();
    let mut app = App::new(doc, &AppOptions::default());

    // Switch to search mode
    app.current_view = ViewMode::Search;
    assert_eq!(app.search_query, "");

    // Keys: 'r', 'e', 'v', 'e', 'n', 'u', 'e'
    // 'n' is a global hotkey (Next search result) in upstream doxx!
    for ch in "revenue".chars() {
        let key = KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
        let quit = app.handle_key_event(key);
        assert!(!quit, "Typing char '{ch}' must not quit the app");
    }
    assert_eq!(app.search_query, "revenue");
    assert!(
        !app.search_results.is_empty(),
        "Should find 'revenue' in document"
    );

    // Keys: 'n', 'p', 'q', 'o', 's', 'h', 'c'
    // In upstream doxx:
    //   'q' quits the app immediately (handle_action returns true)
    //   'n' jumps to next search result
    //   'p' jumps to prev search result
    //   'o' switches to Outline view
    //   's' switches to Search view
    //   'h' shows help
    //   'c' copies text
    app.search_query.clear();
    for ch in "npshoqc".chars() {
        let key = KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE);
        let quit = app.handle_key_event(key);
        assert!(!quit, "Typing char '{ch}' in search mode must not quit");
        assert!(
            matches!(app.current_view, ViewMode::Search),
            "Typing char '{ch}' must not switch view mode away from Search"
        );
    }
    assert_eq!(app.search_query, "npshoqc");

    // Backspace handling
    let bs = KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE);
    app.handle_key_event(bs);
    assert_eq!(app.search_query, "npshoq");

    // Ctrl+U clears search query
    let ctrl_u = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
    app.handle_key_event(ctrl_u);
    assert_eq!(app.search_query, "");

    // Esc returns to Document view
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let quit = app.handle_key_event(esc);
    assert!(!quit);
    assert!(matches!(app.current_view, ViewMode::Document));
}

// -------------------------------------------------------------------------------------------------
// Bug 2: Nested Tables & Structured Content
// -------------------------------------------------------------------------------------------------
#[test]
fn test_nested_table_and_sdt_extraction() {
    let doc_xml = r#"
        <w:tbl>
            <w:tr>
                <w:tc>
                    <w:p><w:r><w:t>NEST_OUTER</w:t></w:r></w:p>
                    <w:tbl>
                        <w:tr>
                            <w:tc>
                                <w:p><w:r><w:t>NEST_INNER: Nested value</w:t></w:r></w:p>
                            </w:tc>
                        </w:tr>
                    </w:tbl>
                    <w:sdt>
                        <w:sdtContent>
                            <w:p><w:r><w:t>STRUCTURED_TAG_DATA</w:t></w:r></w:p>
                        </w:sdtContent>
                    </w:sdt>
                    <w:p><w:r><w:t>NEST_AFTER</w:t></w:r></w:p>
                </w:tc>
            </w:tr>
        </w:tbl>
    "#;
    let fixture = SyntheticDocx::new(doc_xml);
    let tmp = write_temp_docx(&fixture);

    let doc = load_document(tmp.path(), ImageOptions::default()).unwrap();
    assert_eq!(doc.elements.len(), 1);

    let table_elem = &doc.elements[0];
    let rendered_lines = render_element_to_text(table_elem, 80);
    let full_rendered_text = rendered_lines.join("\n");

    // Assert that inner table content is NOT dropped
    assert!(
        full_rendered_text.contains("NEST_OUTER"),
        "Should contain outer table text"
    );
    assert!(
        full_rendered_text.contains("NEST_INNER: Nested value"),
        "Should preserve inner table text (fixed loss bug)"
    );
    assert!(
        full_rendered_text.contains("STRUCTURED_TAG_DATA"),
        "Should preserve structured tag data"
    );
    assert!(
        full_rendered_text.contains("NEST_AFTER"),
        "Should preserve trailing cell text"
    );
}

// -------------------------------------------------------------------------------------------------
// Bug 3: Tabs and Line Breaks Formatting
// -------------------------------------------------------------------------------------------------
#[test]
fn test_tab_and_break_formatting() {
    let doc_xml = r#"
        <w:p>
            <w:r>
                <w:t>BREAK_A</w:t>
                <w:tab/>
                <w:t>BREAK_B</w:t>
                <w:br/>
                <w:t>BREAK_C</w:t>
            </w:r>
        </w:p>
    "#;
    let fixture = SyntheticDocx::new(doc_xml);
    let tmp = write_temp_docx(&fixture);

    let doc = load_document(tmp.path(), ImageOptions::default()).unwrap();
    assert_eq!(doc.elements.len(), 1);

    if let DocumentElement::Paragraph { runs } = &doc.elements[0] {
        let combined: String = runs.iter().map(|r| r.text.as_str()).collect();
        assert!(
            combined.contains('\t'),
            "Extracted run text must contain tab character \\t"
        );
        assert!(
            combined.contains('\n'),
            "Extracted run text must contain newline \\n"
        );
    } else {
        panic!("Expected paragraph element");
    }

    let rendered = render_element_to_text(&doc.elements[0], 80);
    // Line 1 should contain BREAK_A and BREAK_B separated by spaces (from tab stop expansion)
    // Line 2 should contain BREAK_C
    assert!(
        rendered.len() >= 2,
        "Rendered text must have at least 2 lines due to <w:br/>"
    );
    assert!(
        rendered[0].contains("BREAK_A") && rendered[0].contains("BREAK_B"),
        "Line 1 should contain BREAK_A and BREAK_B"
    );
    // Ensure they didn't concatenate into "BREAK_ABREAK_B"
    assert!(
        !rendered[0].contains("BREAK_ABREAK_B"),
        "BREAK_A and BREAK_B must be separated by tab spaces, not concatenated"
    );
    assert!(
        rendered[1].contains("BREAK_C"),
        "Line 2 should contain BREAK_C"
    );
}

// -------------------------------------------------------------------------------------------------
// Bug 4: Table Cell Truncation & Multi-byte UTF-8 / Emojis
// -------------------------------------------------------------------------------------------------
#[test]
fn test_table_cell_wrapping_and_emoji_safety() {
    let doc_xml = r#"
        <w:tbl>
            <w:tr>
                <w:tc><w:p><w:r><w:t>Name</w:t></w:r></w:p></w:tc>
                <w:tc><w:p><w:r><w:t>Value</w:t></w:r></w:p></w:tc>
                <w:tc><w:p><w:r><w:t>Note</w:t></w:r></w:p></w:tc>
            </w:tr>
            <w:tr>
                <w:tc><w:p><w:r><w:t>中文</w:t></w:r></w:p></w:tc>
                <w:tc><w:p><w:r><w:t>42</w:t></w:r></w:p></w:tc>
                <w:tc><w:p/></w:tc>
            </w:tr>
            <w:tr>
                <w:tc><w:p><w:r><w:t>🙂</w:t></w:r></w:p></w:tc>
                <w:tc><w:p/></w:tc>
                <w:tc><w:p><w:r><w:t>TABLE_END_WITH_LONG_DESCRIPTION</w:t></w:r></w:p></w:tc>
            </w:tr>
        </w:tbl>
    "#;
    let fixture = SyntheticDocx::new(doc_xml);
    let tmp = write_temp_docx(&fixture);

    let doc = load_document(tmp.path(), ImageOptions::default()).unwrap();
    assert_eq!(doc.elements.len(), 1);

    // 1. Render with narrow width to force cell wrapping and test emoji boundary slicing
    let rendered_narrow = render_element_to_text(&doc.elements[0], 25);
    let narrow_text = rendered_narrow.join("\n");

    // Assert: No panics occurred and contents are present even when wrapped across sub-lines
    assert!(
        narrow_text.contains('中') && narrow_text.contains('文'),
        "Should contain Chinese characters (wrapped safely)"
    );
    assert!(
        narrow_text.contains("🙂"),
        "Should contain emoji 🙂 without slicing panic"
    );
    assert!(narrow_text.contains("42"), "Should contain '42'");
    assert!(
        narrow_text.contains("TABLE_END"),
        "Should contain wrapped long content"
    );
    // Assert column separators are preserved for empty cells and content rows
    assert!(
        rendered_narrow.iter().any(|line| line.contains('│')),
        "Column borders should be present in table rows"
    );

    // 2. Render with full width (80 columns) to verify standard single-line display
    let rendered_wide = render_element_to_text(&doc.elements[0], 80);
    let wide_text = rendered_wide.join("\n");
    assert!(
        wide_text.contains("中文"),
        "Should contain '中文' on a single line when wide enough"
    );
    assert!(wide_text.contains("🙂"));
    assert!(wide_text.contains("TABLE_END_WITH_LONG_DESCRIPTION"));
}

// -------------------------------------------------------------------------------------------------
// Edge Case: Minimal Package without Document .rels
// -------------------------------------------------------------------------------------------------
#[test]
fn test_minimal_package_without_doc_rels() {
    let doc_xml = r#"
        <w:p><w:r><w:t>Minimal document body without document relationships</w:t></w:r></w:p>
    "#;
    let fixture = SyntheticDocx::new(doc_xml).without_doc_rels();
    let tmp = write_temp_docx(&fixture);

    let doc = load_document(tmp.path(), ImageOptions::default());
    assert!(
        doc.is_ok(),
        "Minimal docx without document.xml.rels should load successfully: {:?}",
        doc.err()
    );
    let doc = doc.unwrap();
    assert_eq!(doc.elements.len(), 1);
    let rendered = render_element_to_text(&doc.elements[0], 80);
    assert_eq!(
        rendered,
        vec!["Minimal document body without document relationships"]
    );
}

// -------------------------------------------------------------------------------------------------
// Unpatched Contrast Assertions
// -------------------------------------------------------------------------------------------------
#[test]
fn test_contrast_unpatched_failure_mechanisms() {
    // Contrast 1: Upstream byte slicing on emoji would panic if sliced at byte boundary
    let emoji_str = "🙂"; // 4-byte UTF-8 sequence: [0xF0, 0x9F, 0x99, 0x82]
    assert_eq!(emoji_str.len(), 4);
    // In unpatched 0.1.4, render_table_row did &cell.content[..width]
    // If width was 1, 2, or 3, byte-slicing panics:
    let panic_result = std::panic::catch_unwind(|| {
        let _ = &emoji_str[..2];
    });
    assert!(
        panic_result.is_err(),
        "Unpatched byte-slice &str[..2] on multi-byte UTF-8 MUST panic"
    );

    // Contrast 2: Upstream loader did not handle RunChild::Tab
    // In upstream loader.rs:
    // match run_child {
    //     RunChild::Text(t) => run_text.push_str(&t.text),
    //     _ => {} // RunChild::Tab was ignored!
    // }
    // Now with RunChild::Tab(_), tab character '\t' is preserved.

    // Contrast 3: Upstream table.rs dropped nested tables
    // In upstream table.rs:
    // for content in &tc.children {
    //     match content {
    //         TableCellContent::Paragraph(p) => { ... }
    //         _ => {} // TableCellContent::Table was dropped!
    //     }
    // }
    // Now with recursive extraction, nested tables are extracted into cell.content.
}
