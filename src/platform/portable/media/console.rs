//! Keyboard controls for playback that has no window: a sound file, or a
//! film on a machine with no display.
//!
//! The same shape as the Windows audio controls, down to the words: no
//! window, no TUI, one line of keys and one line per thing that happens.
//! A stdin that is not a terminal (a pipe, a script) is left untouched and
//! the file simply plays to its end.

use std::io::{IsTerminal, Write};
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode};

use crate::core::log;

use super::SEEK_STEP;
use super::decoder::SECOND;
use super::engine::Engine;

/// How often the end of playback is checked while waiting for a key.
const POLL: Duration = Duration::from_millis(20);

/// Puts the terminal back however playback ends.
struct RawMode;

impl RawMode {
    fn enter() -> std::io::Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

/// A status line. Raw mode turns off the terminal's own newline handling,
/// so the carriage return is written out.
fn say(message: std::fmt::Arguments<'_>) {
    let mut stderr = std::io::stderr().lock();
    let _ = write!(stderr, "{message}\r\n");
    let _ = stderr.flush();
}

/// What a key asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Toggle,
    Seek(i64),
    Volume(i32),
    Stop,
}

fn action(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> Option<Action> {
    if kind == KeyEventKind::Release {
        return None;
    }
    let held = kind == KeyEventKind::Repeat;
    match code {
        KeyCode::Char('c' | 'C') if modifiers.contains(KeyModifiers::CONTROL) => Some(Action::Stop),
        KeyCode::Esc | KeyCode::Char('q' | 'Q') => Some(Action::Stop),
        // A held space bar must not flicker between playing and paused.
        KeyCode::Char(' ') if !held => Some(Action::Toggle),
        KeyCode::Left => Some(Action::Seek(-SEEK_STEP)),
        KeyCode::Right => Some(Action::Seek(SEEK_STEP)),
        KeyCode::Up => Some(Action::Volume(1)),
        KeyCode::Down => Some(Action::Volume(-1)),
        _ => None,
    }
}

/// Play `engine` to its end, or until the user stops it.
pub(super) fn run(engine: &Engine) {
    let raw = if std::io::stdin().is_terminal() {
        match RawMode::enter() {
            Ok(raw) => Some(raw),
            Err(err) => {
                log::diagnostic(format_args!(
                    "Warning: {err}; playing to completion without keyboard controls"
                ));
                None
            }
        }
    } else {
        None
    };
    if raw.is_none() {
        while !engine.finished() {
            std::thread::sleep(POLL);
        }
        return;
    }
    say(format_args!(
        "Audio keys: Space pause/resume; Left/Right seek 5s; Up/Down volume; Esc/q/Ctrl+C quit."
    ));

    while !engine.finished() {
        match event::poll(POLL) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(_) => {
                // The terminal went away; the file still plays out.
                std::thread::sleep(POLL);
                continue;
            }
        }
        let Ok(Event::Key(key)) = event::read() else {
            continue;
        };
        match action(key.code, key.modifiers, key.kind) {
            Some(Action::Stop) => break,
            Some(Action::Toggle) => {
                if engine.toggle_pause() {
                    say(format_args!("Audio: paused"));
                } else {
                    say(format_args!("Audio: playing"));
                }
            }
            Some(Action::Seek(delta)) => {
                let target = engine.seek_by(delta);
                if engine.duration().is_some_and(|duration| target >= duration) {
                    say(format_args!("Audio: end of file"));
                } else {
                    say(format_args!(
                        "Audio: seek {:.2} s",
                        target as f64 / SECOND as f64
                    ));
                }
            }
            Some(Action::Volume(steps)) => {
                let level = engine.change_volume(steps);
                say(format_args!("Audio: volume {:.0}%", level * 100.0));
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    use super::{Action, action};
    use crate::platform::portable::media::SEEK_STEP;

    #[test]
    fn keys_map_to_the_actions_the_key_line_promises() {
        let none = KeyModifiers::NONE;
        let press = KeyEventKind::Press;
        assert_eq!(
            action(KeyCode::Char(' '), none, press),
            Some(Action::Toggle)
        );
        assert_eq!(
            action(KeyCode::Left, none, press),
            Some(Action::Seek(-SEEK_STEP))
        );
        assert_eq!(
            action(KeyCode::Right, none, press),
            Some(Action::Seek(SEEK_STEP))
        );
        assert_eq!(action(KeyCode::Up, none, press), Some(Action::Volume(1)));
        assert_eq!(action(KeyCode::Down, none, press), Some(Action::Volume(-1)));
        for stop in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Char('Q')] {
            assert_eq!(action(stop, none, press), Some(Action::Stop));
        }
        assert_eq!(
            action(KeyCode::Char('c'), KeyModifiers::CONTROL, press),
            Some(Action::Stop)
        );
        assert_eq!(action(KeyCode::Char('c'), none, press), None);
        assert_eq!(action(KeyCode::Char('x'), none, press), None);
    }

    #[test]
    fn a_held_space_bar_toggles_once_and_releases_do_nothing() {
        let none = KeyModifiers::NONE;
        assert_eq!(action(KeyCode::Char(' '), none, KeyEventKind::Repeat), None);
        assert_eq!(
            action(KeyCode::Char(' '), none, KeyEventKind::Release),
            None
        );
        assert_eq!(action(KeyCode::Esc, none, KeyEventKind::Release), None);
        // Arrows do repeat: holding one keeps seeking.
        assert_eq!(
            action(KeyCode::Right, none, KeyEventKind::Repeat),
            Some(Action::Seek(SEEK_STEP))
        );
    }
}
