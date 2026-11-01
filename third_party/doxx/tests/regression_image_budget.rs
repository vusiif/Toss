use std::io::{Cursor, Write};

use doxx::document::*;
use doxx::image;
use doxx::image_budget::{ImageBudgetPolicy, ImageBudgetTracker, ImageRejectionReason};
use doxx::ratatui::Terminal;
use doxx::ratatui::backend::TestBackend;
use doxx::ratatui::buffer::Buffer;
use doxx::ratatui::style::Color;
use doxx::tempfile::{self, NamedTempFile};
use doxx::ui::{App, AppOptions};
use doxx::widgets::DocumentWidget;

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

fn generate_test_jpeg(width: u32, height: u32, color: [u8; 3]) -> Vec<u8> {
    let mut img = image::RgbImage::new(width, height);
    for pixel in img.pixels_mut() {
        *pixel = image::Rgb(color);
    }
    let mut buf = Vec::new();
    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Jpeg)
        .unwrap();
    buf
}

fn create_temp_image_file(bytes: &[u8], suffix: &str) -> NamedTempFile {
    let mut file = tempfile::Builder::new().suffix(suffix).tempfile().unwrap();
    file.write_all(bytes).unwrap();
    file.flush().unwrap();
    file
}

fn make_synthetic_document(elements: Vec<DocumentElement>) -> Document {
    Document {
        title: "Test Image Budget Document".to_string(),
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
    }
}

fn buffer_to_string(buf: &Buffer) -> String {
    let mut text = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            text.push_str(buf[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

#[test]
fn test_original_image_selection_order_and_reading_state() {
    use doxx::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let first = create_temp_image_file(&generate_test_png(2, 2, [255, 0, 0, 255]), ".png");
    let second = create_temp_image_file(&generate_test_png(2, 2, [0, 0, 255, 255]), ".png");
    let mut elements = vec![DocumentElement::Paragraph {
        runs: vec![FormattedRun {
            text: "Before images".into(),
            formatting: TextFormatting::default(),
        }],
    }];
    for path in [first.path(), second.path()] {
        elements.push(DocumentElement::Image {
            description: "image".into(),
            width: Some(2),
            height: Some(2),
            relationship_id: None,
            image_path: Some(path.to_path_buf()),
        });
    }
    let mut app = App::new(
        make_synthetic_document(elements),
        &AppOptions {
            images: true,
            ..Default::default()
        },
    );
    let retained = app.budget_tracker.cumulative_bytes_used();
    let press = |code| KeyEvent::new(code, KeyModifiers::NONE);
    assert!(!app.handle_key_event(press(KeyCode::Char('>'))));
    assert_eq!(app.scroll_offset, 1);
    assert!(!app.handle_key_event(press(KeyCode::Char('>'))));
    assert_eq!(app.scroll_offset, 2);
    assert!(!app.handle_key_event(press(KeyCode::Enter)));
    let request = app.take_image_request().unwrap();
    assert_eq!(
        request.images,
        vec![first.path().to_path_buf(), second.path().to_path_buf()]
    );
    assert_eq!(request.index, 1);
    assert!(
        app.take_image_request().is_none(),
        "opening is consumed once"
    );
    assert_eq!(app.scroll_offset, 2);
    assert!(!app.handle_key_event(press(KeyCode::Char('<'))));
    assert_eq!(app.scroll_offset, 1);
    assert!(!app.handle_key_event(press(KeyCode::Enter)));
    assert_eq!(app.take_image_request().unwrap().index, 0);
    assert_eq!(app.budget_tracker.cumulative_bytes_used(), retained);
    app.current_view = doxx::ui::ViewMode::Search;
    app.handle_key_event(press(KeyCode::Char(']')));
    app.handle_key_event(press(KeyCode::Enter));
    assert_eq!(app.search_query, "]");
    assert!(
        app.take_image_request().is_none(),
        "search Enter must not open a window"
    );
}

#[test]
fn test_original_image_cannot_bypass_decode_rejection() {
    use doxx::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let file = create_temp_image_file(&generate_test_png(2, 2, [255, 0, 0, 255]), ".png");
    let document = make_synthetic_document(vec![DocumentElement::Image {
        description: "rejected".into(),
        width: Some(2),
        height: Some(2),
        relationship_id: None,
        image_path: Some(file.path().to_path_buf()),
    }]);
    let mut app = App::new_with_policy(
        document,
        &AppOptions {
            images: true,
            ..Default::default()
        },
        ImageBudgetPolicy::new(1, 1, 1, 4, 4),
    );
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.take_image_request().is_none());
    assert!(
        app.status_message
            .as_ref()
            .unwrap()
            .starts_with("Cannot open image:")
    );
}

fn render_reader_page(app: &mut App, terminal: &mut Terminal<TestBackend>) -> Buffer {
    terminal
        .draw(|frame| {
            let area = frame.area();
            doxx::ui::render_document(frame, area, app);
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

#[test]
fn test_screen_paging_preserves_long_paragraph_and_table_lines() {
    use doxx::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let text = (0..40)
        .map(|i| format!("ROW_{i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let paragraph = DocumentElement::Paragraph {
        runs: vec![FormattedRun {
            text: text.clone(),
            formatting: TextFormatting::default(),
        }],
    };
    let table = DocumentElement::Table {
        table: TableData {
            headers: vec![],
            rows: vec![vec![TableCell {
                content: text,
                alignment: TextAlignment::Left,
                formatting: TextFormatting::default(),
                data_type: CellDataType::Text,
            }]],
            metadata: TableMetadata {
                column_count: 1,
                row_count: 1,
                has_headers: false,
                column_widths: vec![12],
                column_alignments: vec![],
                title: None,
            },
        },
    };
    for element in [paragraph, table] {
        let mut app = App::new(
            make_synthetic_document(vec![element]),
            &AppOptions::default(),
        );
        let mut terminal = Terminal::new(TestBackend::new(30, 10)).unwrap();
        let first = render_reader_page(&mut app, &mut terminal);
        app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(
            app.page_line_offset > 0,
            "must page within a long source element"
        );
        let second = render_reader_page(&mut app, &mut terminal);
        assert_ne!(first, second);
        app.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.page_line_offset, 0);
        assert_eq!(render_reader_page(&mut app, &mut terminal), first);
        let mut seen = String::new();
        for _ in 0..20 {
            seen.push_str(&buffer_to_string(&render_reader_page(
                &mut app,
                &mut terminal,
            )));
            if app.next_page_position.is_none() {
                break;
            }
            app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        }
        for i in 0..40 {
            assert!(seen.contains(&format!("ROW_{i:02}")), "missing line {i}");
        }
        let end = (app.scroll_offset, app.page_line_offset);
        app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(
            (app.scroll_offset, app.page_line_offset),
            end,
            "end must not wrap or skip"
        );
    }
}

#[test]
fn test_screen_paging_images_and_shifted_angle_keys() {
    use doxx::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let red = create_temp_image_file(&generate_test_png(2, 2, [255, 0, 0, 255]), ".png");
    let blue = create_temp_image_file(&generate_test_png(2, 2, [0, 0, 255, 255]), ".png");
    let elements = [red.path(), blue.path()]
        .into_iter()
        .map(|path| DocumentElement::Image {
            description: "original".into(),
            width: Some(2),
            height: Some(2),
            relationship_id: None,
            image_path: Some(path.to_path_buf()),
        })
        .collect();
    let mut app = App::new(
        make_synthetic_document(elements),
        &AppOptions {
            images: true,
            ..Default::default()
        },
    );
    let mut terminal = Terminal::new(TestBackend::new(50, 20)).unwrap();
    let first = render_reader_page(&mut app, &mut terminal);
    assert!(
        buffer_has_red_halfblock(&first),
        "red image must actually render: {:?}",
        first
    );
    app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.scroll_offset, 1);
    let second = render_reader_page(&mut app, &mut terminal);
    assert!(
        buffer_has_blue_halfblock(&second),
        "blue image must actually render: {:?}",
        second
    );
    app.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.scroll_offset, 0);
    app.handle_key_event(KeyEvent::new(KeyCode::Char('>'), KeyModifiers::SHIFT));
    assert_eq!(app.scroll_offset, 1);
    app.handle_key_event(KeyEvent::new(KeyCode::Char('<'), KeyModifiers::SHIFT));
    assert_eq!(app.scroll_offset, 0);
    app.current_view = doxx::ui::ViewMode::Search;
    for key in ['<', '>'] {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(key), KeyModifiers::SHIFT));
    }
    assert_eq!(app.search_query, "<>");
    assert_eq!(app.scroll_offset, 0);
}

fn buffer_has_red_halfblock(buf: &Buffer) -> bool {
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            if cell.symbol() == "▀" {
                if let Color::Rgb(r, g, b) = cell.fg
                    && r > 100
                    && g == 0
                    && b == 0
                {
                    return true;
                }
                if let Color::Rgb(r, g, b) = cell.bg
                    && r > 100
                    && g == 0
                    && b == 0
                {
                    return true;
                }
            }
        }
    }
    false
}

