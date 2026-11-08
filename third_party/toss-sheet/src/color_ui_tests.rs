use super::*;
use ratatui::{backend::TestBackend, buffer::Buffer, style::Color};
fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
fn app(sheet: &str, no_header: bool) -> TuiState {
    let mut app = TuiState::new(
        Workbook::open_with_options(fixture("colors.xlsx"), no_header, None).unwrap(),
        sheet,
        &crate::config::Config::default(),
        true,
    )
    .unwrap();
    app.toggle_colors().unwrap();
    app
}
fn render(app: &mut TuiState) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(150, 30)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    terminal.backend().buffer().clone()
}
fn find(b: &Buffer, marker: &str) -> (u16, u16) {
    for y in 0..b.area.height {
        for x in 0..b.area.width.saturating_sub(marker.len() as u16) {
            if marker
                .chars()
                .enumerate()
                .all(|(i, c)| b[(x + i as u16, y)].symbol() == c.to_string())
            {
                return (x, y);
            }
        }
    }
    panic!("marker missing: {marker}");
}
fn expect(b: &Buffer, marker: &str, fg: Color, bg: Color) {
    let p = find(b, marker);
    for dx in 0..marker.len() as u16 {
        let c = &b[(p.0 + dx, p.1)];
        assert_eq!((c.fg, c.bg), (fg, bg), "{marker}");
    }
}
#[test]
fn starts_monochrome_and_key_toggle_restores_exact_excel_colors() {
    let mut app = TuiState::new(
        Workbook::open(fixture("colors.xlsx")).unwrap(),
        "Colors",
        &crate::config::Config::default(),
        true,
    )
    .unwrap();
    assert!(!app.excel_colors);
    assert!(app.workbook.color_at(1, 1).is_none());
    let b = render(&mut app);
    for marker in [
        "RGB_MARK",
        "THEME_MARK",
        "INDEX_MARK",
        "PLAIN_MARK",
        "EXCEL_COLOR",
    ] {
        expect(&b, marker, Color::White, Color::Black);
    }
    find(&b, "T:Excel colors");
    let press = |app: &mut TuiState, c, mods| {
        app.handle_event(Event::Key(KeyEvent::new(KeyCode::Char(c), mods)))
    };
    press(&mut app, 't', crossterm::event::KeyModifiers::NONE);
    assert!(app.excel_colors);
    let b = render(&mut app);
    expect(
        &b,
        "RGB_MARK",
        Color::Rgb(192, 0, 0),
        Color::Rgb(255, 242, 204),
    );
    expect(
        &b,
        "PLAIN_MARK",
        Color::Rgb(0, 0, 0),
        Color::Rgb(255, 255, 255),
    );
    find(&b, "T:Black/white");
    app.switch_to_next_sheet().unwrap();
    assert!(app.excel_colors);
    press(&mut app, 'T', crossterm::event::KeyModifiers::SHIFT);
    assert!(!app.excel_colors);
    let b = render(&mut app);
    expect(&b, "RGB_MARK", Color::White, Color::Black);
    press(&mut app, '/', crossterm::event::KeyModifiers::NONE);
    press(&mut app, 't', crossterm::event::KeyModifiers::NONE);
    assert_eq!(app.search_query, "t");
    assert!(!app.excel_colors);
}

#[test]
fn monochrome_keeps_cursor_and_search_highlights() {
    let mut app = app("Colors", false);
    app.excel_colors = false;
    app.cursor_col = 1;
    app.search_matches = vec![(1, 1)];
    app.current_match_index = Some(0);
    let colors = app.current_theme.colors();
    let b = render(&mut app);
    expect(
        &b,
        "RGB_MARK",
        colors.current_cell_fg,
        colors.current_cell_bg,
    );
    expect(
        &b,
        "THEME_MARK",
        colors.current_search_fg,
        colors.current_search_bg,
    );
    expect(&b, "PLAIN_MARK", Color::White, Color::Black);
}

#[test]
fn real_numeric_cells_render_search_and_export_without_rounding() {
    let mut workbook = Workbook::open(fixture("numeric-precision.xlsx")).unwrap();
    let data = workbook.load_sheet("Data").unwrap();
    let mut json = Vec::new();
    crate::export::json(&mut json, &data, "Data").unwrap();
    let document: serde_json::Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(document["data"][0][1].as_f64(), Some(0.004));
    assert_eq!(document["data"][2][1].as_f64(), Some(1.2345678901234567));
    let mut app = TuiState::new(workbook, "Data", &crate::config::Config::default(), true).unwrap();
    let b = render(&mut app);
    for marker in [
        "0.004",
        "-0.0004",
        "1.2345678901234567",
        "0.30000000000000004",
        "1234567.89",
    ] {
        find(&b, marker);
    }
    app.search_query = "0.004".to_owned();
    app.perform_search();
    app.tick_search();
    assert_eq!(app.search_matches, vec![(0, 1)]);
}

