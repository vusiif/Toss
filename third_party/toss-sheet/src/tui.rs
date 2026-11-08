use crate::workbook::{CellValue, LazySheetData, SheetData, Workbook};
use anyhow::Result;
use arboard::Clipboard;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap},
};
use std::io;
use std::time::{Duration, Instant};

/// Available themes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Default,
    Dracula,
    SolarizedDark,
    SolarizedLight,
    GitHubDark,
    Nord,
}

impl Theme {
    /// Get all available themes
    pub fn all() -> &'static [Theme] {
        &[
            Theme::Default,
            Theme::Dracula,
            Theme::SolarizedDark,
            Theme::SolarizedLight,
            Theme::GitHubDark,
            Theme::Nord,
        ]
    }

    /// Get the next theme in the cycle
    pub fn next(&self) -> Theme {
        let themes = Self::all();
        let current_idx = themes.iter().position(|t| t == self).unwrap_or(0);
        themes[(current_idx + 1) % themes.len()]
    }

    /// Get theme name for display
    pub fn name(&self) -> &'static str {
        match self {
            Theme::Default => "Default",
            Theme::Dracula => "Dracula",
            Theme::SolarizedDark => "Solarized Dark",
            Theme::SolarizedLight => "Solarized Light",
            Theme::GitHubDark => "GitHub Dark",
            Theme::Nord => "Nord",
        }
    }

    /// Get the color scheme for this theme
    pub fn colors(&self) -> ColorScheme {
        match self {
            Theme::Default => ColorScheme::default_theme(),
            Theme::Dracula => ColorScheme::dracula(),
            Theme::SolarizedDark => ColorScheme::solarized_dark(),
            Theme::SolarizedLight => ColorScheme::solarized_light(),
            Theme::GitHubDark => ColorScheme::github_dark(),
            Theme::Nord => ColorScheme::nord(),
        }
    }
}

/// Color scheme for the TUI
#[derive(Debug, Clone)]
pub struct ColorScheme {
    // Cell type colors
    pub string_fg: Color,
    pub number_fg: Color,
    pub bool_fg: Color,
    pub datetime_fg: Color,
    pub error_fg: Color,
    pub empty_fg: Color,

    // UI element colors
    pub header_fg: Color,
    pub header_bg: Option<Color>,
    pub current_cell_fg: Color,
    pub current_cell_bg: Color,
    pub current_row_bg: Color,
    pub current_col_fg: Color,
    pub alternating_row_bg: Option<Color>,

    // Search colors
    pub search_match_fg: Color,
    pub search_match_bg: Color,
    pub current_search_fg: Color,
    pub current_search_bg: Color,

    // Border and status bar
    pub border_fg: Color,
    pub status_bar_fg: Color,
    pub status_bar_bg: Option<Color>,
}

impl ColorScheme {
    /// Default theme (current behavior with enhancements)
    pub fn default_theme() -> Self {
        Self {
            // Cell types
            string_fg: Color::White,
            number_fg: Color::Cyan,
            bool_fg: Color::Magenta,
            datetime_fg: Color::Green,
            error_fg: Color::Red,
            empty_fg: Color::DarkGray,

            // UI elements
            header_fg: Color::Yellow,
            header_bg: None,
            current_cell_fg: Color::White,
            current_cell_bg: Color::Blue,
            current_row_bg: Color::DarkGray,
            current_col_fg: Color::Cyan,
            alternating_row_bg: Some(Color::Rgb(25, 25, 28)),

            // Search
            search_match_fg: Color::Black,
            search_match_bg: Color::LightYellow,
            current_search_fg: Color::Black,
            current_search_bg: Color::Yellow,

            // Borders/status
            border_fg: Color::White,
            status_bar_fg: Color::White,
            status_bar_bg: None,
        }
    }

    /// Dracula theme (purple/pink aesthetic)
    pub fn dracula() -> Self {
        Self {
            // Cell types - Dracula palette
            string_fg: Color::Rgb(248, 248, 242),  // Foreground
            number_fg: Color::Rgb(189, 147, 249),  // Purple
            bool_fg: Color::Rgb(255, 121, 198),    // Pink
            datetime_fg: Color::Rgb(80, 250, 123), // Green
            error_fg: Color::Rgb(255, 85, 85),     // Red
            empty_fg: Color::Rgb(98, 114, 164),    // Comment

            // UI elements
            header_fg: Color::Rgb(139, 233, 253),    // Cyan
            header_bg: Some(Color::Rgb(68, 71, 90)), // Current line
            current_cell_fg: Color::Rgb(248, 248, 242),
            current_cell_bg: Color::Rgb(98, 114, 164), // Comment (darker)
            current_row_bg: Color::Rgb(68, 71, 90),    // Current line
            current_col_fg: Color::Rgb(139, 233, 253), // Cyan
            alternating_row_bg: Some(Color::Rgb(50, 52, 65)),

            // Search
            search_match_fg: Color::Rgb(40, 42, 54), // Background
            search_match_bg: Color::Rgb(241, 250, 140), // Yellow
            current_search_fg: Color::Rgb(40, 42, 54),
            current_search_bg: Color::Rgb(255, 184, 108), // Orange

            // Borders/status
            border_fg: Color::Rgb(98, 114, 164), // Comment
            status_bar_fg: Color::Rgb(248, 248, 242),
            status_bar_bg: Some(Color::Rgb(68, 71, 90)),
        }
    }

    /// Solarized Dark theme
    pub fn solarized_dark() -> Self {
        Self {
            // Cell types - Solarized Dark
            string_fg: Color::Rgb(131, 148, 150), // Base0
            number_fg: Color::Rgb(38, 139, 210),  // Blue
            bool_fg: Color::Rgb(211, 54, 130),    // Magenta
            datetime_fg: Color::Rgb(133, 153, 0), // Green
            error_fg: Color::Rgb(220, 50, 47),    // Red
            empty_fg: Color::Rgb(88, 110, 117),   // Base01

            // UI elements
            header_fg: Color::Rgb(181, 137, 0),     // Yellow
            header_bg: Some(Color::Rgb(7, 54, 66)), // Base02
            current_cell_fg: Color::Rgb(253, 246, 227),
            current_cell_bg: Color::Rgb(88, 110, 117), // Base01
            current_row_bg: Color::Rgb(7, 54, 66),     // Base02
            current_col_fg: Color::Rgb(42, 161, 152),  // Cyan
            alternating_row_bg: Some(Color::Rgb(0, 43, 54)),

            // Search
            search_match_fg: Color::Rgb(0, 43, 54),
            search_match_bg: Color::Rgb(181, 137, 0), // Yellow
            current_search_fg: Color::Rgb(0, 43, 54),
            current_search_bg: Color::Rgb(203, 75, 22), // Orange

            // Borders/status
            border_fg: Color::Rgb(88, 110, 117),
            status_bar_fg: Color::Rgb(131, 148, 150),
            status_bar_bg: Some(Color::Rgb(7, 54, 66)),
        }
    }

    /// Solarized Light theme
    pub fn solarized_light() -> Self {
        Self {
            // Cell types - Solarized Light
            string_fg: Color::Rgb(101, 123, 131), // Base00
            number_fg: Color::Rgb(38, 139, 210),  // Blue
            bool_fg: Color::Rgb(211, 54, 130),    // Magenta
            datetime_fg: Color::Rgb(133, 153, 0), // Green
            error_fg: Color::Rgb(220, 50, 47),    // Red
            empty_fg: Color::Rgb(147, 161, 161),  // Base1

            // UI elements
            header_fg: Color::Rgb(181, 137, 0),         // Yellow
            header_bg: Some(Color::Rgb(238, 232, 213)), // Base2
            current_cell_fg: Color::Rgb(0, 43, 54),     // Base02
            current_cell_bg: Color::Rgb(147, 161, 161), // Base1
            current_row_bg: Color::Rgb(238, 232, 213),  // Base2
            current_col_fg: Color::Rgb(42, 161, 152),   // Cyan
            alternating_row_bg: Some(Color::Rgb(253, 246, 227)),

            // Search
            search_match_fg: Color::Rgb(0, 43, 54),
            search_match_bg: Color::Rgb(181, 137, 0), // Yellow
            current_search_fg: Color::Rgb(253, 246, 227),
            current_search_bg: Color::Rgb(203, 75, 22), // Orange

            // Borders/status
            border_fg: Color::Rgb(147, 161, 161),
            status_bar_fg: Color::Rgb(101, 123, 131),
            status_bar_bg: Some(Color::Rgb(238, 232, 213)),
        }
    }