fn buffer_has_blue_halfblock(buf: &Buffer) -> bool {
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            if cell.symbol() == "▀" {
                if let Color::Rgb(r, g, b) = cell.fg
                    && b > 100
                    && r == 0
                    && g == 0
                {
                    return true;
                }
                if let Color::Rgb(r, g, b) = cell.bg
                    && b > 100
                    && r == 0
                    && g == 0
                {
                    return true;
                }
            }
        }
    }
    false
}

fn render_doc_frame(terminal: &mut Terminal<TestBackend>, app: &mut App) {
    terminal
        .draw(|f| {
            let area = f.area();
            let mut doc_widget = DocumentWidget::new(&app.document.elements[..])
                .scroll_offset(app.scroll_offset)
                .color_enabled(app.color_enabled)
                .search_results(&app.search_results[..])
                .current_search_index(app.current_search_index)
                .image_placeholders(&app.image_placeholders);
            doc_widget.render(area, f, &mut app.image_protocols, &mut app.layout_cache);
        })
        .expect("draw frame to test terminal");
}

// -------------------------------------------------------------------------------------------------
// 1. Valid PNG & JPEG: Below, At, and Over Selected Budget Boundaries
// -------------------------------------------------------------------------------------------------

#[test]
fn test_image_budget_valid_png_and_jpeg_boundaries() {
    let png_bytes = generate_test_png(4, 4, [255, 0, 0, 255]); // 4x4 RGBA = 64 bytes
    let jpeg_bytes = generate_test_jpeg(4, 4, [0, 255, 0]); // 4x4 RGB = 48 bytes
    let png_file = create_temp_image_file(&png_bytes, ".png");
    let jpeg_file = create_temp_image_file(&jpeg_bytes, ".jpeg");

    // Case 1: Within budget -> Both PNG and JPEG succeed
    let policy = ImageBudgetPolicy::new(10, 10, 100, 1024, 2048);
    let mut tracker = ImageBudgetTracker::new(policy);

    let img1 = tracker.load_and_decode_element(0, png_file.path()).unwrap();
    assert_eq!(img1.width(), 4);
    assert_eq!(img1.height(), 4);
    assert!(tracker.cumulative_bytes_used() > 0);

    let bytes_after_1 = tracker.cumulative_bytes_used();
    let img2 = tracker
        .load_and_decode_element(1, jpeg_file.path())
        .unwrap();
    assert_eq!(img2.width(), 4);
    assert_eq!(img2.height(), 4);
    assert!(tracker.cumulative_bytes_used() > bytes_after_1);

    // Case 2: At boundary -> Exactly 4x4 dimensions allowed
    let boundary_policy = ImageBudgetPolicy::new(4, 4, 16, 64, 128);
    let mut boundary_tracker = ImageBudgetTracker::new(boundary_policy);
    assert!(
        boundary_tracker
            .load_and_decode_element(0, png_file.path())
            .is_ok()
    );

    // Case 3: Over dimension boundary -> Rejected before full allocation
    let tight_policy = ImageBudgetPolicy::new(3, 3, 16, 64, 128);
    let mut tight_tracker = ImageBudgetTracker::new(tight_policy);
    let err = tight_tracker
        .load_and_decode_element(0, png_file.path())
        .expect_err("4x4 image must exceed 3x3 dimension limit");

    match err {
        ImageRejectionReason::ExceedsDimensions {
            width,
            height,
            max_width,
            max_height,
        } => {
            assert_eq!(width, 4);
            assert_eq!(height, 4);
            assert_eq!(max_width, 3);
            assert_eq!(max_height, 3);
        }
        other => panic!("expected ExceedsDimensions, got: {:?}", other),
    }
    // Failed decode must NOT consume budget
    assert_eq!(tight_tracker.cumulative_bytes_used(), 0);
}

