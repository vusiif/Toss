//! Live terminal companion and character progress bar for media playback.
//!
//! When attached to an interactive terminal, renders dynamic single-line playback
//! status on `stderr` updating the current position, duration, ASCII bar, and volume.
//! When redirected or piped (e.g. scripts, tests), remains completely silent.

use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::time::Instant;

use windows::Win32::Media::MediaFoundation::IMFMediaSession;

/// Refresh throttling interval in milliseconds.
const REFRESH_INTERVAL_MILLIS: u128 = 100;

/// Live console companion for media playback.
pub struct MediaDashboard {
    is_tty: bool,
    duration_100ns: i64,
    last_rendered: Instant,
    last_line_len: usize,
}

impl MediaDashboard {
    /// Create a new terminal companion for a media presentation with `duration_100ns`.
    #[must_use]
    pub fn new(duration_100ns: i64) -> Self {
        Self {
            is_tty: io::stderr().is_terminal(),
            duration_100ns,
            last_rendered: Instant::now() - std::time::Duration::from_secs(1),
            last_line_len: 0,
        }
    }

    /// Update the terminal line with current playback state.
    pub fn update(&mut self, session: &IMFMediaSession, path: &Path, paused: bool) {
        if !self.is_tty {
            return;
        }

        let now = Instant::now();
        if now.duration_since(self.last_rendered).as_millis() < REFRESH_INTERVAL_MILLIS {
            return;
        }
        self.last_rendered = now;
        self.render(session, path, paused);
    }

    /// Force an immediate update, bypassing the throttle (e.g. after keypress).
    pub fn force_update(&mut self, session: &IMFMediaSession, path: &Path, paused: bool) {
        if !self.is_tty {
            return;
        }
        self.last_rendered = Instant::now();
        self.render(session, path, paused);
    }

    fn render(&mut self, session: &IMFMediaSession, path: &Path, paused: bool) {
        let current_100ns = super::backend::current_time(session, path)
            .unwrap_or(0)
            .max(0);
        let ratio = if self.duration_100ns > 0 {
            (current_100ns as f64 / self.duration_100ns as f64).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let bar = crate::core::progress::render_slim_bar(ratio, 20);
        let current_str = super::format_duration(current_100ns as u64);
        let total_str = super::format_duration(self.duration_100ns.max(0) as u64);
        let vol_str = super::backend::volume(session, path)
            .map(|v| format!(" | Vol: {:>3}%", (v * 100.0).round() as u32))
            .unwrap_or_default();
        let state = if paused { "Paused " } else { "Playing" };

        let line = format!("[{state}] {bar} ({current_str} / {total_str}){vol_str}");

        let mut stderr = io::stderr().lock();
        let display_width = line.chars().count();
        let padding = if self.last_line_len > display_width {
            " ".repeat(self.last_line_len - display_width)
        } else {
            String::new()
        };
        self.last_line_len = display_width;
        let _ = write!(stderr, "\r{line}{padding}");
        let _ = stderr.flush();
    }

    /// Erase the dynamic line and leave the cursor at line start.
    pub fn finish(&mut self) {
        if self.is_tty && self.last_line_len > 0 {
            let mut stderr = io::stderr().lock();
            let blank = " ".repeat(self.last_line_len);
            let _ = write!(stderr, "\r{blank}\r");
            let _ = stderr.flush();
            self.last_line_len = 0;
        }
    }
}

impl Drop for MediaDashboard {
    fn drop(&mut self) {
        self.finish();
    }
}