    /// GitHub Dark theme
    pub fn github_dark() -> Self {
        Self {
            // Cell types - GitHub Dark
            string_fg: Color::Rgb(201, 209, 217),   // fgDefault
            number_fg: Color::Rgb(121, 192, 255),   // prettylights-syntax-constant
            bool_fg: Color::Rgb(255, 125, 163),     // prettylights-syntax-entity
            datetime_fg: Color::Rgb(127, 219, 202), // prettylights-syntax-string
            error_fg: Color::Rgb(248, 81, 73),      // danger-fg
            empty_fg: Color::Rgb(110, 118, 129),    // fgMuted

            // UI elements
            header_fg: Color::Rgb(255, 199, 119), // prettylights-syntax-entity-tag
            header_bg: Some(Color::Rgb(33, 38, 45)), // canvas-subtle
            current_cell_fg: Color::Rgb(201, 209, 217),
            current_cell_bg: Color::Rgb(56, 139, 253), // accent-emphasis
            current_row_bg: Color::Rgb(33, 38, 45),    // canvas-subtle
            current_col_fg: Color::Rgb(121, 192, 255),
            alternating_row_bg: Some(Color::Rgb(22, 27, 34)),

            // Search
            search_match_fg: Color::Rgb(13, 17, 23),
            search_match_bg: Color::Rgb(187, 128, 9), // attention-emphasis
            current_search_fg: Color::Rgb(13, 17, 23),
            current_search_bg: Color::Rgb(242, 130, 33), // severe-emphasis

            // Borders/status
            border_fg: Color::Rgb(48, 54, 61), // border-default
            status_bar_fg: Color::Rgb(201, 209, 217),
            status_bar_bg: Some(Color::Rgb(33, 38, 45)),
        }
    }

    /// Nord theme (cool blue/cyan palette)
    pub fn nord() -> Self {
        Self {
            // Cell types - Nord
            string_fg: Color::Rgb(216, 222, 233),   // nord4
            number_fg: Color::Rgb(136, 192, 208),   // nord8
            bool_fg: Color::Rgb(180, 142, 173),     // nord15
            datetime_fg: Color::Rgb(163, 190, 140), // nord14
            error_fg: Color::Rgb(191, 97, 106),     // nord11
            empty_fg: Color::Rgb(76, 86, 106),      // nord3

            // UI elements
            header_fg: Color::Rgb(235, 203, 139),    // nord13
            header_bg: Some(Color::Rgb(59, 66, 82)), // nord1
            current_cell_fg: Color::Rgb(236, 239, 244),
            current_cell_bg: Color::Rgb(94, 129, 172), // nord9
            current_row_bg: Color::Rgb(59, 66, 82),    // nord1
            current_col_fg: Color::Rgb(136, 192, 208), // nord8
            alternating_row_bg: Some(Color::Rgb(46, 52, 64)),

            // Search
            search_match_fg: Color::Rgb(46, 52, 64),
            search_match_bg: Color::Rgb(235, 203, 139), // nord13
            current_search_fg: Color::Rgb(46, 52, 64),
            current_search_bg: Color::Rgb(208, 135, 112), // nord12

            // Borders/status
            border_fg: Color::Rgb(76, 86, 106), // nord3
            status_bar_fg: Color::Rgb(216, 222, 233),
            status_bar_bg: Some(Color::Rgb(59, 66, 82)),
        }
    }

    /// Get foreground color for a cell based on its value type
    pub fn cell_color(&self, cell: &CellValue) -> Color {
        match cell {
            CellValue::Empty => self.empty_fg,
            CellValue::String(_) => self.string_fg,
            CellValue::Int(_) | CellValue::Float(_) => self.number_fg,
            CellValue::Bool(_) => self.bool_fg,
            CellValue::Error(_) => self.error_fg,
            CellValue::DateTime(_) => self.datetime_fg,
        }
    }
}

/// Cached row data for lazy loading
struct RowCache {
    start_row: usize,
    rows: Vec<Vec<CellValue>>,
    formulas: Vec<Vec<Option<String>>>,
}

/// Sheet data source (either eager or lazy)
enum SheetDataSource {
    Eager(SheetData),
    Lazy {
        data: LazySheetData,
        cache: Option<RowCache>,
        cache_size: usize, // Number of rows to cache at once
    },
}

impl SheetDataSource {
    fn data_start(&self) -> (usize, usize) {
        let start = match self {
            Self::Eager(data) => data.data_start,
            Self::Lazy { data, .. } => data.data_start,
        };
        (start.0 as usize, start.1 as usize)
    }

    fn headers(&self) -> &[String] {
        match self {
            SheetDataSource::Eager(data) => &data.headers,
            SheetDataSource::Lazy { data, .. } => &data.headers,
        }
    }

    fn width(&self) -> usize {
        match self {
            SheetDataSource::Eager(data) => data.width,
            SheetDataSource::Lazy { data, .. } => data.width,
        }
    }

    fn height(&self) -> usize {
        match self {
            SheetDataSource::Eager(data) => data.height,
            SheetDataSource::Lazy { data, .. } => data.height,
        }
    }

    /// Fetches rows with automatic cache management
    fn get_rows(
        &mut self,
        start: usize,
        count: usize,
    ) -> (&[Vec<CellValue>], &[Vec<Option<String>>]) {
        match self {
            SheetDataSource::Eager(data) => {
                let start = start.min(data.rows.len());
                let end = start.saturating_add(count).min(data.rows.len());
                (&data.rows[start..end], &data.formulas[start..end])
            }
            SheetDataSource::Lazy {
                data,
                cache,
                cache_size,
            } => {
                // Check if we need to reload the cache
                let needs_reload = match cache {
                    None => true,
                    Some(c) => {
                        start < c.start_row
                            || start.saturating_add(count).min(data.height)
                                > c.start_row.saturating_add(c.rows.len())
                    }
                };

                if needs_reload {
                    // Load new chunk centered around the requested start
                    let cache_start = start.saturating_sub(*cache_size / 4); // Start a bit before
                    let (rows, formulas) = data.get_rows(
                        cache_start,
                        (*cache_size).max(start.saturating_sub(cache_start).saturating_add(count)),
                    );
                    *cache = Some(RowCache {
                        start_row: cache_start,
                        rows,
                        formulas,
                    });
                }

                // Return from cache
                if let Some(c) = cache {
                    let offset = start.saturating_sub(c.start_row);
                    let offset = offset.min(c.rows.len());
                    let end = offset.saturating_add(count).min(c.rows.len());
                    (&c.rows[offset..end], &c.formulas[offset..end])
                } else {
                    // Shouldn't happen, but return empty slices
                    (&[], &[])
                }
            }
        }
    }

    fn get_cell(&mut self, row: usize, col: usize) -> (Option<CellValue>, Option<String>) {
        match self {
            SheetDataSource::Eager(data) => {
                let cell = data.rows.get(row).and_then(|r| r.get(col)).cloned();
                let formula = data
                    .formulas
                    .get(row)
                    .and_then(|r| r.get(col))
                    .and_then(|f| f.clone());
                (cell, formula)
            }
            SheetDataSource::Lazy { .. } => {
                // For lazy loading, get just the one row we need
                let (rows, formulas) = self.get_rows(row, 1);
                let cell = rows.first().and_then(|r| r.get(col)).cloned();
                let formula = formulas
                    .first()
                    .and_then(|r| r.get(col))
                    .and_then(|f| f.clone());
                (cell, formula)
            }
        }
    }
}

/// Progress information for long-running operations
#[derive(Debug, Clone)]
struct ProgressInfo {
    message: String,
    current: usize,
    total: usize,
    started_at: Instant,
}

impl ProgressInfo {
    fn new(message: impl Into<String>, total: usize) -> Self {
        Self {
            message: message.into(),
            current: 0,
            total,
            started_at: Instant::now(),
        }
    }

    fn update(&mut self, current: usize) {
        self.current = current;
    }

    fn percentage(&self) -> usize {
        (self.current * 100).checked_div(self.total).unwrap_or(100)
    }

    fn format(&self) -> String {
        let pct = self.percentage();
        let _elapsed = self.started_at.elapsed().as_secs_f64();
        format!(
            "{} {}% ({}/{})",
            self.message, pct, self.current, self.total
        )
    }
}

/// TUI application state
pub struct TuiState {
    workbook: Workbook,
    sheet_names: Vec<String>,
    current_sheet_index: usize,
    sheet_data: SheetDataSource,
    should_quit: bool,
    cursor_row: usize,               // Current row (0-indexed in data)
    cursor_col: usize,               // Current column (0-indexed)
    scroll_offset: usize,            // Vertical scroll offset
    horizontal_scroll_offset: usize, // Horizontal scroll offset
    horizontal_scroll_enabled: bool, // Whether horizontal scrolling is enabled
    column_widths: Vec<usize>,       // Cached column widths for horizontal scroll
    show_help: bool,                 // Help overlay visible
    show_cell_detail: bool,          // Cell detail popup visible
    cell_detail_scroll: usize,       // Scroll offset for cell detail popup
    // Search state
    search_mode: bool, // Whether we're in search input mode
    search_task: Option<crate::search::Search>,
    search_limited: bool,
    search_complete: bool,
    search_query: String,                // Current search query
    search_matches: Vec<(usize, usize)>, // List of (row, col) matches
    current_match_index: Option<usize>,  // Index in search_matches
    // Jump mode state
    jump_mode: bool,    // Whether we're in jump input mode
    jump_input: String, // Current jump input (row number or cell address)
    // Clipboard state
    copy_feedback: Option<(String, Instant)>, // Message and timestamp for copy feedback
    // Progress state
    progress: Option<ProgressInfo>, // Current operation progress
    // Theme state
    current_theme: Theme, // Current color theme
    excel_colors: bool,   // Show workbook colors instead of monochrome cells.
    // Config state
    config: crate::config::Config, // User configuration
}