// -------------------------------------------------------------------------------------------------
// 2. Oversized Dimensions, Pixel Limits, and Byte Limits Rejected Before Full Allocation
// -------------------------------------------------------------------------------------------------

#[test]
fn test_image_budget_oversized_and_overflow_headers_rejected() {
    // Synthetic PNG header with extreme dimensions: 0xFFFF_FFFF x 0xFFFF_FFFF
    // 8 bytes magic + 4 length + "IHDR" + 4 width + 4 height + 5 bytes flags + 4 CRC
    let mut extreme_png = Vec::new();
    extreme_png.extend_from_slice(b"\x89PNG\r\n\x1a\n"); // Magic
    extreme_png.extend_from_slice(&13u32.to_be_bytes()); // Length of IHDR data
    extreme_png.extend_from_slice(b"IHDR");
    extreme_png.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes()); // Width
    extreme_png.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes()); // Height
    extreme_png.extend_from_slice(&[8, 6, 0, 0, 0]); // Bit depth 8, RGBA
    let crc = super::crc32(&extreme_png[12..29]);
    extreme_png.extend_from_slice(&crc.to_be_bytes());

    let extreme_file = create_temp_image_file(&extreme_png, ".png");

    let policy = ImageBudgetPolicy::default();
    let mut tracker = ImageBudgetTracker::new(policy);

    // Extreme header must be rejected immediately without huge memory allocation
    let res = tracker.load_and_decode_element(0, extreme_file.path());
    assert!(res.is_err(), "extreme PNG header must be rejected");
    assert_eq!(tracker.cumulative_bytes_used(), 0);

    // Pixel count limit check
    let valid_10x10_png = generate_test_png(10, 10, [0, 0, 0, 255]); // 100 pixels
    let valid_file = create_temp_image_file(&valid_10x10_png, ".png");

    let pixel_limited_policy = ImageBudgetPolicy::new(100, 100, 50, 10_000, 10_000);
    let mut pixel_tracker = ImageBudgetTracker::new(pixel_limited_policy);
    let err = pixel_tracker
        .load_and_decode_element(0, valid_file.path())
        .expect_err("100 pixels must exceed max_pixels 50");
    match err {
        ImageRejectionReason::ExceedsPixels { pixels, max_pixels } => {
            assert_eq!(pixels, 100);
            assert_eq!(max_pixels, 50);
        }
        other => panic!("expected ExceedsPixels, got: {:?}", other),
    }
    assert_eq!(pixel_tracker.cumulative_bytes_used(), 0);

    // Decoded byte limit check
    let byte_limited_policy = ImageBudgetPolicy::new(100, 100, 10_000, 200, 10_000); // 10x10 RGBA = 400 bytes > 200
    let mut byte_tracker = ImageBudgetTracker::new(byte_limited_policy);
    let err = byte_tracker
        .load_and_decode_element(0, valid_file.path())
        .expect_err("400 bytes must exceed max_bytes_per_image 200");
    match err {
        ImageRejectionReason::ExceedsBytes { bytes, max_bytes } => {
            assert_eq!(bytes, 400);
            assert_eq!(max_bytes, 200);
        }
        other => panic!("expected ExceedsBytes, got: {:?}", other),
    }
    assert_eq!(byte_tracker.cumulative_bytes_used(), 0);
}

// -------------------------------------------------------------------------------------------------
// 3. Cumulative Budget: Two Valid Images Exceed Budget; Second Gets Placeholder, Text Intact
// -------------------------------------------------------------------------------------------------

#[test]
fn test_image_budget_cumulative_budget_exhaustion_with_clean_degradation() {
    let img_a_bytes = generate_test_png(2, 2, [255, 0, 0, 255]); // 2x2 RGBA = 16 bytes
    let img_b_bytes = generate_test_png(2, 2, [0, 255, 0, 255]); // 2x2 RGBA = 16 bytes

    let file_a = create_temp_image_file(&img_a_bytes, ".png");
    let file_b = create_temp_image_file(&img_b_bytes, ".png");

    let elements = vec![
        DocumentElement::Heading {
            level: 1,
            text: "Heading Before Images".to_string(),
            number: None,
        },
        DocumentElement::Image {
            description: "First Image".to_string(),
            width: Some(2),
            height: Some(2),
            relationship_id: Some("rId1".to_string()),
            image_path: Some(file_a.path().to_path_buf()),
        },
        DocumentElement::Paragraph {
            runs: vec![FormattedRun {
                text: "Surrounding Text Between Images".to_string(),
                formatting: Default::default(),
            }],
        },
        DocumentElement::Image {
            description: "Second Image".to_string(),
            width: Some(2),
            height: Some(2),
            relationship_id: Some("rId2".to_string()),
            image_path: Some(file_b.path().to_path_buf()),
        },
        DocumentElement::Paragraph {
            runs: vec![FormattedRun {
                text: "Footer Text After Images".to_string(),
                formatting: Default::default(),
            }],
        },
    ];

    let doc = make_synthetic_document(elements);

    // Cumulative budget = 25 bytes. First image (16 B) fits, second (16 B) needs 32 B > 25 B.
    let cumulative_policy = ImageBudgetPolicy::new(100, 100, 10_000, 100, 25);
    let app = App::new_with_policy(
        doc,
        &AppOptions {
            images: true,
            ..AppOptions::default()
        },
        cumulative_policy,
    );

    // Element 1 (First Image) must be admitted into protocols
    assert!(
        app.image_protocols.contains_key(&1),
        "First image must be admitted"
    );
    // Element 3 (Second Image) must be rejected and recorded in placeholders
    assert!(
        !app.image_protocols.contains_key(&3),
        "Second image must be rejected due to cumulative budget"
    );
    let placeholder_reason = app
        .image_placeholders
        .get(&3)
        .expect("Placeholder reason must be recorded for element 3");
    assert!(
        placeholder_reason.contains("cumulative budget exceeded"),
        "Placeholder must state cumulative budget exceeded, got: {placeholder_reason}"
    );

    // Real headless rendering via DocumentWidget::render with ratatui TestBackend and Terminal frame
    let backend = TestBackend::new(80, 40);
    let mut terminal = Terminal::new(backend).expect("create test terminal");

    let mut app = app;
    render_doc_frame(&mut terminal, &mut app);

    let rendered_text = buffer_to_string(terminal.backend().buffer());

    // Assert real rendered surrounding text
    assert!(
        rendered_text.contains("Heading Before Images"),
        "Heading must be rendered by production widget path"
    );
    assert!(
        rendered_text.contains("Surrounding Text Between Images"),
        "Middle text must be rendered by production widget path"
    );
    assert!(
        rendered_text.contains("Footer Text After Images"),
        "Footer text must be rendered by production widget path"
    );

    // Assert actual rejection reason
    assert!(
        rendered_text.contains("[Image rejected: cumulative budget exceeded"),
        "Rejected image placeholder with actual reason must appear in rendered output, got:\n{rendered_text}"
    );

    // Assert absence of wrong --images hint
    assert!(
        !rendered_text.contains("use --images flag"),
        "Must not show wrong --images hint when image is rejected by budget, got:\n{rendered_text}"
    );
    assert!(
        !rendered_text.contains("--images"),
        "Must not contain --images in rendered text, got:\n{rendered_text}"
    );
}