#[test]
fn real_number_formats_render_search_and_switch_without_changing_raw_values() {
    let mut workbook = Workbook::open(fixture("number-formats.xlsx")).unwrap();
    let raw = workbook.load_sheet("Numbers").unwrap();
    let mut before = Vec::new();
    crate::export::json(&mut before, &raw, "Numbers").unwrap();
    let mut app =
        TuiState::new(workbook, "Numbers", &crate::config::Config::default(), true).unwrap();
    let expected = [
        "25%",
        "12.35%",
        "¥1,200.00",
        "001234",
        "(¥1,200.50)",
        "1.23E+04",
        "0.004",
        "1.20",
        "0.25",
        "zero",
        "0.25%",
        "001234",
        "25%",
        "",
    ];
    let origin = app.sheet_data.data_start();
    for (row, expected) in expected.iter().enumerate() {
        let cell = app.sheet_data.get_cell(row, 1).0.unwrap();
        assert_eq!(
            app.workbook
                .display_value(origin.0 + row, origin.1 + 1, &cell),
            *expected
        );
    }
    let b = render(&mut app);
    find(&b, "25%");
    find(&b, "¥1,200.00");
    find(&b, "001234");
    app.search_query = "¥1,200.00".to_owned();
    app.perform_search();
    app.tick_search();
    assert_eq!(app.search_matches, vec![(2, 1)]);
    app.toggle_colors().unwrap();
    let cell = app.sheet_data.get_cell(0, 1).0.unwrap();
    assert_eq!(cell.to_raw_string(), "0.25");
    app.switch_to_next_sheet().unwrap();
    let cell = app.sheet_data.get_cell(0, 1).0.unwrap();
    assert_eq!(app.workbook.display_value(1, 1, &cell), "0.25");
    app.switch_to_next_sheet().unwrap();
    assert_eq!(
        app.sheet_data.headers(),
        &["25%".to_owned(), "001234".to_owned()]
    );
    let mut workbook = Workbook::open(fixture("number-formats.xlsx")).unwrap();
    workbook.enable_number_formats();
    let raw = workbook.load_sheet("Numbers").unwrap();
    let mut after = Vec::new();
    crate::export::json(&mut after, &raw, "Numbers").unwrap();
    assert_eq!(before, after);
}

#[test]
fn first_row_is_navigable_searchable_and_styled_in_data_mode() {
    let mut app = app("Colors", true);
    assert_eq!(app.current_cell_address(), "A1");
    assert_eq!(app.sheet_data.height(), 6);
    assert_eq!(
        app.sheet_data.headers(),
        &["A".to_owned(), "B".to_owned(), "C".to_owned()]
    );
    assert_eq!(
        app.sheet_data.get_cell(0, 1).0.unwrap().to_raw_string(),
        "EXCEL_COLOR"
    );
    let b = render(&mut app);
    expect(
        &b,
        "EXCEL_COLOR",
        Color::Rgb(255, 255, 255),
        Color::Rgb(36, 64, 98),
    );
    app.search_query = "EXCEL_COLOR".to_owned();
    app.perform_search();
    app.tick_search();
    assert_eq!(app.search_matches, vec![(0, 1)]);
    assert_eq!(app.current_cell_address(), "B1");
    app.switch_to_next_sheet().unwrap();
    assert_eq!(app.current_cell_address(), "A1");
    assert_eq!(
        app.sheet_data.get_cell(0, 1).0.unwrap().to_raw_string(),
        "EXCEL_COLOR"
    );
}

#[test]
fn original_colors_leave_chrome_dark_and_grid_in_reserved_spacing() {
    let mut app = app("Colors", false);
    for original in [true, false] {
        app.excel_colors = original;
        let b = render(&mut app);
        expect(&b, "Colors (Sheet 1/3)", Color::White, Color::Black);
        assert_eq!(b[(0, 0)].fg, Color::White);
        assert_eq!(b[(0, 0)].bg, Color::Black);
        assert_eq!(b[(140, 20)].bg, Color::Black);
        let (x, y) = find(&b, "RGB_MARK");
        assert_eq!(b[(x - 1, y)].symbol(), "│");
        assert_eq!(b[(x, y + 1)].symbol(), "─");
        assert_eq!(b[(x - 1, y + 1)].symbol(), "┼");
        assert_eq!(
            b[(x - 1, y)].bg,
            if original {
                Color::Rgb(255, 255, 255)
            } else {
                Color::Black
            }
        );
        assert_ne!(b[(x - 1, y)].fg, b[(x - 1, y)].bg);
    }
}

