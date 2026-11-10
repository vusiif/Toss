//! Showing a picture in the terminal when no display can be reached — an
//! SSH session, a text console, a container.
//!
//! The terminal is asked what it can draw (kitty, sixel or iTerm2 graphics)
//! and falls back to Unicode half-blocks when it cannot answer, so every
//! terminal shows *something* recognisable. Browsing works as in the
//! window; zoom and pan do not, and the controls printed say so.

use std::io::{self, IsTerminal, Stdout};

use image_codecs::{DynamicImage, RgbImage};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Constraint, Layout};
use ratatui::widgets::Paragraph;
use ratatui_image::StatefulImage;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

use crate::core::error::TossError;
use crate::platform::raster::Decoded;

use super::view::{Effect, Key, View};

/// The keys the terminal viewer actually answers to.
pub(super) const CONTROLS: [(&str, &str); 2] =
    [("Left/Right", "Previous / next"), ("Esc, q", "Close")];

/// Whether there is a terminal to draw in at all.
pub(super) fn available() -> bool {
    io::stdout().is_terminal() && io::stdin().is_terminal()
}

/// Leaves the alternate screen and raw mode however the viewer ends.
struct Screen;

impl Screen {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(err) = execute!(io::stdout(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(err);
        }
        Ok(Self)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

/// Show `view` in the terminal until the user closes it.
pub(super) fn run(mut view: View) -> Result<(), TossError> {
    let failed = |err: io::Error| TossError::other(format!("terminal image viewer: {err}"));
    // Ask before taking over the screen: the query needs a cooked terminal.
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((8, 16)));
    let _screen = Screen::enter().map_err(failed)?;
    let mut terminal: Terminal<CrosstermBackend<Stdout>> =
        Terminal::new(CrosstermBackend::new(io::stdout())).map_err(failed)?;
    let mut protocol: StatefulProtocol = picker.new_resize_protocol(flatten(view.image()));
    let footer = CONTROLS
        .iter()
        .map(|(key, action)| format!("{key}: {action}"))
        .collect::<Vec<_>>()
        .join("  ·  ");

    loop {
        let title = view.title();
        terminal
            .draw(|frame| {
                let [picture, status] =
                    Layout::vertical([Constraint::Min(1), Constraint::Length(1)])
                        .areas(frame.area());
                frame.render_stateful_widget(StatefulImage::new(), picture, &mut protocol);
                frame.render_widget(Paragraph::new(format!("{title}  —  {footer}")), status);
            })
            .map_err(failed)?;

        let Event::Key(key) = event::read().map_err(failed)? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let key = match key.code {
            KeyCode::Right | KeyCode::PageDown => Key::Next,
            KeyCode::Left | KeyCode::PageUp => Key::Previous,
            KeyCode::Esc | KeyCode::Char('q') => Key::Close,
            _ => continue,
        };
        match view.key(key) {
            Effect::Close => return Ok(()),
            Effect::Retitle => protocol = picker.new_resize_protocol(flatten(view.image())),
            Effect::Nothing | Effect::Redraw => {}
        }
    }
}

/// Premultiplied BGRA composited over white, as the encoder wants RGB.
fn flatten(image: &Decoded) -> DynamicImage {
    let mut rgb = Vec::with_capacity(image.width as usize * image.height as usize * 3);
    for bgra in image.pixels.as_chunks::<4>().0 {
        let clear = 255 - u16::from(bgra[3]);
        for c in [bgra[2], bgra[1], bgra[0]] {
            rgb.push((u16::from(c) + clear).min(255) as u8);
        }
    }
    RgbImage::from_raw(image.width, image.height, rgb)
        .map(DynamicImage::ImageRgb8)
        .unwrap_or_else(|| DynamicImage::new_rgb8(1, 1))
}

#[cfg(test)]
mod tests {
    use super::flatten;
    use crate::platform::raster::Decoded;

    #[test]
    fn flattening_keeps_opaque_colour_and_whitens_transparency() {
        let image = Decoded {
            width: 2,
            height: 1,
            pixels: vec![10, 20, 30, 255, 0, 0, 0, 0],
        };
        let rgb = flatten(&image).to_rgb8().into_raw();
        assert_eq!(rgb, [30, 20, 10, 255, 255, 255]);
    }
}