// -------------------------------------------------------------------------------------------------
// 4. Corrupt/Unsupported Image Followed by Valid Image: Failure Releases Reservation
// -------------------------------------------------------------------------------------------------

#[test]
fn test_image_budget_corrupt_and_unsupported_release_reservation() {
    let corrupt_bytes = b"THIS IS NOT A VALID PNG OR JPEG DATA AT ALL";
    let valid_png_bytes = generate_test_png(2, 2, [0, 0, 255, 255]); // 16 bytes

    let corrupt_file = create_temp_image_file(corrupt_bytes, ".png");
    let valid_file = create_temp_image_file(&valid_png_bytes, ".png");

    let elements = vec![
        DocumentElement::Image {
            description: "Corrupt Image".to_string(),
            width: Some(2),
            height: Some(2),
            relationship_id: Some("rId1".to_string()),
            image_path: Some(corrupt_file.path().to_path_buf()),
        },
        DocumentElement::Image {
            description: "Valid Image".to_string(),
            width: Some(2),
            height: Some(2),
            relationship_id: Some("rId2".to_string()),
            image_path: Some(valid_file.path().to_path_buf()),
        },
    ];

    let doc = make_synthetic_document(elements);

    // Cumulative budget = 20 bytes.
    // If corrupt image leaked budget or failed the document, valid image wouldn't load.
    let policy = ImageBudgetPolicy::new(100, 100, 10_000, 100, 20);
    let app = App::new_with_policy(
        doc,
        &AppOptions {
            images: true,
            ..AppOptions::default()
        },
        policy,
    );

    // Element 0 (corrupt) must be recorded in placeholders
    assert!(!app.image_protocols.contains_key(&0));
    let reason_0 = app.image_placeholders.get(&0).unwrap();
    assert!(
        reason_0.contains("corrupt image data") || reason_0.contains("unsupported format"),
        "Reason must state corrupt/unsupported, got: {reason_0}"
    );

    // Element 1 (valid) must be admitted into protocols
    assert!(
        app.image_protocols.contains_key(&1),
        "Valid image after corrupt image must be admitted successfully"
    );
    assert_eq!(app.budget_tracker.cumulative_bytes_used(), 16);
}

// -------------------------------------------------------------------------------------------------
// 5. Stable Identity: At Least Two Valid Images + One Rejected; No Swap on Scroll
// -------------------------------------------------------------------------------------------------