#[test]
fn grid_keeps_scrolled_cursor_visible_in_small_viewports() {
    let mut app = app("Colors", false);
    app.cursor_row = 4;
    app.cursor_col = 1;
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    assert!(app.scroll_offset > 0);
    expect(
        terminal.backend().buffer(),
        "PLAIN_MARK",
        app.current_theme.colors().current_cell_fg,
        app.current_theme.colors().current_cell_bg,
    );
    for size in [(1, 1), (6, 5), (20, 8)] {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
    }
}

#[test]
fn explicit_borders_parse_inheritance_and_render_separately_from_gray_grid() {
    use crate::colors::BorderKind;
    let mut workbook = Workbook::open(fixture("borders.xlsx")).unwrap();
    workbook.enable_colors();
    workbook.load_sheet("Colors").unwrap();
    let black = workbook.color_at(1, 1).unwrap().borders[0].unwrap();
    assert_eq!(black.color, Color::Rgb(0, 0, 0));
    assert_eq!(black.kind, BorderKind::Thin);
    assert_eq!(
        workbook.color_at(2, 1).unwrap().borders[1].unwrap().color,
        Color::Rgb(255, 0, 0)
    );
    assert_eq!(
        workbook.color_at(2, 1).unwrap().borders[1].unwrap().kind,
        BorderKind::Thick
    );
    assert_eq!(
        workbook.color_at(3, 1).unwrap().borders[2].unwrap().kind,
        BorderKind::Dashed
    );
    assert_eq!(
        workbook.color_at(3, 1).unwrap().borders[2].unwrap().color,
        Color::Rgb(0, 0, 255)
    );
    assert_eq!(workbook.color_at(5, 1).unwrap().borders[0], Some(black));
    assert_eq!(
        workbook.color_at(5, 2).unwrap().borders[0].unwrap().kind,
        BorderKind::Dashed
    );
    assert_eq!(workbook.color_at(1, 2).unwrap().borders, [None; 4]);
    let mut app =
        TuiState::new(workbook, "Colors", &crate::config::Config::default(), true).unwrap();
    app.excel_colors = true;
    let b = render(&mut app);
    let (x, y) = find(&b, "RGB_MARK");
    assert_eq!(
        (b[(x, y - 1)].fg, b[(x, y - 1)].bg),
        (Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255))
    );
    let (cx, cy) = find(&b, "Red text on pale yellow");
    assert_eq!(b[(cx + 2, cy + 1)].fg, Color::DarkGray);
    let (x, y) = find(&b, "THEME_MARK");
    assert_eq!(b[(x - 1, y)].symbol(), "┃");
    assert_eq!(b[(x - 1, y)].fg, Color::Rgb(255, 0, 0));
    app.excel_colors = false;
    let b = render(&mut app);
    let (x, y) = find(&b, "RGB_MARK");
    assert_eq!(
        (b[(x, y - 1)].fg, b[(x, y - 1)].bg),
        (Color::DarkGray, Color::Black)
    );
    for marker in ["THEME_MARK", "INDEX_MARK"] {
        let (x, y) = find(&b, marker);
        assert_eq!(b[(x - 1, y)].symbol(), "│");
        assert_eq!(b[(x, y + 1)].symbol(), "─");
        assert_eq!(b[(x - 1, y)].fg, Color::DarkGray);
        assert_eq!(b[(x, y + 1)].fg, Color::DarkGray);
    }
    app.switch_to_next_sheet().unwrap();
    assert_eq!(app.workbook.color_at(1, 1).unwrap().borders, [None; 4]);
}