impl TuiState {
    const LAZY_LOADING_THRESHOLD: usize = 1000; // Use lazy loading for sheets with >1000 rows
    const ROW_CACHE_SIZE: usize = 200; // Cache 200 rows at a time for lazy loading

    pub fn new(
        mut workbook: Workbook,
        initial_sheet_name: &str,
        config: &crate::config::Config,
        horizontal_scroll: bool,
    ) -> Result<Self> {
        workbook.enable_colors();
        let sheet_names = workbook.sheet_names();
        let current_sheet_index = sheet_names
            .iter()
            .position(|name| name == initial_sheet_name)
            .ok_or_else(|| anyhow::anyhow!("Sheet not found: {initial_sheet_name}"))?;

        // Load sheet lazily first to check size
        let lazy_data = workbook.load_sheet_lazy(&sheet_names[current_sheet_index])?;
        let sheet_height = lazy_data.height;

        // Choose loading strategy based on size
        let sheet_data = if sheet_height > Self::LAZY_LOADING_THRESHOLD {
            eprintln!(
                "📊 Large file detected ({} rows) - using lazy loading",
                sheet_height
            );
            SheetDataSource::Lazy {
                data: lazy_data,
                cache: None,
                cache_size: Self::ROW_CACHE_SIZE,
            }
        } else {
            // Convert to eager loading for small files
            SheetDataSource::Eager(lazy_data.to_sheet_data())
        };

        let mut state = Self {
            workbook,
            sheet_names,
            current_sheet_index,
            sheet_data,
            should_quit: false,
            cursor_row: 0,
            cursor_col: 0,
            scroll_offset: 0,
            horizontal_scroll_offset: 0,
            horizontal_scroll_enabled: horizontal_scroll,
            column_widths: Vec::new(),
            show_help: false,
            show_cell_detail: false,
            cell_detail_scroll: 0,
            search_mode: false,
            search_task: None,
            search_limited: false,
            search_complete: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            current_match_index: None,
            jump_mode: false,
            jump_input: String::new(),
            copy_feedback: None,
            progress: None,
            current_theme: Self::parse_theme_name(&config.theme.default),
            excel_colors: false,
            config: config.clone(),
        };

        // Calculate column widths if horizontal scrolling is enabled
        if horizontal_scroll {
            state.column_widths = state.calculate_column_widths();
        }

        Ok(state)
    }

    /// Parse theme name from config string
    fn parse_theme_name(name: &str) -> Theme {
        match name.to_lowercase().as_str() {
            "dracula" => Theme::Dracula,
            "solarized dark" | "solarizeddark" => Theme::SolarizedDark,
            "solarized light" | "solarizedlight" => Theme::SolarizedLight,
            "github dark" | "githubdark" => Theme::GitHubDark,
            "nord" => Theme::Nord,
            _ => Theme::Default, // Fallback to default for unknown themes
        }
    }

    fn current_sheet_name(&self) -> &str {
        &self.sheet_names[self.current_sheet_index]
    }

    fn switch_to_next_sheet(&mut self) -> Result<()> {
        if self.sheet_names.len() <= 1 {
            return Ok(()); // No other sheets to switch to
        }

        self.current_sheet_index = (self.current_sheet_index + 1) % self.sheet_names.len();
        self.load_current_sheet()?;
        self.reset_cursor();
        self.clear_search(); // Clear search when changing sheets
        Ok(())
    }

    fn switch_to_prev_sheet(&mut self) -> Result<()> {
        if self.sheet_names.len() <= 1 {
            return Ok(()); // No other sheets to switch to
        }

        self.current_sheet_index = if self.current_sheet_index == 0 {
            self.sheet_names.len() - 1
        } else {
            self.current_sheet_index - 1
        };
        self.load_current_sheet()?;
        self.reset_cursor();
        self.clear_search(); // Clear search when changing sheets
        Ok(())
    }

    fn load_current_sheet(&mut self) -> Result<()> {
        let sheet_name = self.sheet_names[self.current_sheet_index].clone();

        // Load sheet lazily first to check size
        let lazy_data = self.workbook.load_sheet_lazy(&sheet_name)?;
        let sheet_height = lazy_data.height;

        // Choose loading strategy based on size
        self.sheet_data = if sheet_height > Self::LAZY_LOADING_THRESHOLD {
            eprintln!(
                "📊 Large file detected ({} rows) - using lazy loading",
                sheet_height
            );
            SheetDataSource::Lazy {
                data: lazy_data,
                cache: None,
                cache_size: Self::ROW_CACHE_SIZE,
            }
        } else {
            // Convert to eager loading for small files
            SheetDataSource::Eager(lazy_data.to_sheet_data())
        };

        // Recalculate column widths if horizontal scrolling is enabled
        if self.horizontal_scroll_enabled {
            self.column_widths = self.calculate_column_widths();
        }

        Ok(())
    }

    fn reset_cursor(&mut self) {
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.scroll_offset = 0;
        self.horizontal_scroll_offset = 0;
    }

    /// Compile once on Enter; advance cooperatively between input events.
    fn perform_search(&mut self) {
        self.search_matches.clear();
        self.current_match_index = None;
        self.search_limited = false;
        self.search_complete = false;
        self.progress = None;
        self.search_task = None;
        if self.search_query.is_empty() {
            return;
        }
        match crate::search::Search::new(&self.search_query) {
            Ok(job) => {
                self.search_task = Some(job);
                self.progress = Some(ProgressInfo::new(
                    "Searching (Esc cancel)",
                    self.sheet_data.height(),
                ));
            }
            Err(error) => {
                self.copy_feedback = Some((format!("Search: {error}"), Instant::now()));
            }
        }
    }
    fn tick_search(&mut self) {
        let Some(mut job) = self.search_task.take() else {
            return;
        };
        let begin = Instant::now();
        let mut cells = 0usize;
        let width = self.sheet_data.width();
        let height = self.sheet_data.height();
        while width > 0
            && job.row < height
            && cells < crate::search::CELLS_PER_TICK
            && begin.elapsed() < Duration::from_millis(8)
        {
            let (rows, _) = self.sheet_data.get_rows(job.row, 1);
            if let Some(value) = rows.first().and_then(|r| r.get(job.col)) {
                let text = match value {
                    CellValue::String(s) => std::borrow::Cow::Borrowed(s.as_str()),
                    v => std::borrow::Cow::Owned(v.to_string()),
                };
                if job.matches(&text) {
                    self.search_matches.push((job.row, job.col));
                }
            }
            job.advance(width);
            cells += 1;
            if self.search_matches.len() == crate::search::MAX_RESULTS {
                self.search_limited = true;
                break;
            }
        }
        if let Some(progress) = self.progress.as_mut() {
            progress.update(job.row);
        }
        if self.search_limited || job.row >= height || width == 0 {
            self.search_complete = true;
            self.progress = None;
        } else {
            self.search_task = Some(job);
        }
        if self.current_match_index.is_none() && !self.search_matches.is_empty() {
            self.current_match_index = Some(0);
            self.jump_to_current_match();
        }
    }