#[test]
fn test_image_budget_stable_multi_image_identity_and_no_swap() {
    let img_red = generate_test_png(2, 2, [255, 0, 0, 255]);
    let img_blue = generate_test_png(2, 2, [0, 0, 255, 255]);
    let corrupt_bytes = b"CORRUPT IMAGE PLACEHOLDER";

    let file_corrupt = create_temp_image_file(corrupt_bytes, ".png");
    let file_red = create_temp_image_file(&img_red, ".png");
    let file_blue = create_temp_image_file(&img_blue, ".png");

    let elements = vec![
        DocumentElement::Heading {
            level: 1,
            text: "Heading".to_string(),
            number: None,
        },
        DocumentElement::Image {
            description: "Corrupt Image (elem 1)".to_string(),
            width: Some(2),
            height: Some(2),
            relationship_id: Some("rId1".to_string()),
            image_path: Some(file_corrupt.path().to_path_buf()),
        },
        DocumentElement::Paragraph {
            runs: vec![FormattedRun {
                text: "Separator 1".to_string(),
                formatting: Default::default(),
            }],
        },
        DocumentElement::Image {
            description: "Red Image (elem 3)".to_string(),
            width: Some(2),
            height: Some(2),
            relationship_id: Some("rId2".to_string()),
            image_path: Some(file_red.path().to_path_buf()),
        },
        DocumentElement::Paragraph {
            runs: vec![FormattedRun {
                text: "Separator 2".to_string(),
                formatting: Default::default(),
            }],
        },
        DocumentElement::Image {
            description: "Blue Image (elem 5)".to_string(),
            width: Some(2),
            height: Some(2),
            relationship_id: Some("rId3".to_string()),
            image_path: Some(file_blue.path().to_path_buf()),
        },
        DocumentElement::Paragraph {
            runs: vec![FormattedRun {
                text: "Footer".to_string(),
                formatting: Default::default(),
            }],
        },
    ];

    let doc = make_synthetic_document(elements.clone());

    let mut app = App::new(
        doc,
        &AppOptions {
            images: true,
            ..AppOptions::default()
        },
    );

    // 1. Initial keying & protocol identity verification:
    // elem 1 -> rejected and in placeholders
    // elem 3 -> admitted in protocols (red)
    // elem 5 -> admitted in protocols (blue)
    assert!(!app.image_protocols.contains_key(&1));
    assert!(app.image_placeholders.contains_key(&1));
    assert!(app.image_protocols.contains_key(&3));
    assert!(app.image_protocols.contains_key(&5));
    assert_eq!(app.budget_tracker.cumulative_bytes_used(), 32);

    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).expect("create test terminal");

    // 2. Viewport 1 (scroll_offset = 0):
    // Renders Heading, Rejected Image (elem 1) placeholder, Separator 1, and Red Image (elem 3).
    // Blue Image (elem 5) is below the 24-row viewport.
    app.scroll_offset = 0;
    render_doc_frame(&mut terminal, &mut app);

    let buf0 = terminal.backend().buffer();
    let text0 = buffer_to_string(buf0);
    assert!(text0.contains("Heading"), "Header must be in viewport 0");
    assert!(
        text0.contains("Corrupt Image (elem 1)"),
        "Earlier rejected image placeholder must be in viewport 0"
    );
    assert!(
        text0.contains("[Image rejected:"),
        "Rejection reason must be in viewport 0"
    );
    assert!(
        text0.contains("Red Image (elem 3)"),
        "Red image description must be in viewport 0"
    );
    assert!(
        !text0.contains("Blue Image (elem 5)"),
        "Blue image must not be in viewport 0"
    );
    assert!(
        buffer_has_red_halfblock(buf0),
        "Red halfblock color must be present in viewport 0"
    );
    assert!(
        !buffer_has_blue_halfblock(buf0),
        "Blue halfblock color must NOT be present in viewport 0"
    );

    // 3. Viewport 2 (scroll_offset = 4):
    // Scrolled past elem 0..3: renders Separator 2, Blue Image (elem 5), and Footer.
    // Red image and rejected image placeholder are scrolled out of view.
    app.scroll_offset = 4;
    render_doc_frame(&mut terminal, &mut app);

    let buf1 = terminal.backend().buffer();
    let text1 = buffer_to_string(buf1);
    assert!(
        !text1.contains("Heading"),
        "Header must be scrolled out of view"
    );
    assert!(
        !text1.contains("Corrupt Image (elem 1)"),
        "Rejected image must be scrolled out of view"
    );
    assert!(
        !text1.contains("Red Image (elem 3)"),
        "Red image must be scrolled out of view"
    );
    assert!(
        text1.contains("Separator 2"),
        "Separator 2 must be in viewport 4"
    );
    assert!(
        text1.contains("Blue Image (elem 5)"),
        "Blue image description must be in viewport 4"
    );
    assert!(
        text1.contains("Footer"),
        "Footer text must be in viewport 4"
    );
    assert!(
        buffer_has_blue_halfblock(buf1),
        "Blue halfblock color must be present in viewport 4"
    );
    assert!(
        !buffer_has_red_halfblock(buf1),
        "Red halfblock color must NOT be present in viewport 4"
    );

    // 4. Viewport 3 (scroll back to 0):
    // Re-render through real production path to assert stable protocol identity and no swap.
    app.scroll_offset = 0;
    render_doc_frame(&mut terminal, &mut app);

    let buf2 = terminal.backend().buffer();
    let text2 = buffer_to_string(buf2);
    assert!(
        text2.contains("Heading"),
        "Header must reappear upon scroll back"
    );
    assert!(
        text2.contains("Red Image (elem 3)"),
        "Red image must reappear upon scroll back"
    );
    assert!(
        !text2.contains("Blue Image (elem 5)"),
        "Blue image must not be in viewport upon scroll back"
    );
    assert!(
        buffer_has_red_halfblock(buf2),
        "Red halfblock color must reappear in viewport 0"
    );
    assert!(
        !buffer_has_blue_halfblock(buf2),
        "Blue halfblock color must NOT be in viewport 0"
    );

    // Protocol identities must remain strictly mapped to original element indices
    assert!(!app.image_protocols.contains_key(&1));
    assert!(app.image_placeholders.contains_key(&1));
    assert!(app.image_protocols.contains_key(&3));
    assert!(app.image_protocols.contains_key(&5));

    // 5. Cumulative budget must be completely unchanged across re-renders and scrolling
    assert_eq!(
        app.budget_tracker.cumulative_bytes_used(),
        32,
        "Cumulative budget must remain 32 bytes after re-rendering and scrolling"
    );

    // 6. Per-document budget independence
    let doc2 = make_synthetic_document(elements);
    let app2 = App::new(
        doc2,
        &AppOptions {
            images: true,
            ..AppOptions::default()
        },
    );
    assert_eq!(
        app2.budget_tracker.cumulative_bytes_used(),
        32,
        "Independent document must have its own cumulative budget"
    );
    assert_eq!(
        app.budget_tracker.cumulative_bytes_used(),
        32,
        "Original document budget must remain untouched by new document"
    );
}

// -------------------------------------------------------------------------------------------------
// 6. Re-Render / Scroll Idempotent: Does Not Re-Charge Budget; Independent Documents
// -------------------------------------------------------------------------------------------------

#[test]
fn test_image_budget_rerender_and_scroll_idempotent_no_recharge() {
    let img_bytes = generate_test_png(2, 2, [100, 100, 100, 255]); // 16 bytes
    let file = create_temp_image_file(&img_bytes, ".png");

    let elements = vec![
        DocumentElement::Paragraph {
            runs: vec![FormattedRun {
                text: "Paragraph".to_string(),
                formatting: Default::default(),
            }],
        },
        DocumentElement::Image {
            description: "Test Image".to_string(),
            width: Some(2),
            height: Some(2),
            relationship_id: Some("rId1".to_string()),
            image_path: Some(file.path().to_path_buf()),
        },
    ];

    let doc1 = make_synthetic_document(elements.clone());
    let mut app1 = App::new(
        doc1,
        &AppOptions {
            images: true,
            ..AppOptions::default()
        },
    );

    let initial_budget = app1.budget_tracker.cumulative_bytes_used();
    assert_eq!(initial_budget, 16);

    // Simulate scrolling and navigating
    app1.scroll_down();
    app1.scroll_down();
    app1.scroll_up();
    app1.scroll_up();

    // Budget must NOT change upon scrolling or re-evaluating
    assert_eq!(app1.budget_tracker.cumulative_bytes_used(), initial_budget);

    // Open a second document: must get independent budget tracker starting at its own count
    let doc2 = make_synthetic_document(elements);
    let app2 = App::new(
        doc2,
        &AppOptions {
            images: true,
            ..AppOptions::default()
        },
    );
    assert_eq!(app2.budget_tracker.cumulative_bytes_used(), 16);
}

