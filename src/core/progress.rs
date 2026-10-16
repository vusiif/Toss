//! Console progress display for long-running operations (§20).
//!
//! When connected to an interactive terminal, renders a single-line progress
//! indicator on stderr with a dynamic spinner, entry count, byte volume and
//! current filename.
//!
//! When redirected (e.g. scripts, pipelines, CI or automated tests), it
//! remains completely silent, adhering to the output purity contract (§25).

use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

/// Dots animation frames (Indicatif Dots).
const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Minimum elapsed duration between two consecutive console redraws.
const REFRESH_INTERVAL_MILLIS: u128 = 40;

/// State for the active progress line.
struct ProgressState {
    entries: u64,
    bytes: u64,
    total_bytes: Option<u64>,
    spinner_idx: usize,
    last_rendered: Instant,
    last_line_len: usize,
}

/// Render an 8-level sub-character fractional Unicode block progress bar (Indicatif Fine Bar).
///
/// Output shape: `▕████████▌       ▏  53%`
#[must_use]
pub fn render_fine_bar(ratio: f64, width: usize) -> String {
    let clamped = ratio.clamp(0.0, 1.0);
    let pct = (clamped * 100.0) as u32;

    if width == 0 {
        return format!("{pct:>3}%");
    }

    const FRACTIONAL: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let total_steps = (width * 8) as f64;
    let current_step = (clamped * total_steps).round() as usize;
    let full_blocks = (current_step / 8).min(width);
    let frac = current_step % 8;

    let mut bar = String::with_capacity(width * 4);
    for _ in 0..full_blocks {
        bar.push('█');
    }

    if full_blocks < width {
        let has_frac = frac > 0;
        if has_frac {
            bar.push(FRACTIONAL[frac]);
        }
        let remaining = width - full_blocks - if has_frac { 1 } else { 0 };
        for _ in 0..remaining {
            bar.push(' ');
        }
    }

    format!("▕{bar}▏ {pct:>3}%")
}

/// Render the "Slim Modern" indicatif progress bar for media playback.
///
/// Output shape: `╢━━━━━━━━━┈┈┈┈┈┈┈┈┈┈┈╟  45%`
#[allow(dead_code)]
#[must_use]
pub fn render_slim_bar(ratio: f64, width: usize) -> String {
    let clamped = ratio.clamp(0.0, 1.0);
    let pct = (clamped * 100.0) as u32;

    if width == 0 {
        return format!("{pct:>3}%");
    }

    let filled = (clamped * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width - filled;

    let mut bar = String::with_capacity(width * 4);
    for _ in 0..filled {
        bar.push('━');
    }
    for _ in 0..empty {
        bar.push('┈');
    }

    format!("╢{bar}╟ {pct:>3}%")
}

/// Render a visual progress bar (defaults to high-precision fine bar).
#[must_use]
pub fn render_bar(ratio: f64, width: usize) -> String {
    render_fine_bar(ratio, width)
}

/// Render an indeterminate/sliding block inside borders `▕...▏`.
///
/// Output shape: e.g. `▕   ████         ▏` sliding back and forth across the bar.
#[must_use]
pub fn render_indeterminate_bar(idx: usize, width: usize, block_len: usize) -> String {
    if width <= block_len {
        return format!("▕{}▏", "█".repeat(width));
    }
    let max_pos = width - block_len;
    let cycle = max_pos * 2;
    let step = idx % cycle;
    let pos = if step <= max_pos { step } else { cycle - step };
    let before = " ".repeat(pos);
    let bar = "█".repeat(block_len);
    let after = " ".repeat(width - pos - block_len);
    format!("▕{before}{bar}{after}▏")
}

/// A terminal-aware progress indicator.
pub struct ProgressBar {
    operation: &'static str,
    is_tty: bool,
    start_time: Instant,
    state: Mutex<ProgressState>,
}

/// Statistics gathered over the lifetime of an operation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressSummary {
    pub entries: u64,
    pub bytes: u64,
    pub elapsed: std::time::Duration,
}

impl ProgressSummary {
    /// Calculate average throughput in MiB/s.
    #[must_use]
    pub fn throughput_mibs(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs > 0.0 {
            (self.bytes as f64 / 1_048_576.0) / secs
        } else {
            0.0
        }
    }
}