    /// Jump to the next search match
    fn jump_to_next_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }

        self.current_match_index = Some(match self.current_match_index {
            Some(idx) => (idx + 1) % self.search_matches.len(),
            None => 0,
        });

        self.jump_to_current_match();
    }

    /// Jump to the previous search match
    fn jump_to_prev_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }

        self.current_match_index = Some(match self.current_match_index {
            Some(idx) => {
                if idx == 0 {
                    self.search_matches.len() - 1
                } else {
                    idx - 1
                }
            }
            None => self.search_matches.len() - 1,
        });

        self.jump_to_current_match();
    }

    /// Move cursor to the current search match
    fn jump_to_current_match(&mut self) {
        if let Some(idx) = self.current_match_index
            && let Some(&(row, col)) = self.search_matches.get(idx)
        {
            self.cursor_row = row;
            self.cursor_col = col;
        }
    }

    /// Clear search state
    fn clear_search(&mut self) {
        self.search_task = None;
        self.search_limited = false;
        self.search_complete = false;
        self.progress = None;
        self.search_query.clear();
        self.search_matches.clear();
        self.current_match_index = None;
    }

    /// Enter jump mode
    fn enter_jump_mode(&mut self) {
        self.jump_mode = true;
        self.jump_input.clear();
    }

    /// Parse jump input and navigate to that location
    /// Supports formats: "100" (row), "A5" (cell address), "5,3" (row,col)
    fn perform_jump(&mut self) {
        let input = self.jump_input.trim();
        let (origin_row, origin_col) = self.sheet_data.data_start();
        let position = if let Ok(row) = input.parse::<usize>() {
            row.checked_sub(1)
                .map(|r| (origin_col + self.cursor_col, r))
        } else if let Some((row, col)) = input.split_once(',') {
            row.trim()
                .parse::<usize>()
                .ok()
                .and_then(|r| r.checked_sub(1))
                .zip(
                    col.trim()
                        .parse::<usize>()
                        .ok()
                        .and_then(|c| c.checked_sub(1)),
                )
                .map(|(r, c)| (c, r))
        } else {
            Self::parse_cell_address(input)
        };
        let relative =
            position.and_then(|(c, r)| r.checked_sub(origin_row).zip(c.checked_sub(origin_col)));
        if let Some((row, col)) =
            relative.filter(|(r, c)| *r < self.sheet_data.height() && *c < self.sheet_data.width())
        {
            self.cursor_row = row;
            self.cursor_col = col;
            self.copy_feedback = Some((
                format!("Jumped to {}", self.current_cell_address()),
                Instant::now(),
            ));
        } else {
            self.copy_feedback = Some(("Cell address out of bounds".to_string(), Instant::now()));
        }
        self.jump_mode = false;
        self.jump_input.clear();
    }
    fn parse_cell_address(addr: &str) -> Option<(usize, usize)> {
        let split = addr.bytes().position(|b| !b.is_ascii_alphabetic())?;
        if split == 0 {
            return None;
        }
        let (letters, digits) = addr.split_at(split);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let mut col = 0usize;
        for letter in letters.bytes() {
            col = col
                .checked_mul(26)?
                .checked_add(usize::from(letter.to_ascii_uppercase() - b'A' + 1))?;
        }
        Some((
            col.checked_sub(1)?,
            digits.parse::<usize>().ok()?.checked_sub(1)?,
        ))
    }

    /// Copy the current cell value to clipboard
    fn copy_current_cell(&mut self) {
        let (cell, _formula) = self.sheet_data.get_cell(self.cursor_row, self.cursor_col);
        let cell_value = cell.map(|v| v.to_raw_string()).unwrap_or_default();

        match Clipboard::new() {
            Ok(mut clipboard) => {
                if let Err(e) = clipboard.set_text(&cell_value) {
                    self.copy_feedback = Some((format!("Copy failed: {}", e), Instant::now()));
                } else {
                    let cell_addr = self.current_cell_address();
                    self.copy_feedback =
                        Some((format!("Copied cell {}", cell_addr), Instant::now()));
                }
            }
            Err(e) => {
                self.copy_feedback = Some((format!("Clipboard error: {}", e), Instant::now()));
            }
        }
    }

    /// Copy the current row to clipboard (tab-separated)
    fn copy_current_row(&mut self) {
        let (rows, _formulas) = self.sheet_data.get_rows(self.cursor_row, 1);
        let row_values = rows
            .first()
            .map(|row| {
                row.iter()
                    .map(|cell| {
                        let value = cell.to_raw_string();
                        // Escape cells that contain tabs, newlines, or quotes
                        if value.contains('\t') || value.contains('\n') || value.contains('"') {
                            format!("\"{}\"", value.replace('"', "\"\""))
                        } else {
                            value
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .unwrap_or_default();

        match Clipboard::new() {
            Ok(mut clipboard) => {
                if let Err(e) = clipboard.set_text(&row_values) {
                    self.copy_feedback = Some((format!("Copy failed: {}", e), Instant::now()));
                } else {
                    self.copy_feedback = Some((
                        format!(
                            "Copied row {} ({} cells)",
                            self.cursor_row + 1,
                            self.sheet_data.width()
                        ),
                        Instant::now(),
                    ));
                }
            }
            Err(e) => {
                self.copy_feedback = Some((format!("Clipboard error: {}", e), Instant::now()));
            }
        }
    }

    fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            // Auto-scroll up if cursor moves above viewport
            if self.cursor_row < self.scroll_offset {
                self.scroll_offset = self.cursor_row;
            }
        }
    }

    fn move_down(&mut self) {
        if self.cursor_row < self.sheet_data.height().saturating_sub(1) {
            self.cursor_row += 1;
            // Auto-scroll down will be handled in render based on viewport height
        }
    }

    /// Update scroll offset to keep cursor visible
    fn update_scroll(&mut self, viewport_height: usize) {
        // Scroll down if cursor is below visible area
        if self.cursor_row >= self.scroll_offset + viewport_height {
            self.scroll_offset = self
                .cursor_row
                .saturating_sub(viewport_height.saturating_sub(1));
        }
        // Scroll up if cursor is above visible area
        if self.cursor_row < self.scroll_offset {
            self.scroll_offset = self.cursor_row;
        }
    }

    /// Calculate column widths based on content
    fn calculate_column_widths(&mut self) -> Vec<usize> {
        let num_cols = self.sheet_data.width();
        let mut widths = vec![0; num_cols];

        // Measure headers
        let headers = self.sheet_data.headers();
        for (i, header) in headers.iter().enumerate() {
            widths[i] = header.len();
        }

        // Sample first 100 rows (or fewer if sheet is smaller)
        let sample_size = 100.min(self.sheet_data.height());
        let (sample_rows, _) = self.sheet_data.get_rows(0, sample_size);

        for row in sample_rows.iter() {
            for (col_idx, cell) in row.iter().enumerate() {
                let len = cell.to_string().len();
                widths[col_idx] = widths[col_idx].max(len);
            }
        }

        // Apply constraints: min 3 chars, max 30 chars
        widths.iter().map(|&w| w.clamp(3, 30)).collect()
    }

    /// Update horizontal scroll offset to keep cursor visible
    fn update_horizontal_scroll(&mut self, viewport_width: usize) {
        if !self.horizontal_scroll_enabled {
            return;
        }

        // Calculate which columns are visible
        let mut total_width = 0;
        let mut visible_end = self.horizontal_scroll_offset;

        for i in self.horizontal_scroll_offset..self.column_widths.len() {
            total_width += self.column_widths[i] + 1; // +1 for separator
            visible_end = i + 1; // Always include current column
            if total_width > viewport_width {
                break; // Break after including partially-visible column
            }
        }

        // Scroll right if cursor is beyond visible area
        if self.cursor_col >= visible_end {
            self.horizontal_scroll_offset += 1;
        }

        // Scroll left if cursor is before visible area
        if self.cursor_col < self.horizontal_scroll_offset {
            self.horizontal_scroll_offset = self.cursor_col;
        }
    }

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
            // Auto-scroll left if cursor moves before visible area
            if self.horizontal_scroll_enabled && self.cursor_col < self.horizontal_scroll_offset {
                self.horizontal_scroll_offset = self.cursor_col;
            }
        }
    }

    fn move_right(&mut self) {
        if self.cursor_col < self.sheet_data.width().saturating_sub(1) {
            self.cursor_col += 1;
            // Auto-scroll right will be handled in render based on viewport width
        }
    }

    fn move_to_start_of_row(&mut self) {
        self.cursor_col = 0;
        if self.horizontal_scroll_enabled {
            self.horizontal_scroll_offset = 0;
        }
    }

    fn move_to_end_of_row(&mut self) {
        self.cursor_col = self.sheet_data.width().saturating_sub(1);
        // Horizontal scroll will be updated in render to show the last column
    }

    fn page_up(&mut self, page_size: usize) {
        self.cursor_row = self.cursor_row.saturating_sub(page_size);
    }

    fn page_down(&mut self, page_size: usize) {
        self.cursor_row =
            (self.cursor_row + page_size).min(self.sheet_data.height().saturating_sub(1));
    }

    fn move_to_top(&mut self) {
        self.cursor_row = 0;
    }

    fn move_to_bottom(&mut self) {
        self.cursor_row = self.sheet_data.height().saturating_sub(1);
    }

    fn col_to_letter(&self, col: usize) -> String {
        let mut result = String::new();
        let mut n = col + 1;
        while n > 0 {
            n -= 1;
            result.push((b'A' + (n % 26) as u8) as char);
            n /= 26;
        }
        result.chars().rev().collect()
    }

    fn current_cell_address(&self) -> String {
        let (row, col) = self.sheet_data.data_start();
        format!(
            "{}{}",
            self.col_to_letter(col + self.cursor_col),
            row + self.cursor_row + 1
        )
    }

    /// Check if a key press matches a configured action
    fn key_matches(
        &self,
        code: KeyCode,
        modifiers: crossterm::event::KeyModifiers,
        action: &str,
    ) -> bool {
        if let Some((expected_code, expected_mods)) = self.config.get_keybinding(action) {
            if code != expected_code {
                return false;
            }
            // Some terminals omit SHIFT for shifted symbol characters (e.g. '?', '$')
            // because the shift is already encoded in the character itself. Strip SHIFT
            // from both sides for non-alphabetic chars so bindings work consistently.
            if let KeyCode::Char(c) = code
                && !c.is_alphabetic()
            {
                let strip = crossterm::event::KeyModifiers::SHIFT;
                return (modifiers - strip) == (expected_mods - strip);
            }
            modifiers == expected_mods
        } else {
            false
        }
    }

    fn handle_event(&mut self, event: Event) {
        if let Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            ..
        }) = event
        {
            // If help is showing, any key closes it
            if self.show_help {
                self.show_help = false;
                return;
            }

            // If cell detail is showing, handle scrolling or close
            if self.show_cell_detail {
                match code {
                    KeyCode::Up => {
                        self.cell_detail_scroll = self.cell_detail_scroll.saturating_sub(1);
                    }
                    KeyCode::Down => {
                        self.cell_detail_scroll = self.cell_detail_scroll.saturating_add(1);
                    }
                    KeyCode::PageUp => {
                        self.cell_detail_scroll = self.cell_detail_scroll.saturating_sub(10);
                    }
                    KeyCode::PageDown => {
                        self.cell_detail_scroll = self.cell_detail_scroll.saturating_add(10);
                    }
                    KeyCode::Home => {
                        self.cell_detail_scroll = 0;
                    }
                    _ => {
                        // Any other key closes the detail view
                        self.show_cell_detail = false;
                        self.cell_detail_scroll = 0;
                    }
                }
                return;
            }

            // If in search mode, handle search input
            if self.search_mode {
                match code {
                    KeyCode::Char(c) => {
                        if self.search_query.len() + c.len_utf8() <= crate::search::MAX_QUERY_BYTES
                        {
                            self.search_query.push(c);
                        } else {
                            self.copy_feedback = Some((
                                "Search query limited to 4096 UTF-8 bytes".into(),
                                Instant::now(),
                            ));
                        }
                    }
                    KeyCode::Backspace => {
                        self.search_query.pop();
                    }
                    KeyCode::Enter => {
                        self.search_mode = false;
                        self.perform_search();
                    }
                    KeyCode::Esc => {
                        // Exit search mode and clear search
                        self.search_mode = false;
                        self.clear_search();
                    }
                    _ => {}
                }
                return;
            }

            // If in jump mode, handle jump input
            if self.jump_mode {
                match code {
                    KeyCode::Char(c) => {
                        self.jump_input.push(c);
                    }
                    KeyCode::Backspace => {
                        self.jump_input.pop();
                    }
                    KeyCode::Enter => {
                        // Perform jump
                        self.perform_jump();
                    }
                    KeyCode::Esc => {
                        // Exit jump mode
                        self.jump_mode = false;
                        self.jump_input.clear();
                    }
                    _ => {}
                }
                return;
            }

            // Normal navigation and commands - using configured keybindings
            // Check actions in order of priority
            if self.key_matches(code, modifiers, "quit") {
                self.should_quit = true;
            } else if self.key_matches(code, modifiers, "help") {
                self.show_help = true;
            } else if self.key_matches(code, modifiers, "theme_toggle")
                || (code == KeyCode::Char('T')
                    && (modifiers - crossterm::event::KeyModifiers::SHIFT).is_empty())
            {
                self.excel_colors = !self.excel_colors;
            } else if self.key_matches(code, modifiers, "search") {
                self.search_mode = true;
                self.clear_search();
            } else if self.key_matches(code, modifiers, "next_match") {
                self.jump_to_next_match();
            } else if self.key_matches(code, modifiers, "prev_match") {
                self.jump_to_prev_match();
            } else if self.key_matches(code, modifiers, "copy_cell") {
                self.copy_current_cell();
            } else if self.key_matches(code, modifiers, "copy_row") {
                self.copy_current_row();
            } else if self.key_matches(code, modifiers, "jump") {
                self.enter_jump_mode();
            } else if self.key_matches(code, modifiers, "show_cell_detail") {
                self.show_cell_detail = true;
                self.cell_detail_scroll = 0;
            } else if self.key_matches(code, modifiers, "next_sheet") {
                let _ = self.switch_to_next_sheet();
            } else if self.key_matches(code, modifiers, "prev_sheet") || code == KeyCode::BackTab {
                // BackTab is another way to detect Shift+Tab on some terminals
                let _ = self.switch_to_prev_sheet();
            } else if self.key_matches(code, modifiers, "up") {
                self.move_up();
            } else if self.key_matches(code, modifiers, "down") {
                self.move_down();
            } else if self.key_matches(code, modifiers, "left") {
                self.move_left();
            } else if self.key_matches(code, modifiers, "right") {
                self.move_right();
            } else if self.key_matches(code, modifiers, "jump_to_top") {
                self.move_to_top();
            } else if self.key_matches(code, modifiers, "jump_to_bottom") {
                self.move_to_bottom();
            } else if self.key_matches(code, modifiers, "jump_to_row_start") {
                self.move_to_start_of_row();
            } else if self.key_matches(code, modifiers, "jump_to_row_end") {
                self.move_to_end_of_row();
            } else if self.key_matches(code, modifiers, "page_up") {
                self.page_up(10);
            } else if self.key_matches(code, modifiers, "page_down") {
                self.page_down(10);
            } else if code == KeyCode::Esc {
                // Special handling for Esc - clear search if active, otherwise quit
                if self.search_task.is_some() || !self.search_query.is_empty() {
                    self.clear_search();
                } else {
                    self.should_quit = true;
                }
            }
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        frame.render_widget(
            Block::default().style(Style::default().fg(Color::White).bg(Color::Black)),
            frame.area(),
        );
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),    // Main content
                Constraint::Length(3), // Status bar
            ])
            .split(frame.area());

        // Calculate visible viewport
        // Each visible row has a content line and a grid separator.
        let table_height = (chunks[0].height.saturating_sub(4) / 2) as usize;
        let viewport_width = chunks[0].width.saturating_sub(2) as usize; // Account for borders

        // Update scroll to keep cursor visible
        self.update_scroll(table_height);
        self.update_horizontal_scroll(viewport_width);

        let visible_start = self.scroll_offset;

        // Calculate visible column range
        let (visible_col_start, visible_col_end) = if self.horizontal_scroll_enabled {
            // Calculate which columns fit in viewport
            let mut total_width = 0;
            let mut end = self.horizontal_scroll_offset;

            for i in self.horizontal_scroll_offset..self.column_widths.len() {
                total_width += self.column_widths[i] + 1; // +1 for separator
                end = i + 1; // Always include current column
                if total_width > viewport_width {
                    break; // Break after including partially-visible column
                }
            }
            (self.horizontal_scroll_offset, end)
        } else {
            (0, self.sheet_data.width())
        };

        // Clone headers to avoid borrow issues
        let headers = self.sheet_data.headers().to_vec();

        // Get theme colors
        let colors = self.current_theme.colors();
        let cell_base = if self.excel_colors {
            Style::default()
                .fg(Color::Rgb(0, 0, 0))
                .bg(Color::Rgb(255, 255, 255))
        } else {
            Style::default().fg(Color::White).bg(Color::Black)
        };

        let origin = self.sheet_data.data_start();

        // Build table rows with highlighting
        let header_cells: Vec<Cell> = headers
            .iter()
            .enumerate()
            .skip(visible_col_start)
            .take(visible_col_end - visible_col_start)
            .map(|(col_idx, h)| {
                let mut style = cell_base.add_modifier(Modifier::BOLD);

                let source = (self.excel_colors && self.workbook.uses_headers())
                    .then(|| origin.0.checked_sub(1))
                    .flatten()
                    .and_then(|r| self.workbook.color_at(r, origin.1 + col_idx));
                if let Some(c) = source {
                    style = c.apply(style);
                }
                // Keep the original header color; underline indicates the active column.
                if col_idx == self.cursor_col {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }

                Cell::from(h.as_str()).style(style)
            })
            .collect();

        let header = Row::new(header_cells).height(1).bottom_margin(1);

        // Get visible rows from data source (handles lazy loading if needed)
        let (visible_rows, _visible_formulas) =
            self.sheet_data.get_rows(visible_start, table_height);

        let data_rows: Vec<Row> = visible_rows
            .iter()
            .enumerate()
            .map(|(visible_idx, row)| {
                let row_idx = visible_start + visible_idx; // Absolute row index
                let cells: Vec<Cell> = row
                    .iter()
                    .enumerate()
                    .skip(visible_col_start)
                    .take(visible_col_end - visible_col_start)
                    .map(|(col_idx, cell)| {
                        let mut style = cell_base;
                        let source = self
                            .excel_colors
                            .then(|| {
                                self.workbook
                                    .color_at(origin.0 + row_idx, origin.1 + col_idx)
                            })
                            .flatten();
                        if let Some(c) = source {
                            style = c.apply(style);
                        }
                        // Check if this cell is a search match
                        let is_search_match = self
                            .search_matches
                            .binary_search(&(row_idx, col_idx))
                            .is_ok();
                        let is_current_match = self
                            .current_match_index
                            .and_then(|idx| self.search_matches.get(idx))
                            .map(|&pos| pos == (row_idx, col_idx))
                            .unwrap_or(false);

                        // Highlight current search match (highest priority)
                        if is_current_match {
                            style = style
                                .bg(colors.current_search_bg)
                                .fg(colors.current_search_fg)
                                .add_modifier(Modifier::BOLD);
                        }
                        // Highlight current cell
                        else if row_idx == self.cursor_row && col_idx == self.cursor_col {
                            style = style
                                .bg(colors.current_cell_bg)
                                .fg(colors.current_cell_fg)
                                .add_modifier(Modifier::BOLD);
                        }
                        // Highlight other search matches
                        else if is_search_match {
                            style = style.bg(colors.search_match_bg).fg(colors.search_match_fg);
                        }
                        Cell::from(cell.to_string()).style(style)
                    })
                    .collect();
                Row::new(cells).height(1).bottom_margin(1)
            })
            .collect();

        // Calculate column widths
        let col_widths: Vec<Constraint> = if self.horizontal_scroll_enabled {
            // Use fixed widths based on content
            self.column_widths[visible_col_start..visible_col_end]
                .iter()
                .map(|&w| Constraint::Length(w as u16))
                .collect()
        } else {
            // Use percentage-based widths (current behavior)
            let sheet_width = self.sheet_data.width();
            headers
                .iter()
                .map(|_| Constraint::Percentage((100 / sheet_width.max(1)) as u16))
                .collect()
        };

        let table_title = if self.sheet_names.len() > 1 {
            format!(
                " {} (Sheet {}/{}) ",
                self.current_sheet_name(),
                self.current_sheet_index + 1,
                self.sheet_names.len()
            )
        } else {
            format!(" {} ", self.current_sheet_name())
        };

        let visible_row_count = data_rows.len();
        let ui_style = Style::default().fg(Color::White).bg(Color::Black);
        let grid_columns = Layout::horizontal(col_widths.clone())
            .spacing(1)
            .split(Rect::new(
                chunks[0].x.saturating_add(1),
                chunks[0].y.saturating_add(1),
                chunks[0].width.saturating_sub(2),
                chunks[0].height.saturating_sub(2),
            ));
        let table = Table::new(data_rows, col_widths)
            .style(ui_style)
            .header(header)
            .block(
                Block::default()
                    .style(ui_style)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(colors.border_fg))
                    .title(table_title),
            );

        frame.render_widget(table, chunks[0]);
        // Grid lines are viewer chrome, not Excel border-style reproduction.
        // Paint only reserved spacing, never cell content or the outer frame.
        if let (Some(first), Some(last)) = (grid_columns.first(), grid_columns.last()) {
            let right = last.right().min(chunks[0].right().saturating_sub(1));
            let bottom = first
                .y
                .saturating_add(((visible_row_count + 1) as u16).saturating_mul(2))
                .min(chunks[0].bottom().saturating_sub(1));
            let grid_style = Style::default().fg(Color::DarkGray).bg(Color::Black);
            let buf = frame.buffer_mut();
            for y in first.y..bottom {
                let horizontal = (y - first.y) % 2 == 1;
                if horizontal {
                    for x in first.x..right {
                        buf[(x, y)].set_symbol("─").set_style(grid_style);
                    }
                }
                for col in grid_columns
                    .iter()
                    .take(grid_columns.len().saturating_sub(1))
                {
                    let x = col.right();
                    if x < right {
                        buf[(x, y)]
                            .set_symbol(if horizontal { "┼" } else { "│" })
                            .set_style(grid_style);
                    }
                }
            }
        }

        // Status bar with current cell info
        let (cell, _) = self.sheet_data.get_cell(self.cursor_row, self.cursor_col);
        let current_cell_value = cell.map(|v| v.to_string()).unwrap_or_default();

        // Format sheet dimensions with scroll indicator
        let sheet_dims = if self.horizontal_scroll_enabled && self.horizontal_scroll_offset > 0 {
            let first_col = headers
                .get(visible_col_start)
                .map(|s| s.as_str())
                .unwrap_or("?");
            let last_col = headers
                .get(visible_col_end.saturating_sub(1))
                .map(|s| s.as_str())
                .unwrap_or("?");
            format!(
                "{} rows × {} columns (showing {}-{})",
                self.sheet_data.height(),
                self.sheet_data.width(),
                first_col,
                last_col
            )
        } else {
            format!(
                "{} rows × {} columns",
                self.sheet_data.height(),
                self.sheet_data.width()
            )
        };

        let status_text = if let Some(ref progress) = self.progress {
            // Show progress indicator
            format!(" ⏳ {} ", progress.format())
        } else if self.jump_mode {
            format!(
                " Jump to (row, cell like A5, or row,col): {} ",
                self.jump_input
            )
        } else if self.search_mode {
            format!(
                " Search (Enter to start, Esc cancel): {} ",
                self.search_query
            )
        } else if let Some(idx) = self.current_match_index {
            // Show search results
            let match_info = format!(
                "Match {}/{}{} | ",
                idx + 1,
                self.search_matches.len(),
                if self.search_limited {
                    " [result limit reached; refine search]"
                } else {
                    ""
                }
            );
            if self.sheet_names.len() > 1 {
                format!(
                    " {} | {}n:next N:prev Esc:clear | {} | Tab:next sheet ?:help q:quit ",
                    match_info,
                    self.current_cell_address(),
                    sheet_dims
                )
            } else {
                format!(
                    " {} | {}n:next N:prev Esc:clear | {} | ?:help q:quit ",
                    match_info,
                    self.current_cell_address(),
                    sheet_dims
                )
            }
        } else if self.search_complete && self.search_matches.is_empty() {
            format!(" No matches for {} | Esc:clear q:quit ", self.search_query)
        } else {
            // Show loading mode indicator for large files
            let mode_indicator = match &self.sheet_data {
                SheetDataSource::Lazy { .. } => " [Lazy] ",
                SheetDataSource::Eager(_) => "",
            };

            if self.sheet_names.len() > 1 {
                format!(
                    " {} | {}{} | T:{} /:search Tab:sheet ?:help q:quit ",
                    self.current_cell_address(),
                    sheet_dims,
                    mode_indicator,
                    if self.excel_colors {
                        "Black/white"
                    } else {
                        "Excel colors"
                    }
                )
            } else {
                format!(
                    " {} | {}{} | T:{} /:search ?:help q:quit ",
                    self.current_cell_address(),
                    sheet_dims,
                    mode_indicator,
                    if self.excel_colors {
                        "Black/white"
                    } else {
                        "Excel colors"
                    }
                )
            }
        };

        let mut status_style = Style::default().fg(colors.status_bar_fg);
        if let Some(bg) = colors.status_bar_bg {
            status_style = status_style.bg(bg);
        }

        let status = Paragraph::new(status_text).style(status_style).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(colors.border_fg))
                .title(format!(" {} ", current_cell_value)),
        );

        frame.render_widget(status, chunks[1]);

        // Render cell detail overlay if visible
        if self.show_cell_detail {
            self.render_cell_detail(frame);
        }

        // Render help overlay if visible
        if self.show_help {
            self.render_help(frame);
        }

        // Render copy feedback if active (and not expired)
        if let Some((ref message, timestamp)) = self.copy_feedback {
            // Show feedback for 2 seconds
            if timestamp.elapsed() < Duration::from_secs(2) {
                self.render_copy_feedback(frame, message);
            } else {
                // Clear expired feedback
                self.copy_feedback = None;
            }
        }
    }

    fn render_help(&self, frame: &mut Frame) {
        use ratatui::text::{Line, Span};

        // Build help content with rich formatting
        let help_lines = vec![
            Line::from(vec![
                Span::styled(
                    "Toss",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" - Interactive Excel Viewer"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "NAVIGATION",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("  ↑ ↓ ← →          ", Style::default().fg(Color::Green)),
                Span::raw("Move cursor one cell"),
            ]),
            Line::from(vec![
                Span::styled("  Page Up/Down     ", Style::default().fg(Color::Green)),
                Span::raw("Scroll 10 rows"),
            ]),
            Line::from(vec![
                Span::styled("  Home             ", Style::default().fg(Color::Green)),
                Span::raw("Jump to first column (start of row)"),
            ]),
            Line::from(vec![
                Span::styled("  End              ", Style::default().fg(Color::Green)),
                Span::raw("Jump to last column (end of row)"),
            ]),
            Line::from(vec![
                Span::styled("  Ctrl+Home        ", Style::default().fg(Color::Green)),
                Span::raw("Jump to first row (top of sheet)"),
            ]),
            Line::from(vec![
                Span::styled("  Ctrl+End         ", Style::default().fg(Color::Green)),
                Span::raw("Jump to last row (bottom of sheet)"),
            ]),
            Line::from(vec![
                Span::styled("  Ctrl+G           ", Style::default().fg(Color::Green)),
                Span::raw("Jump to row/cell (e.g., 100, A5, or 10,3)"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "SEARCH",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("  /                ", Style::default().fg(Color::Green)),
                Span::raw("Start search (type query, Enter to confirm)"),
            ]),
            Line::from(vec![
                Span::styled("  n                ", Style::default().fg(Color::Green)),
                Span::raw("Jump to next search match"),
            ]),
            Line::from(vec![
                Span::styled("  N (Shift+n)      ", Style::default().fg(Color::Green)),
                Span::raw("Jump to previous search match"),
            ]),
            Line::from(vec![
                Span::styled("  Esc              ", Style::default().fg(Color::Green)),
                Span::raw("Clear search results"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "CLIPBOARD",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("  c                ", Style::default().fg(Color::Green)),
                Span::raw("Copy current cell value"),
            ]),
            Line::from(vec![
                Span::styled("  C (Shift+c)      ", Style::default().fg(Color::Green)),
                Span::raw("Copy entire current row (tab-separated)"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "SHEET NAVIGATION",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("  Tab              ", Style::default().fg(Color::Green)),
                Span::raw("Switch to next sheet"),
            ]),
            Line::from(vec![
                Span::styled("  Shift+Tab        ", Style::default().fg(Color::Green)),
                Span::raw("Switch to previous sheet"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "GENERAL",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled("  Enter            ", Style::default().fg(Color::Green)),
                Span::raw("Show cell details (type, formula, value)"),
            ]),
            Line::from(vec![
                Span::styled("  t                ", Style::default().fg(Color::Green)),
                Span::raw("Toggle black/white and original Excel colors"),
            ]),
            Line::from(vec![
                Span::styled("  ?                ", Style::default().fg(Color::Green)),
                Span::raw("Toggle this help screen"),
            ]),
            Line::from(vec![
                Span::styled("  q                ", Style::default().fg(Color::Green)),
                Span::raw("Quit Toss"),
            ]),
            Line::from(vec![
                Span::styled("  Esc              ", Style::default().fg(Color::Green)),
                Span::raw("Quit Toss (or clear search)"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "VISUAL CUES",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled(
                    "  Blue background  ",
                    Style::default().bg(Color::Blue).fg(Color::White),
                ),
                Span::raw("  Current cell (selected)"),
            ]),
            Line::from(vec![
                Span::styled("  Dark gray bg     ", Style::default().bg(Color::DarkGray)),
                Span::raw("  Current row highlight"),
            ]),
            Line::from(vec![
                Span::styled("  Cyan text        ", Style::default().fg(Color::Cyan)),
                Span::raw("  Current column highlight"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  Yellow bold      ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  Column headers"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  Yellow bg        ",
                    Style::default().bg(Color::Yellow).fg(Color::Black),
                ),
                Span::raw("  Current search match"),
            ]),
            Line::from(vec![
                Span::styled(
                    "  Light yellow bg  ",
                    Style::default().bg(Color::LightYellow).fg(Color::Black),
                ),
                Span::raw("  Other search matches"),
            ]),
            Line::from(""),
            Line::from("  Cells start with white text on a black background."),
            Line::from("  Press 't' for original Excel colors (default black on white)."),
            Line::from("  Cursor and search highlights remain visible in both modes."),
            Line::from(""),
            Line::from(Span::styled(
                "STATUS BAR INFO",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("  Cell address (e.g., B7) shown in bottom left"),
            Line::from("  Current cell value displayed in status bar title"),
            Line::from("  Sheet dimensions (rows × columns) shown"),
            Line::from("  Match counter shown when searching (e.g., Match 3/12)"),
            Line::from(""),
            Line::from(vec![Span::styled(
                "Press any key to close",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::ITALIC),
            )]),
        ];

        // Calculate popup size (centered, 70% width, auto height)
        let area = frame.area();
        let popup_width = (area.width as f32 * 0.7).min(80.0) as u16;
        let popup_height =
            (help_lines.len() + 4).min(area.height.saturating_sub(2) as usize) as u16;

        let popup_area = Rect {
            x: (area.width.saturating_sub(popup_width)) / 2,
            y: (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        // Clear the area behind the popup
        frame.render_widget(Clear, popup_area);

        // Create help content with styled text
        let help_paragraph = Paragraph::new(help_lines)
            .style(Style::default().fg(Color::White).bg(Color::Black))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                    .title(vec![
                        Span::raw(" "),
                        Span::styled(
                            "Help",
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(" - Keyboard Shortcuts "),
                    ])
                    .title_alignment(Alignment::Center),
            )
            .wrap(Wrap { trim: false });

        frame.render_widget(help_paragraph, popup_area);
    }

    fn render_cell_detail(&mut self, frame: &mut Frame) {
        use ratatui::text::{Line, Span};

        // Get current cell info
        let (cell_value, cell_formula) = self.sheet_data.get_cell(self.cursor_row, self.cursor_col);

        let cell_addr = self.current_cell_address();
        let header = self
            .sheet_data
            .headers()
            .get(self.cursor_col)
            .map(|s| s.as_str())
            .unwrap_or("");

        // Build detail lines
        let mut detail_lines = vec![
            Line::from(vec![
                Span::styled(
                    "Cell: ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(cell_addr.clone(), Style::default().fg(Color::Cyan)),
            ]),
            Line::from(vec![
                Span::styled(
                    "Column: ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(header),
            ]),
            Line::from(""),
        ];

        // Show formula first if it exists (more important than type)
        if let Some(ref formula) = cell_formula {
            detail_lines.push(Line::from(vec![
                Span::styled(
                    "Formula: ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    formula.clone(),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            detail_lines.push(Line::from(""));
        }

        if let Some(cell) = cell_value {
            // Cell type
            let cell_type = match cell {
                crate::workbook::CellValue::Empty => "Empty",
                crate::workbook::CellValue::String(_) => "String",
                crate::workbook::CellValue::Int(_) => "Integer",
                crate::workbook::CellValue::Float(_) => "Float",
                crate::workbook::CellValue::Bool(_) => "Boolean",
                crate::workbook::CellValue::Error(_) => "Error",
                crate::workbook::CellValue::DateTime(_) => "DateTime",
            };

            detail_lines.push(Line::from(vec![
                Span::styled(
                    "Type: ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(cell_type, Style::default().fg(Color::Green)),
            ]));

            // Raw value (unformatted)
            let raw_value = cell.to_raw_string();

            // If cell is empty but has a formula, add explanation
            if raw_value.is_empty() && cell_formula.is_some() {
                detail_lines.push(Line::from(vec![
                    Span::styled(
                        "Value: ",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "(empty - formula not evaluated)",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]));
            } else {
                let value_display = if raw_value.is_empty() {
                    "(empty)".to_string()
                } else {
                    raw_value.clone()
                };
                detail_lines.push(Line::from(vec![
                    Span::styled(
                        "Value: ",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(value_display),
                ]));
            }

            // Formatted display value
            let display_value = cell.to_string();
            if display_value != raw_value {
                detail_lines.push(Line::from(vec![
                    Span::styled(
                        "Display Value: ",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(display_value.clone()),
                ]));
            }

            detail_lines.push(Line::from(""));
            detail_lines.push(Line::from(Span::styled(
                "Full Content:",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            detail_lines.push(Line::from(""));

            // Split content by lines for multi-line display
            for line in raw_value.lines() {
                detail_lines.push(Line::from(Span::raw(line.to_string())));
            }
        } else {
            // No cell value - might be a formula cell or truly empty
            if cell_formula.is_some() {
                detail_lines.push(Line::from(vec![
                    Span::styled(
                        "Value: ",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        "(formula not evaluated by Excel reader)",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    ),
                ]));
            } else {
                detail_lines.push(Line::from(Span::styled(
                    "No cell data",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
            }
        }

        detail_lines.push(Line::from(""));
        detail_lines.push(Line::from(vec![Span::styled(
            "↑↓ to scroll | Any other key to close",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::ITALIC),
        )]));

        // Calculate popup size (60% width, auto height)
        let area = frame.area();
        let popup_width = (area.width as f32 * 0.6).min(80.0) as u16;
        let popup_height =
            (detail_lines.len() + 4).min(area.height.saturating_sub(2) as usize) as u16;

        // Calculate content area (inside borders)
        let content_height = popup_height.saturating_sub(2) as usize; // Subtract borders
        let total_lines = detail_lines.len();

        // Clamp scroll offset to valid range
        let max_scroll = total_lines.saturating_sub(content_height);
        let scroll_offset = self.cell_detail_scroll.min(max_scroll);

        let popup_area = Rect {
            x: (area.width.saturating_sub(popup_width)) / 2,
            y: (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        // Clear the area behind the popup
        frame.render_widget(Clear, popup_area);

        // Build title with scroll indicator
        let mut title_spans = vec![
            Span::raw(" "),
            Span::styled(
                "Cell Details",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" - "),
            Span::styled(cell_addr.clone(), Style::default().fg(Color::Cyan)),
        ];

        // Add scroll indicator if content is scrollable
        if total_lines > content_height {
            let scroll_info = format!(" [{}/{}]", scroll_offset + 1, max_scroll + 1);
            title_spans.push(Span::styled(
                scroll_info,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ));
        }

        title_spans.push(Span::raw(" "));

        // Create detail content with scroll
        let detail_paragraph = Paragraph::new(detail_lines)
            .style(Style::default().fg(Color::White).bg(Color::Black))
            .scroll((scroll_offset as u16, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )
                    .title(title_spans)
                    .title_alignment(Alignment::Center),
            )
            .wrap(Wrap { trim: false });

        frame.render_widget(detail_paragraph, popup_area);
    }

    fn render_copy_feedback(&self, frame: &mut Frame, message: &str) {
        use ratatui::text::{Line, Span};

        // Create a small popup in the center
        let area = frame.area();
        let popup_width = (message.len() as u16 + 6).min(60);
        let popup_height = 3;

        let popup_area = Rect {
            x: (area.width.saturating_sub(popup_width)) / 2,
            y: (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        // Clear the area behind the popup
        frame.render_widget(Clear, popup_area);

        // Create feedback content
        let feedback_paragraph = Paragraph::new(Line::from(vec![Span::styled(
            message,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )]))
        .style(Style::default().bg(Color::Green).fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
                .title(" ✓ ")
                .title_alignment(Alignment::Center),
        )
        .alignment(Alignment::Center);

        frame.render_widget(feedback_paragraph, popup_area);
    }
}

/// Run the TUI application
pub fn run_tui(
    workbook: Workbook,
    sheet_name: &str,
    config: &crate::config::Config,
    horizontal_scroll: bool,
) -> Result<()> {
    let mut app = TuiState::new(workbook, sheet_name, config, horizontal_scroll)?;
    crate::terminal::with_terminal(|terminal| run_event_loop(terminal, &mut app))
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut TuiState,
) -> Result<()> {
    loop {
        // Draw needs mutable access to app for scroll updates
        terminal.draw(|f| {
            app.render(f);
        })?;

        app.tick_search();
        if event::poll(std::time::Duration::from_millis(
            if app.search_task.is_some() { 8 } else { 100 },
        ))? {
            let event = event::read()?;
            app.handle_event(event);
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cell_address_basic() {
        assert_eq!(TuiState::parse_cell_address("A1"), Some((0, 0)));
        assert_eq!(TuiState::parse_cell_address("B2"), Some((1, 1)));
        assert_eq!(TuiState::parse_cell_address("Z26"), Some((25, 25)));
    }

    #[test]
    fn test_parse_cell_address_double_letter() {
        assert_eq!(TuiState::parse_cell_address("AA1"), Some((26, 0)));
        assert_eq!(TuiState::parse_cell_address("AB5"), Some((27, 4)));
        assert_eq!(TuiState::parse_cell_address("AZ100"), Some((51, 99)));
    }

    #[test]
    fn test_parse_cell_address_lowercase() {
        assert_eq!(TuiState::parse_cell_address("a1"), Some((0, 0)));
        assert_eq!(TuiState::parse_cell_address("b2"), Some((1, 1)));
        assert_eq!(TuiState::parse_cell_address("aa10"), Some((26, 9)));
    }

    #[test]
    fn test_parse_cell_address_invalid() {
        assert_eq!(TuiState::parse_cell_address(""), None);
        assert_eq!(TuiState::parse_cell_address("1"), None);
        assert_eq!(TuiState::parse_cell_address("A"), None);
        assert_eq!(TuiState::parse_cell_address("123"), None);
        // Note: "A1B2" actually parses as AB12, which is valid
        // Test truly invalid inputs instead
        assert_eq!(TuiState::parse_cell_address("!@#"), None);
        assert_eq!(TuiState::parse_cell_address("A-1"), None);
    }

    #[test]
    fn test_parse_cell_address_large_column() {
        // BA = 2*26 + 1 = 53 (0-indexed: 52)
        assert_eq!(TuiState::parse_cell_address("BA1"), Some((52, 0)));
        // ZZ = 26*26 + 26 = 702 (0-indexed: 701)
        assert_eq!(TuiState::parse_cell_address("ZZ1"), Some((701, 0)));
    }

    #[test]
    fn test_column_to_letter() {
        // Test helper function for column letters
        let col_a = 0;
        let col_z = 25;
        let col_aa = 26;

        // Helper to convert column index to letter
        fn col_to_letter(col: usize) -> String {
            let mut result = String::new();
            let mut n = col + 1;
            while n > 0 {
                n -= 1;
                result.push((b'A' + (n % 26) as u8) as char);
                n /= 26;
            }
            result.chars().rev().collect()
        }

        assert_eq!(col_to_letter(col_a), "A");
        assert_eq!(col_to_letter(col_z), "Z");
        assert_eq!(col_to_letter(col_aa), "AA");
    }
}

// Toss evaluation-only tests against the real upstream state and renderer.
#[cfg(test)]
mod toss_eval_tests {
    use super::*;
    fn app() -> TuiState {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic.xlsx");
        TuiState::new(
            Workbook::open(path).unwrap(),
            "Data",
            &crate::config::Config::default(),
            true,
        )
        .unwrap()
    }
    fn press(app: &mut TuiState, code: KeyCode) {
        app.handle_event(Event::Key(KeyEvent::new(
            code,
            crossterm::event::KeyModifiers::NONE,
        )));
    }
    #[test]
    fn actual_renderer_navigation_sheet_search_and_quit() {
        let mut app = app();
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("Alpha"), "actual first sheet render");
        press(&mut app, KeyCode::Down);
        assert_eq!(app.cursor_row, 1);
        press(&mut app, KeyCode::Char('/'));
        for c in "needle".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        while app.search_task.is_some() {
            app.tick_search();
        }
        assert_eq!(app.search_matches, vec![(2, 2)]);
        assert_eq!(app.cursor_row, 2);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.current_sheet_name(), "Second");
        terminal.draw(|frame| app.render(frame)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("second-only"), "actual second sheet render");
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
    }
    #[test]
    fn narrow_terminal_and_cell_detail_render_without_panic() {
        let mut app = app();
        press(&mut app, KeyCode::Enter);
        assert!(app.show_cell_detail);
        for (w, h) in [(20, 8), (1, 1), (80, 24)] {
            let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
            terminal.draw(|frame| app.render(frame)).unwrap();
        }
    }
}

#[cfg(test)]
mod toss_repair_ui_tests {
    use super::*;
    fn fixture(n: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(n)
    }
    #[test]
    fn offset_address_jump_and_actual_detail_render() {
        let mut app = TuiState::new(
            Workbook::open(fixture("offset.xlsx")).unwrap(),
            "Offset",
            &crate::config::Config::default(),
            true,
        )
        .unwrap();
        assert_eq!(app.current_cell_address(), "D6");
        app.jump_input = "E6".into();
        app.perform_jump();
        assert_eq!((app.cursor_row, app.cursor_col), (0, 1));
        assert_eq!(app.current_cell_address(), "E6");
        app.handle_event(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        )));
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("1+2"));
        assert!(TuiState::parse_cell_address("A0").is_none());
        assert!(TuiState::parse_cell_address("A1B2").is_none());
        assert!(TuiState::parse_cell_address("AAAAAAAAAAAAAAAAAAAA1").is_none());
    }
    #[test]
    fn csv_no_header_renders_first_row_at_a1() {
        let wb = Workbook::open_with_options(fixture("edge.csv"), true, None).unwrap();
        let mut app = TuiState::new(wb, "edge", &crate::config::Config::default(), true).unwrap();
        assert_eq!(app.current_cell_address(), "A1");
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("00123"));
    }
    #[test]
    fn lazy_cache_returns_complete_multi_chunk_requests() {
        let mut app = TuiState::new(
            Workbook::open(fixture("rows-10000.xlsx")).unwrap(),
            "Rows",
            &crate::config::Config::default(),
            true,
        )
        .unwrap();
        for (start, count) in [(0, 3), (150, 500), (9800, 500)] {
            let (rows, _) = app.sheet_data.get_rows(start, count);
            assert_eq!(rows.len(), count.min(10000 - start));
            assert_eq!(rows[0][0].to_raw_string(), format!("row-{start:06}"));
        }
    }
}

#[cfg(test)]
mod toss_search_tests {
    use super::*;
    fn app() -> TuiState {
        let p =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rows-10000.xlsx");
        TuiState::new(
            Workbook::open(p).unwrap(),
            "Rows",
            &crate::config::Config::default(),
            true,
        )
        .unwrap()
    }
    fn key(app: &mut TuiState, k: KeyCode) {
        app.handle_event(Event::Key(KeyEvent::new(
            k,
            crossterm::event::KeyModifiers::NONE,
        )));
    }
    fn finish(app: &mut TuiState) {
        let mut ticks = 0;
        while app.search_task.is_some() {
            app.tick_search();
            ticks += 1;
            assert!(ticks < 10000);
        }
    }
    #[test]
    fn typing_does_not_scan_and_running_search_can_be_cancelled() {
        let mut app = app();
        key(&mut app, KeyCode::Char('/'));
        for c in "not-found".chars() {
            key(&mut app, KeyCode::Char(c));
        }
        assert!(app.search_task.is_none());
        assert!(app.search_matches.is_empty());
        assert!(app.progress.is_none());
        key(&mut app, KeyCode::Enter);
        assert!(app.search_task.is_some());
        app.tick_search();
        assert!(app.search_task.is_some());
        key(&mut app, KeyCode::Esc);
        assert!(app.search_task.is_none());
        assert!(app.search_matches.is_empty());
        assert!(!app.should_quit);
        assert!(app.progress.is_none());
        app.tick_search();
        assert!(app.search_matches.is_empty());
    }
    #[test]
    fn result_cap_visible_and_matches_sorted_for_binary_highlights() {
        let mut app = app();
        app.search_query = "row-".into();
        app.perform_search();
        finish(&mut app);
        assert_eq!(app.search_matches.len(), crate::search::MAX_RESULTS);
        assert!(app.search_limited);
        assert!(app.search_matches.windows(2).all(|w| w[0] < w[1]));
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(240, 30)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("result limit reached"));
        app.clear_search();
        app.search_query = "row-000499".into();
        app.perform_search();
        finish(&mut app);
        assert_eq!(app.search_matches, [(499, 0)]);
        assert!(!app.search_limited);
    }
    #[test]
    fn unicode_query_byte_limit_and_literal_metacharacters() {
        assert!(crate::search::Search::new(&"中".repeat(1400)).is_err());
        let search = crate::search::Search::new("A.*中").unwrap();
        assert!(search.matches("a.*中文"));
        assert!(!search.matches("abcd中文"));
        let mut app = app();
        key(&mut app, KeyCode::Char('/'));
        for _ in 0..1500 {
            key(&mut app, KeyCode::Char('中'));
        }
        assert!(app.search_query.len() <= crate::search::MAX_QUERY_BYTES);
    }
}

#[cfg(test)]
#[path = "color_ui_tests.rs"]
mod color_ui_tests;