// -------------------------------------------------------------------------------------------------
// 7. Clean Plain Text: Redirected Plain Text Does Not Decode or Emit ANSI
// -------------------------------------------------------------------------------------------------

#[test]
fn test_image_budget_clean_plain_text_and_no_eager_decode() {
    let img_bytes = generate_test_png(2, 2, [255, 0, 0, 255]);
    let file = create_temp_image_file(&img_bytes, ".png");

    let elem = DocumentElement::Image {
        description: "Test Image".to_string(),
        width: Some(2),
        height: Some(2),
        relationship_id: Some("rId1".to_string()),
        image_path: Some(file.path().to_path_buf()),
    };

    let text_lines = doxx::widgets::render_element_to_text(&elem, 80);
    assert_eq!(text_lines.len(), 1);
    assert_eq!(text_lines[0], "[Image]");

    // Verify ANSI escape characters (\x1b) are completely absent
    for line in &text_lines {
        assert!(
            !line.contains('\x1b'),
            "Plain text output must not contain ANSI escape codes: {line}"
        );
    }
}

// -------------------------------------------------------------------------------------------------
// 8. Bounded Synthetic Multi-Image Workload Measurement
// -------------------------------------------------------------------------------------------------

/// Small bounded synthetic multi-image workload measurement.
///
/// Parameters:
/// - Image count: 8 images
/// - Dimensions: 64x64 pixels per image (PNG RGBA, 4 bytes/pixel)
/// - Logical decoded bytes per image: 64 * 64 * 4 = 16,384 bytes (16.0 KiB)
/// - Policy: max_width=128, max_height=128, max_pixels=16384, max_bytes_per_image=32768, max_cumulative_bytes=131072
/// - Retained decoded bytes: 8 * 16,384 = 131,072 bytes (128.0 KiB)
/// - Monotonic timings: measured over 5 samples using `std::time::Instant`
///
/// NOTE: The logical source-decoded-byte budget tracks and bounds decoded uncompressed image data
/// admitted into memory and is NOT a strict total process RSS bound. No broad performance claims
/// are made.
#[test]
fn test_image_budget_bounded_multi_image_workload_measurement() {
    let img_count = 8usize;
    let width = 64u32;
    let height = 64u32;
    let bytes_per_img = (width * height * 4) as usize; // 16,384 bytes
    let total_expected_bytes = (img_count * bytes_per_img) as u64; // 131,072 bytes (128.0 KiB)

    // Pre-generate 8 distinct synthetic PNG files
    let mut files = Vec::with_capacity(img_count);
    for i in 0..img_count {
        let color = [((i * 30) % 256) as u8, ((i * 50) % 256) as u8, 128, 255];
        let bytes = generate_test_png(width, height, color);
        files.push(create_temp_image_file(&bytes, ".png"));
    }

    let policy = ImageBudgetPolicy::new(
        128,
        128,
        16_384,
        32_768,
        total_expected_bytes, // exactly 131,072 bytes to admit all 8 images
    );

    // Run 5 samples with monotonic timings
    let sample_count = 5usize;
    let mut sample_durations = Vec::with_capacity(sample_count);

    for sample_idx in 0..sample_count {
        let mut elements = Vec::with_capacity(img_count);
        for (i, file) in files.iter().enumerate() {
            elements.push(DocumentElement::Image {
                description: format!("Workload Image {i}"),
                width: Some(width),
                height: Some(height),
                relationship_id: Some(format!("rId{i}")),
                image_path: Some(file.path().to_path_buf()),
            });
        }
        let doc = make_synthetic_document(elements);

        let start = std::time::Instant::now();
        let app = App::new_with_policy(
            doc,
            &AppOptions {
                images: true,
                ..AppOptions::default()
            },
            policy,
        );
        let elapsed = start.elapsed();
        sample_durations.push(elapsed);

        // Verify logical decoded-byte accounting and protocol retention on each sample
        assert_eq!(
            app.budget_tracker.cumulative_bytes_used(),
            total_expected_bytes,
            "Sample {sample_idx}: cumulative decoded bytes must equal exactly {total_expected_bytes}"
        );
        assert_eq!(
            app.image_protocols.len(),
            img_count,
            "Sample {sample_idx}: all {img_count} images must be admitted"
        );
        assert_eq!(
            app.image_placeholders.len(),
            0,
            "Sample {sample_idx}: zero images should be rejected"
        );
    }

    let min_dur = *sample_durations.iter().min().unwrap();
    let max_dur = *sample_durations.iter().max().unwrap();
    let sum_dur: std::time::Duration = sample_durations.iter().copied().sum();
    let avg_dur = sum_dur / (sample_count as u32);
    let spread = max_dur.saturating_sub(min_dur);

    eprintln!("=== DOCX Image Budget: Synthetic Multi-Image Workload Measurement ===");
    eprintln!("  Image count:              {img_count}");
    eprintln!(
        "  Image dimensions:         {width} x {height} px (PNG RGBA, {bytes_per_img} bytes decoded/image)"
    );
    eprintln!(
        "  Policy limits:            max_width=128, max_height=128, max_pixels=16384, max_bytes_per_img=32768, max_cumulative=131072"
    );
    eprintln!("  Admitted images:          {img_count} / {img_count}");
    eprintln!("  Retained decoded bytes:   {total_expected_bytes} bytes (128.0 KiB)");
    eprintln!("  Sample count:             {sample_count}");
    eprintln!("  Timing min:               {min_dur:?}");
    eprintln!("  Timing max:               {max_dur:?}");
    eprintln!("  Timing avg:               {avg_dur:?}");
    eprintln!("  Timing spread (max-min):  {spread:?}");
    eprintln!(
        "  NOTE: The logical source-decoded-byte budget tracks and bounds decoded image pixels/bytes"
    );
    eprintln!(
        "        admitted into memory and is NOT a strict total process RSS bound; no broad performance claims."
    );

    // Monotonic timings assertions
    for dur in &sample_durations {
        assert!(dur.as_nanos() > 0, "Duration must be positive and non-zero");
    }
    assert!(min_dur <= avg_dur && avg_dur <= max_dur);

    // Also verify multi-image partial degradation under tight cumulative budget
    let tight_policy = ImageBudgetPolicy::new(128, 128, 16_384, 32_768, 65_536);
    let mut elements_tight = Vec::with_capacity(img_count);
    for (i, file) in files.iter().enumerate() {
        elements_tight.push(DocumentElement::Image {
            description: format!("Workload Image {i}"),
            width: Some(width),
            height: Some(height),
            relationship_id: Some(format!("rId{i}")),
            image_path: Some(file.path().to_path_buf()),
        });
    }
    let doc_tight = make_synthetic_document(elements_tight);
    let app_tight = App::new_with_policy(
        doc_tight,
        &AppOptions {
            images: true,
            ..AppOptions::default()
        },
        tight_policy,
    );
    assert_eq!(
        app_tight.budget_tracker.cumulative_bytes_used(),
        65_536,
        "Tight cumulative budget must cap retained decoded bytes at exactly 65,536 (64 KiB)"
    );
    assert_eq!(
        app_tight.image_protocols.len(),
        4,
        "Exactly 4 images should be admitted under 64 KiB limit"
    );
    assert_eq!(
        app_tight.image_placeholders.len(),
        4,
        "Remaining 4 images should degrade to placeholders"
    );
}