impl ProgressBar {
    /// Create a new progress bar for `operation` (e.g. "Extracting", "Compressing").
    #[must_use]
    pub fn new(operation: &'static str) -> Self {
        let is_tty = io::stderr().is_terminal();
        Self {
            operation,
            is_tty,
            start_time: Instant::now(),
            state: Mutex::new(ProgressState {
                entries: 0,
                bytes: 0,
                total_bytes: None,
                spinner_idx: 0,
                last_rendered: Instant::now(),
                last_line_len: 0,
            }),
        }
    }

    /// Attach an expected total byte count to enable percentage calculation and a visual bar.
    #[must_use]
    pub fn with_total_bytes(self, total: Option<u64>) -> Self {
        if let Ok(mut state) = self.state.lock() {
            state.total_bytes = total;
        }
        self
    }

    /// Whether this progress bar is connected to an interactive terminal.
    #[cfg(test)]
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.is_tty
    }

    /// Format the single-line progress text for display.
    #[must_use]
    pub fn format_line(
        operation: &str,
        spinner: char,
        entries: u64,
        bytes: u64,
        total_bytes: Option<u64>,
        spinner_idx: usize,
        current: &Path,
    ) -> String {
        let size_str = compact_size(bytes);

        // Truncate path if too long to prevent terminal wrapping
        let name_str = current
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_else(|| current.to_string_lossy());
        let truncated_name = if name_str.chars().count() > 28 {
            let prefix: String = name_str.chars().take(25).collect();
            format!("{prefix}...")
        } else {
            name_str.into_owned()
        };

        if let Some(total) = total_bytes.filter(|&t| t > 0) {
            let ratio = (bytes as f64 / total as f64).clamp(0.0, 1.0);
            let bar = render_bar(ratio, 14);
            let total_str = compact_size(total);
            format!(
                "{operation} {spinner} {bar} [{entries} entries, {size_str} / {total_str}] {truncated_name}"
            )
        } else {
            let bar = render_indeterminate_bar(spinner_idx, 10, 3);
            format!("{operation} {spinner} {bar} [{entries} entries, {size_str}] {truncated_name}")
        }
    }

    /// Update progress with current counters and active file name.
    pub fn update(&self, entries: u64, bytes: u64, current: &Path) {
        if !self.is_tty {
            return;
        }

        let Ok(mut state) = self.state.lock() else {
            return;
        };

        state.entries = entries;
        state.bytes = bytes;

        let now = Instant::now();
        // Throttle rapid updates to prevent terminal I/O latency
        if now.duration_since(state.last_rendered).as_millis() < REFRESH_INTERVAL_MILLIS {
            return;
        }

        state.last_rendered = now;
        state.spinner_idx = (state.spinner_idx + 1) % SPINNER.len();

        let spinner = SPINNER[state.spinner_idx];
        let line = Self::format_line(
            self.operation,
            spinner,
            entries,
            bytes,
            state.total_bytes,
            state.spinner_idx,
            current,
        );

        let mut stderr = io::stderr().lock();
        let display_width = line.chars().count();
        let padding = if state.last_line_len > display_width {
            " ".repeat(state.last_line_len - display_width)
        } else {
            String::new()
        };

        state.last_line_len = display_width;
        let _ = write!(stderr, "\r{line}{padding}");
        let _ = stderr.flush();
    }

    /// Clear the progress line and return statistics gathered over the operation.
    pub fn finish(&self) -> ProgressSummary {
        let (entries, bytes, last_line_len) = {
            if let Ok(mut state) = self.state.lock() {
                let len = state.last_line_len;
                state.last_line_len = 0;
                (state.entries, state.bytes, len)
            } else {
                (0, 0, 0)
            }
        };

        if self.is_tty && last_line_len > 0 {
            let mut stderr = io::stderr().lock();
            let blank = " ".repeat(last_line_len);
            let _ = write!(stderr, "\r{blank}\r");
            let _ = stderr.flush();
        }

        ProgressSummary {
            entries,
            bytes,
            elapsed: self.start_time.elapsed(),
        }
    }
}

/// Format bytes into human-readable compact form (B, KiB, MiB, GiB, TiB).
#[must_use]
pub fn compact_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

    if bytes < 1024 {
        return format!("{bytes} B");
    }

    let mut unit = 0;
    let mut probe = bytes;
    while probe >= 1024 && unit < UNITS.len() - 1 {
        probe /= 1024;
        unit += 1;
    }

    let divisor = 1024u64.pow(unit as u32);
    let value = bytes / divisor;
    let remainder = bytes % divisor;
    let tenths = (remainder * 10) / divisor;

    format!("{value}.{tenths} {}", UNITS[unit])
}