#[test]
fn deferred_colors_keep_data_navigation_and_uncached_formula_borders() {
    let mut app = TuiState::new(
        Workbook::open(fixture("borders.xlsx")).unwrap(),
        "Colors",
        &crate::config::Config::default(),
        true,
    )
    .unwrap();
    assert!(app.workbook.color_at(1, 1).is_none());
    app.cursor_row = 2;
    app.cursor_col = 1;
    app.search_query = "INDEX".to_owned();
    let before = app.sheet_data.get_cell(2, 1).0.unwrap().to_string();
    let widths = app.column_widths.clone();
    app.toggle_colors().unwrap();
    assert_eq!((app.cursor_row, app.cursor_col), (2, 1));
    assert_eq!(app.search_query, "INDEX");
    assert_eq!(app.column_widths, widths);
    assert_eq!(app.sheet_data.get_cell(2, 1).0.unwrap().to_string(), before);
    assert_eq!(
        app.workbook.color_at(5, 2).unwrap().borders[0]
            .unwrap()
            .kind,
        crate::colors::BorderKind::Dashed
    );
    app.toggle_colors().unwrap();
    app.toggle_colors().unwrap();
    assert!(app.excel_colors);
    assert_eq!(app.sheet_data.get_cell(2, 1).0.unwrap().to_string(), before);
}
#[test]
fn actual_rgb_theme_indexed_blank_header_and_default() {
    let mut app = app("Colors", false);
    let b = render(&mut app);
    expect(
        &b,
        "RGB_MARK",
        Color::Rgb(192, 0, 0),
        Color::Rgb(255, 242, 204),
    );
    expect(
        &b,
        "THEME_MARK",
        Color::Rgb(55, 96, 146),
        Color::Rgb(149, 179, 215),
    );
    expect(
        &b,
        "INDEX_MARK",
        Color::Rgb(0, 0, 255),
        Color::Rgb(255, 255, 0),
    );
    expect(
        &b,
        "PLAIN_MARK",
        Color::Rgb(0, 0, 0),
        Color::Rgb(255, 255, 255),
    );
    expect(
        &b,
        "EXCEL_COLOR",
        Color::Rgb(255, 255, 255),
        Color::Rgb(36, 64, 98),
    );
    let (x, _) = find(&b, "RGB_MARK");
    let (_, y) = find(&b, "BLANK");
    assert_eq!(b[(x, y)].bg, Color::Rgb(0, 255, 0));
}
#[test]
fn cursor_and_search_remain_readable_over_original_colors() {
    let mut app = app("Colors", false);
    app.cursor_col = 1;
    let b = render(&mut app);
    let c = app.current_theme.colors();
    expect(&b, "RGB_MARK", c.current_cell_fg, c.current_cell_bg);
    app.search_matches = vec![(0, 1), (1, 1)];
    app.current_match_index = Some(1);
    let b = render(&mut app);
    expect(&b, "RGB_MARK", c.current_cell_fg, c.current_cell_bg);
    expect(&b, "THEME_MARK", c.current_search_fg, c.current_search_bg);
    app.cursor_col = 0;
    let b = render(&mut app);
    expect(&b, "RGB_MARK", c.search_match_fg, c.search_match_bg);
}
#[test]
fn switching_sheets_replaces_styles_and_offset_no_header_coordinates() {
    let mut app = app("Colors", false);
    app.switch_to_next_sheet().unwrap();
    let b = render(&mut app);
    expect(
        &b,
        "RGB_MARK",
        Color::Rgb(255, 255, 255),
        Color::Rgb(128, 0, 128),
    );
    app.switch_to_next_sheet().unwrap();
    app.cursor_col = 1;
    let b = render(&mut app);
    expect(
        &b,
        "OFFSET_MARK",
        Color::Rgb(192, 0, 0),
        Color::Rgb(255, 242, 204),
    );
    let mut app = super::color_ui_tests::app("Offset", true);
    app.cursor_col = 1;
    let b = render(&mut app);
    expect(
        &b,
        "OFFSET_MARK",
        Color::Rgb(192, 0, 0),
        Color::Rgb(255, 242, 204),
    );
}
#[test]
fn custom_palette_exports_untouched_and_csv_no_colors() {
    let mut w = Workbook::open(fixture("colors-custom.xlsx")).unwrap();
    assert!(w.color_at(3, 1).is_none());
    let before = w.load_sheet("Colors").unwrap().rows[2][1].to_raw_string();
    w.enable_colors();
    w.load_sheet("Colors").unwrap();
    let c = w.color_at(3, 1).unwrap();
    assert_eq!(c.fg, Color::Rgb(18, 52, 86));
    assert_eq!(c.bg, Color::Rgb(101, 67, 33));
    assert_eq!(before, "INDEX_MARK");
    let mut csv = Workbook::open(fixture("edge.csv")).unwrap();
    csv.enable_colors();
    assert!(csv.color_at(0, 0).is_none());
}
#[test]
fn invalid_or_oversized_theme_metadata_falls_back_without_losing_data() {
    for name in ["colors-invalid.xlsx", "colors-large-style.xlsx"] {
        let mut w = Workbook::open(fixture(name)).unwrap();
        w.enable_colors();
        assert!(w.color_at(1, 1).is_none());
        assert_eq!(
            w.load_sheet("Colors").unwrap().rows[0][1].to_raw_string(),
            "RGB_MARK"
        );
    }
}