fn lazy_image(path: &std::path::Path, name: &str) -> DocumentElement {
    DocumentElement::Image {
        description: name.into(),
        width: Some(2),
        height: Some(2),
        relationship_id: None,
        image_path: Some(path.to_path_buf()),
    }
}

fn lazy_frame(terminal: &mut Terminal<TestBackend>, app: &mut App) {
    terminal
        .draw(|frame| doxx::ui::render_document(frame, frame.area(), app))
        .unwrap();
}

#[test]
fn lazy_images_do_not_decode_beyond_the_first_screen() {
    let image = create_temp_image_file(&generate_test_png(2, 2, [255, 0, 0, 255]), ".png");
    let mut elements = vec![DocumentElement::Paragraph {
        runs: vec![FormattedRun {
            text: "Ordinary paragraph content. ".repeat(300),
            formatting: TextFormatting::default(),
        }],
    }];
    elements.push(lazy_image(image.path(), "Later image"));
    let mut app = App::new_lazy(
        make_synthetic_document(elements),
        &AppOptions {
            images: true,
            ..Default::default()
        },
    );
    assert!(app.image_protocols.is_empty());
    assert_eq!(app.budget_tracker.cumulative_bytes_used(), 0);
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    lazy_frame(&mut terminal, &mut app);
    assert!(
        app.image_protocols.is_empty(),
        "No decoding behind a long first paragraph"
    );
    app.scroll_offset = 1;
    app.page_line_offset = 0;
    lazy_frame(&mut terminal, &mut app);
    assert!(app.image_protocols.contains_key(&1));
    assert!(buffer_has_red_halfblock(terminal.backend().buffer()));
    assert_eq!(app.budget_tracker.cumulative_bytes_used(), 16);
}

#[test]
fn lazy_images_evict_release_and_reload_without_swapping_identity() {
    let red = create_temp_image_file(&generate_test_png(2, 2, [255, 0, 0, 255]), ".png");
    let blue = create_temp_image_file(&generate_test_png(2, 2, [0, 0, 255, 255]), ".png");
    let policy = ImageBudgetPolicy::new(10, 10, 100, 16, 16);
    let mut app = App::new_lazy(
        make_synthetic_document(vec![
            lazy_image(red.path(), "Red"),
            lazy_image(blue.path(), "Blue"),
        ]),
        &AppOptions {
            images: true,
            image_budget: Some(policy),
            ..Default::default()
        },
    );
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    lazy_frame(&mut terminal, &mut app);
    assert!(buffer_has_red_halfblock(terminal.backend().buffer()));
    assert_eq!(app.image_protocols.len(), 1);
    app.scroll_offset = 1;
    lazy_frame(&mut terminal, &mut app);
    assert!(buffer_has_blue_halfblock(terminal.backend().buffer()));
    assert!(!buffer_has_red_halfblock(terminal.backend().buffer()));
    assert!(!app.image_protocols.contains_key(&0));
    assert!(app.image_protocols.contains_key(&1));
    assert_eq!(app.budget_tracker.cumulative_bytes_used(), 16);
    for index in [0, 1, 0, 1] {
        app.scroll_offset = index;
        lazy_frame(&mut terminal, &mut app);
        assert!(app.image_protocols.contains_key(&index));
        assert_eq!(app.budget_tracker.cumulative_bytes_used(), 16);
        assert_eq!(app.budget_tracker.admitted_count(), 1);
        assert_eq!(
            buffer_has_red_halfblock(terminal.backend().buffer()),
            index == 0
        );
        assert_eq!(
            buffer_has_blue_halfblock(terminal.backend().buffer()),
            index == 1
        );
    }
}

#[test]
fn lazy_images_bound_protocol_count_and_original_navigation_survives_eviction() {
    let files: Vec<_> = (0..12)
        .map(|_| create_temp_image_file(&generate_test_png(2, 2, [255, 0, 0, 255]), ".png"))
        .collect();
    let mut elements = vec![DocumentElement::Paragraph {
        runs: vec![FormattedRun {
            text: "Ordinary paragraph content. ".repeat(100),
            formatting: TextFormatting::default(),
        }],
    }];
    elements.extend(files.iter().map(|file| lazy_image(file.path(), "Image")));
    let mut app = App::new_lazy(
        make_synthetic_document(elements),
        &AppOptions {
            images: true,
            ..Default::default()
        },
    );
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    for index in 1..=12 {
        app.scroll_offset = index;
        lazy_frame(&mut terminal, &mut app);
        assert!(app.image_protocols.len() <= 8);
        assert_eq!(
            app.budget_tracker.cumulative_bytes_used(),
            (app.image_protocols.len() * 16) as u64
        );
    }
    assert!(!app.image_protocols.contains_key(&1));
    app.scroll_offset = 2;
    app.page_size = (78, 17);
    app.turn_page(false);
    assert_eq!(
        (app.scroll_offset, app.page_line_offset),
        (1, 0),
        "Back paging must retain atomic image geometry after eviction"
    );
    app.scroll_offset = 12;
    app.image_open_requested = true;
    let request = app.take_image_request().unwrap();
    assert_eq!(request.index, 11);
    assert_eq!(
        request.images,
        files
            .iter()
            .map(|file| file.path().to_path_buf())
            .collect::<Vec<_>>()
    );
    assert!(app.image_protocols.len() <= 8);
    app.scroll_offset = 1;
    app.image_open_requested = true;
    let request = app.take_image_request().unwrap();
    assert_eq!(request.index, 0);
    assert!(
        app.image_protocols.contains_key(&1),
        "An evicted selected image is admitted again"
    );
    assert!(app.image_protocols.len() <= 8);
}