impl Drop for ProgressBar {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn non_tty_progress_bar_remains_silent() {
        let bar = ProgressBar::new("Testing").with_total_bytes(Some(2048));
        // In test runners stderr is piped, so is_terminal is false
        if !bar.is_terminal() {
            bar.update(10, 1024, Path::new("sample.txt"));
            bar.finish();
        }
    }

    #[test]
    fn render_fine_bar_produces_smooth_unicode_fractional_blocks() {
        assert_eq!(render_fine_bar(0.0, 10), "▕          ▏   0%");
        assert_eq!(render_fine_bar(0.5, 10), "▕█████     ▏  50%");
        assert_eq!(render_fine_bar(1.0, 10), "▕██████████▏ 100%");
        assert_eq!(render_fine_bar(1.5, 10), "▕██████████▏ 100%");
        assert_eq!(render_fine_bar(0.5, 0), " 50%");
    }

    #[test]
    fn render_slim_bar_produces_indicatif_slim_modern_style() {
        assert_eq!(render_slim_bar(0.0, 10), "╢┈┈┈┈┈┈┈┈┈┈╟   0%");
        assert_eq!(render_slim_bar(0.5, 10), "╢━━━━━┈┈┈┈┈╟  50%");
        assert_eq!(render_slim_bar(1.0, 10), "╢━━━━━━━━━━╟ 100%");
        assert_eq!(render_slim_bar(1.5, 10), "╢━━━━━━━━━━╟ 100%");
        assert_eq!(render_slim_bar(0.5, 0), " 50%");
    }

    #[test]
    fn render_indeterminate_bar_bounces_correctly() {
        assert_eq!(render_indeterminate_bar(0, 10, 3), "▕███       ▏");
        assert_eq!(render_indeterminate_bar(1, 10, 3), "▕ ███      ▏");
        assert_eq!(render_indeterminate_bar(7, 10, 3), "▕       ███▏");
        assert_eq!(render_indeterminate_bar(8, 10, 3), "▕      ███ ▏");
    }

    #[test]
    fn format_line_formats_entries_and_human_size_with_total() {
        let line = ProgressBar::format_line(
            "Compressing",
            '⠋',
            42,
            2048,
            Some(4096),
            0,
            Path::new("test.txt"),
        );
        assert!(line.contains("50%"), "expected percentage in line: {line}");
        assert!(
            line.contains("[42 entries, 2.0 KiB / 4.0 KiB]"),
            "expected entries and sizes: {line}"
        );
        assert!(line.contains("▕███████       ▏"));
    }

    #[test]
    fn format_line_formats_indeterminate_bar_without_total() {
        let line =
            ProgressBar::format_line("Extracting", '⠙', 42, 2048, None, 0, Path::new("test.txt"));
        assert_eq!(
            line,
            "Extracting ⠙ ▕███       ▏ [42 entries, 2.0 KiB] test.txt"
        );
    }

    #[test]
    fn format_line_truncates_overly_long_filenames() {
        let long_name = "this_is_an_extremely_long_filename_that_would_overflow_the_terminal.dat";
        let line =
            ProgressBar::format_line("Compressing", '⠋', 5, 500, None, 0, Path::new(long_name));
        assert!(
            line.contains("..."),
            "line should truncate long name: {line}"
        );
        assert!(
            line.starts_with(
                "Compressing ⠋ ▕███       ▏ [5 entries, 500 B] this_is_an_extremely_long..."
            ),
            "unexpected formatted line: {line}"
        );
    }

    #[test]
    fn progress_summary_calculates_throughput() {
        let summary = ProgressSummary {
            entries: 10,
            bytes: 2 * 1_048_576, // 2 MiB
            elapsed: std::time::Duration::from_secs(1),
        };
        assert!((summary.throughput_mibs() - 2.0).abs() < 0.001);

        let zero = ProgressSummary {
            entries: 0,
            bytes: 0,
            elapsed: std::time::Duration::from_secs(0),
        };
        assert_eq!(zero.throughput_mibs(), 0.0);
    }
}