#[test]
fn lazy_images_keep_precise_rejections_and_do_not_charge_failed_decode() {
    let corrupt = create_temp_image_file(b"not an image", ".png");
    let red = create_temp_image_file(&generate_test_png(2, 2, [255, 0, 0, 255]), ".png");
    let oversized = create_temp_image_file(&generate_test_png(3, 3, [0, 0, 255, 255]), ".png");
    let policy = ImageBudgetPolicy::new(2, 2, 4, 16, 16);
    let mut app = App::new_lazy(
        make_synthetic_document(vec![
            lazy_image(corrupt.path(), "Bad"),
            lazy_image(red.path(), "Red"),
            lazy_image(oversized.path(), "Too big"),
        ]),
        &AppOptions {
            images: true,
            image_budget: Some(policy),
            ..Default::default()
        },
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 25)).unwrap();
    lazy_frame(&mut terminal, &mut app);
    assert!(
        app.image_placeholders
            .get(&0)
            .unwrap()
            .starts_with("unsupported format")
    );
    assert!(app.image_protocols.contains_key(&1));
    assert_eq!(app.budget_tracker.cumulative_bytes_used(), 16);
    app.scroll_offset = 2;
    lazy_frame(&mut terminal, &mut app);
    assert!(
        app.image_placeholders
            .get(&2)
            .unwrap()
            .contains("dimensions")
    );
    assert_eq!(app.budget_tracker.cumulative_bytes_used(), 16);
    app.image_open_requested = true;
    assert!(app.take_image_request().is_none());
    app.scroll_offset = 1;
    app.image_open_requested = true;
    let request = app.take_image_request().unwrap();
    assert_eq!(request.images, vec![red.path().to_path_buf()]);
}

#[test]
fn image_header_admission_does_not_charge_and_release_is_idempotent() {
    let red = create_temp_image_file(&generate_test_png(2, 2, [255, 0, 0, 255]), ".png");
    let mut tracker = ImageBudgetTracker::new(ImageBudgetPolicy::new(2, 2, 4, 16, 16));
    tracker.validate_image(red.path()).unwrap();
    assert_eq!(tracker.cumulative_bytes_used(), 0);
    let image = tracker.load_and_decode_element(7, red.path()).unwrap();
    assert_eq!(tracker.cumulative_bytes_used(), 16);
    drop(image);
    tracker.release_element(7);
    tracker.release_element(7);
    assert_eq!(tracker.cumulative_bytes_used(), 0);
    assert_eq!(tracker.admitted_count(), 0);
    let tiny = ImageBudgetTracker::new(ImageBudgetPolicy::new(2, 2, 4, 16, 15));
    assert!(matches!(
        tiny.validate_image(red.path()),
        Err(ImageRejectionReason::ExceedsCumulativeBudget { .. })
    ));
}

#[test]
fn lazy_images_pin_visible_protocols_and_retry_capacity_on_next_page() {
    let red = create_temp_image_file(&generate_test_png(2, 2, [255, 0, 0, 255]), ".png");
    let blue = create_temp_image_file(&generate_test_png(2, 2, [0, 0, 255, 255]), ".png");
    let mut app = App::new_lazy(
        make_synthetic_document(vec![
            lazy_image(red.path(), "Red"),
            lazy_image(blue.path(), "Blue"),
        ]),
        &AppOptions {
            images: true,
            image_budget: Some(ImageBudgetPolicy::new(2, 2, 4, 16, 16)),
            ..Default::default()
        },
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 45)).unwrap();
    lazy_frame(&mut terminal, &mut app);
    assert!(app.image_protocols.contains_key(&0));
    assert!(!app.image_protocols.contains_key(&1));
    assert!(buffer_has_red_halfblock(terminal.backend().buffer()));
    assert!(buffer_to_string(terminal.backend().buffer()).contains("cumulative budget exceeded"));
    app.scroll_offset = 1;
    lazy_frame(&mut terminal, &mut app);
    assert!(app.image_protocols.contains_key(&1));
    assert!(!app.image_placeholders.contains_key(&1));
    assert!(buffer_has_blue_halfblock(terminal.backend().buffer()));
}

#[test]
fn lazy_images_bound_cache_when_a_tall_view_pins_every_entry() {
    let file = create_temp_image_file(&generate_test_png(2, 2, [255, 0, 0, 255]), ".png");
    let mut app = App::new_lazy(
        make_synthetic_document((0..12).map(|_| lazy_image(file.path(), "Red")).collect()),
        &AppOptions {
            images: true,
            ..Default::default()
        },
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 230)).unwrap();
    lazy_frame(&mut terminal, &mut app);
    assert_eq!(app.image_protocols.len(), 8);
    assert_eq!(app.budget_tracker.cumulative_bytes_used(), 8 * 16);
    assert!(
        app.image_placeholders
            .values()
            .all(|reason| reason.starts_with("image cache capacity exceeded"))
    );
    app.scroll_offset = 8;
    lazy_frame(&mut terminal, &mut app);
    assert!(app.image_protocols.contains_key(&8));
    assert_eq!(app.image_protocols.len(), 8);
}

#[test]
fn eager_library_rendering_keeps_existing_capacity_reasons() {
    let red = create_temp_image_file(&generate_test_png(2, 2, [255, 0, 0, 255]), ".png");
    let mut app = App::new(
        make_synthetic_document(vec![
            lazy_image(red.path(), "First"),
            lazy_image(red.path(), "Second"),
        ]),
        &AppOptions {
            images: true,
            image_budget: Some(ImageBudgetPolicy::new(2, 2, 4, 16, 16)),
            ..Default::default()
        },
    );
    let reason = app.image_placeholders.get(&1).unwrap().clone();
    let mut terminal = Terminal::new(TestBackend::new(100, 45)).unwrap();
    lazy_frame(&mut terminal, &mut app);
    assert_eq!(app.image_placeholders.get(&1), Some(&reason));
    assert!(buffer_to_string(terminal.backend().buffer()).contains("cumulative budget exceeded"));
}
