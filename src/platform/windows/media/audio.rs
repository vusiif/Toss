//! Keyboard-only audio controls. No window, detached input reader or TUI.
//! Redirected/non-console stdin is left untouched for the blocking MF pump.
use crate::core::error::TossError;
use std::{
    collections::VecDeque,
    path::Path,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::HANDLE,
    Media::MediaFoundation::{
        IMFMediaSession, IMFSimpleAudioVolume, MEEndOfPresentation, MESessionPaused,
        MESessionStarted, MF_E_NO_EVENTS_AVAILABLE, MF_EVENT_FLAG_NO_WAIT,
    },
    System::Console::{
        CONSOLE_MODE, ENABLE_ECHO_INPUT, ENABLE_EXTENDED_FLAGS, ENABLE_LINE_INPUT,
        ENABLE_PROCESSED_INPUT, ENABLE_QUICK_EDIT_MODE, ENABLE_VIRTUAL_TERMINAL_INPUT, FOCUS_EVENT,
        GetConsoleMode, GetNumberOfConsoleInputEvents, GetStdHandle, INPUT_RECORD, KEY_EVENT,
        LEFT_ALT_PRESSED, LEFT_CTRL_PRESSED, RIGHT_ALT_PRESSED, RIGHT_CTRL_PRESSED,
        ReadConsoleInputW, STD_INPUT_HANDLE, SetConsoleMode,
    },
    UI::Input::KeyboardAndMouse::{VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RIGHT, VK_SPACE, VK_UP},
};

const POLL: Duration = Duration::from_millis(20);
const SEEK_STEP: i64 = 5 * 10_000_000;
const MAX_QUEUE: usize = 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Toggle,
    Seek(i8),
    Volume(i8),
    Stop,
    Pause,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Starting,
    Playing,
    Paused,
}
#[derive(Clone, Copy, Debug)]
enum Pending {
    Started {
        restore_pause: bool,
        target: Option<i64>,
    },
    Paused,
}
struct Controls {
    mode: Mode,
    pending: Option<(Pending, Instant)>,
    restore_pause: bool,
    queue: VecDeque<Action>,
    stopped: bool,
}
impl Controls {
    fn new() -> Self {
        Self {
            mode: Mode::Starting,
            pending: Some((
                Pending::Started {
                    restore_pause: false,
                    target: None,
                },
                Instant::now(),
            )),
            restore_pause: false,
            queue: VecDeque::new(),
            stopped: false,
        }
    }
    fn push(&mut self, action: Action) -> bool {
        if action == Action::Stop {
            self.stopped = true;
            return true;
        }
        if self.queue.len() == MAX_QUEUE {
            return false;
        }
        self.queue.push_back(action);
        true
    }
    fn next(&mut self) -> Option<Action> {
        if self.stopped {
            return Some(Action::Stop);
        }
        if self.pending.is_some() {
            return None;
        }
        if self.restore_pause && self.mode == Mode::Playing {
            return Some(Action::Pause);
        }
        self.queue.pop_front()
    }
    fn started(&mut self) -> Option<Option<i64>> {
        let Some((
            Pending::Started {
                restore_pause,
                target,
            },
            _,
        )) = self.pending
        else {
            return None;
        };
        self.pending = None;
        self.mode = Mode::Playing;
        self.restore_pause = restore_pause;
        Some(target)
    }
    fn paused(&mut self) -> bool {
        if !matches!(self.pending, Some((Pending::Paused, _))) {
            return false;
        }
        self.pending = None;
        self.mode = Mode::Paused;
        self.restore_pause = false;
        true
    }
    fn begin(&mut self, pending: Pending) {
        self.pending = Some((pending, Instant::now()));
    }
    fn expired(&self) -> bool {
        self.pending.is_some_and(|(_, since)| {
            since.elapsed()
                >= if self.mode == Mode::Starting {
                    Duration::from_secs(15)
                } else {
                    Duration::from_secs(5)
                }
        })
    }
}

#[derive(Default)]
struct Keys {
    space_down: bool,
}
impl Keys {
    fn action(&mut self, record: INPUT_RECORD) -> Option<(Action, u16)> {
        if u32::from(record.EventType) == FOCUS_EVENT {
            // SAFETY: EventType selects the union's FocusEvent member.
            if !unsafe { record.Event.FocusEvent.bSetFocus }.as_bool() {
                self.space_down = false;
            }
            return None;
        }
        if u32::from(record.EventType) != KEY_EVENT {
            return None;
        }
        // SAFETY: EventType selects the union's KeyEvent member.
        let key = unsafe { record.Event.KeyEvent };
        if !key.bKeyDown.as_bool() {
            if key.wVirtualKeyCode == VK_SPACE.0 {
                self.space_down = false;
            }
            return None;
        }
        let ctrl = key.dwControlKeyState & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0;
        if key.wVirtualKeyCode == VK_ESCAPE.0 || (ctrl && key.wVirtualKeyCode == u16::from(b'C')) {
            return Some((Action::Stop, 1));
        }
        if key.dwControlKeyState
            & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED | LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED)
            != 0
        {
            return None;
        }
        let action = match key.wVirtualKeyCode {
            k if k == u16::from(b'Q') => Action::Stop,
            k if k == VK_SPACE.0 => {
                if self.space_down {
                    return None;
                }
                self.space_down = true;
                return Some((Action::Toggle, 1));
            }
            k if k == VK_LEFT.0 => Action::Seek(-1),
            k if k == VK_RIGHT.0 => Action::Seek(1),
            k if k == VK_UP.0 => Action::Volume(1),
            k if k == VK_DOWN.0 => Action::Volume(-1),
            _ => return None,
        };
        Some((action, key.wRepeatCount.clamp(1, 4)))
    }
}

struct ConsoleInput {
    handle: HANDLE,
    original: CONSOLE_MODE,
}
impl ConsoleInput {
    fn acquire() -> Result<Option<Self>, TossError> {
        // SAFETY: Reads the process's borrowed standard input handle and mode.
        // A pipe/file/invalid handle is not opened as CONIN$ or consumed.
        let handle = match unsafe { GetStdHandle(STD_INPUT_HANDLE) } {
            Ok(handle) => handle,
            Err(_) => return Ok(None),
        };
        let mut original = CONSOLE_MODE::default();
        // SAFETY: original is writable storage; GetConsoleMode rejects pipes.
        if unsafe { GetConsoleMode(handle, &mut original) }.is_err() {
            return Ok(None);
        }
        let guard = Self { handle, original };
        let disabled = ENABLE_LINE_INPUT
            | ENABLE_ECHO_INPUT
            | ENABLE_PROCESSED_INPUT
            | ENABLE_QUICK_EDIT_MODE
            | ENABLE_VIRTUAL_TERMINAL_INPUT;
        // SAFETY: Only the input console's flags are changed. The live guard
        // restores them even if this call or a later playback operation fails.
        unsafe { SetConsoleMode(handle, (original | ENABLE_EXTENDED_FLAGS) & !disabled) }.map_err(
            |err| TossError::other(format!("cannot enable audio keyboard input: {err}")),
        )?;
        Ok(Some(guard))
    }
    fn record(&self) -> Result<Option<INPUT_RECORD>, TossError> {
        let mut count = 0;
        // SAFETY: Borrowed live console handle and local writable count.
        unsafe { GetNumberOfConsoleInputEvents(self.handle, &mut count) }
            .map_err(|err| TossError::other(format!("cannot poll audio keyboard input: {err}")))?;
        if count == 0 {
            return Ok(None);
        }
        let mut record = INPUT_RECORD::default();
        let mut read = 0;
        // SAFETY: The console has input pending, and the slice owns one record.
        // Read only one so a quit does not consume subsequent shell input.
        unsafe { ReadConsoleInputW(self.handle, std::slice::from_mut(&mut record), &mut read) }
            .map_err(|err| TossError::other(format!("cannot read audio keyboard input: {err}")))?;
        Ok((read != 0).then_some(record))
    }
}
impl Drop for ConsoleInput {
    fn drop(&mut self) {
        // SAFETY: Handle is borrowed, not closed by this guard; restore exactly
        // the successfully read original flags, including Ctrl+C/echo settings.
        if let Err(err) = unsafe { SetConsoleMode(self.handle, self.original) } {
            eprintln!("Warning: could not restore console input mode: {err}");
        }
    }
}

fn volume_service(session: &IMFMediaSession, path: &Path) -> Option<IMFSimpleAudioVolume> {
    super::backend::simple_audio_volume(path, session).ok()
}
fn apply(
    action: Action,
    controls: &mut Controls,
    session: &IMFMediaSession,
    path: &Path,
    duration: i64,
    volume: &mut Option<IMFSimpleAudioVolume>,
) -> Result<bool, TossError> {
    match action {
        Action::Stop => return Ok(true),
        Action::Toggle if controls.mode == Mode::Paused => {
            super::backend::resume(session, path)?;
            controls.begin(Pending::Started {
                restore_pause: false,
                target: None,
            });
        }
        Action::Toggle | Action::Pause => {
            super::backend::pause(session, path)?;
            controls.begin(Pending::Paused);
        }
        Action::Seek(direction) => {
            let now = super::backend::current_time(session, path)?;
            let target = now.saturating_add(SEEK_STEP * i64::from(direction)).max(0);
            if duration > 0 && target >= duration {
                eprintln!("Audio: end of file");
                return Ok(true);
            }
            super::backend::seek_to(session, path, target)?;
            controls.begin(Pending::Started {
                restore_pause: controls.mode == Mode::Paused,
                target: Some(target),
            });
        }
        Action::Volume(direction) => {
            if volume.is_none() {
                *volume = volume_service(session, path);
            }
            let service = volume
                .as_ref()
                .ok_or_else(|| TossError::other("audio volume service is unavailable"))?;
            // SAFETY: The service is owned by this loop. Retaining its reference
            // permits paused volume changes without a paused-session lookup.
            let level = unsafe { service.GetMasterVolume() }
                .map_err(|err| TossError::other(format!("cannot read audio volume: {err}")))?;
            let target = (level + 0.1 * f32::from(direction)).clamp(0.0, 1.0);
            // SAFETY: The requested level is finite and clamped to 0..=1.
            unsafe { service.SetMasterVolume(target) }
                .map_err(|err| TossError::other(format!("cannot set audio volume: {err}")))?;
            // SAFETY: Read back the owned renderer's level rather than merely
            // reporting the requested value as an observed result.
            let actual = unsafe { service.GetMasterVolume() }.map_err(|err| {
                TossError::other(format!("cannot read changed audio volume: {err}"))
            })?;
            eprintln!("Audio: volume {:.0}%", actual * 100.0);
        }
    }
    Ok(false)
}

/// Return false without reading stdin when it is not a console. The caller
/// then uses its original blocking, noninteractive MF event pump.
pub(super) fn run_if_interactive(
    session: &IMFMediaSession,
    path: &Path,
    duration: i64,
) -> Result<bool, TossError> {
    let input = match ConsoleInput::acquire() {
        Ok(Some(input)) => input,
        Ok(None) => return Ok(false),
        Err(err) => {
            eprintln!("Warning: {err}; playing to completion without keyboard controls");
            return Ok(false);
        }
    };
    eprintln!(
        "Audio keys: Space pause/resume; Left/Right seek 5s; Up/Down volume; Esc/q/Ctrl+C quit."
    );
    let mut controls = Controls::new();
    let mut keys = Keys::default();
    let mut volume = None;
    let mut warned_queue = false;
    loop {
        for _ in 0..32 {
            // SAFETY: The caller owns the session throughout this loop; NO_WAIT
            // does not block while paused or waiting for keyboard input.
            let event = match unsafe { session.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => event,
                Err(err) if err.code() == MF_E_NO_EVENTS_AVAILABLE => break,
                Err(err) => return Err(super::failed(path, "poll audio playback", err)),
            };
            // SAFETY: The event is owned locally and both calls only read it.
            let (kind, status) = unsafe { (event.GetType(), event.GetStatus()) };
            let kind = kind.map_err(|err| super::failed(path, "read an audio event", err))?;
            let status =
                status.map_err(|err| super::failed(path, "read audio event status", err))?;
            if status.is_err() {
                return Err(super::from_hresult(path, "play audio", status));
            }
            if kind == MEEndOfPresentation.0 as u32 {
                return Ok(true);
            }
            if kind == MESessionStarted.0 as u32
                && let Some(target) = controls.started()
            {
                if let Some(service) = volume_service(session, path) {
                    volume = Some(service);
                }
                if let Some(target) = target {
                    eprintln!("Audio: seek {:.2} s", target as f64 / 10_000_000.0);
                } else {
                    eprintln!("Audio: playing");
                    if let Some(service) = volume.as_ref() {
                        // SAFETY: Read only the retained renderer service.
                        if let Ok(level) = unsafe { service.GetMasterVolume() } {
                            eprintln!("Audio: volume {:.0}%", level * 100.0);
                        }
                    }
                }
            }
            if kind == MESessionPaused.0 as u32 && controls.paused() {
                eprintln!("Audio: paused");
            }
        }
        for _ in 0..32 {
            let Some(record) = input.record()? else {
                break;
            };
            if let Some((action, repeat)) = keys.action(record) {
                for _ in 0..repeat {
                    if !controls.push(action) && !warned_queue {
                        eprintln!("Warning: audio input is busy; repeated controls were limited");
                        warned_queue = true;
                    }
                }
                if controls.stopped {
                    return Ok(true);
                }
            }
        }
        if controls.expired() {
            return Err(TossError::other(
                "audio session did not acknowledge a playback control in time",
            ));
        }
        if let Some(action) = controls.next() {
            match apply(action, &mut controls, session, path, duration, &mut volume) {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(err) => {
                    eprintln!("Warning: audio control failed: {err}");
                    controls.pending = None;
                    controls.restore_pause = false;
                }
            }
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Console::{INPUT_RECORD_0, KEY_EVENT_RECORD};
    fn key(code: u16, down: bool, repeat: u16, modifiers: u32) -> INPUT_RECORD {
        INPUT_RECORD {
            EventType: KEY_EVENT as u16,
            Event: INPUT_RECORD_0 {
                KeyEvent: KEY_EVENT_RECORD {
                    bKeyDown: down.into(),
                    wRepeatCount: repeat,
                    wVirtualKeyCode: code,
                    dwControlKeyState: modifiers,
                    ..Default::default()
                },
            },
        }
    }
    #[test]
    fn keys_map_controls_without_space_autorepeat_or_modified_arrow_capture() {
        let mut keys = Keys::default();
        assert_eq!(
            keys.action(key(VK_SPACE.0, true, 1, 0)),
            Some((Action::Toggle, 1))
        );
        assert_eq!(keys.action(key(VK_SPACE.0, true, 10, 0)), None);
        assert_eq!(keys.action(key(VK_SPACE.0, false, 1, 0)), None);
        assert_eq!(
            keys.action(key(VK_SPACE.0, true, 1, 0)),
            Some((Action::Toggle, 1))
        );
        assert_eq!(
            keys.action(key(VK_RIGHT.0, true, 10, 0)),
            Some((Action::Seek(1), 4))
        );
        assert_eq!(
            keys.action(key(VK_LEFT.0, true, 1, LEFT_CTRL_PRESSED)),
            None
        );
        assert_eq!(
            keys.action(key(b'C' as u16, true, 1, LEFT_CTRL_PRESSED)),
            Some((Action::Stop, 1))
        );
        assert_eq!(
            keys.action(key(b'Q' as u16, true, 1, 0)),
            Some((Action::Stop, 1))
        );
    }
    #[test]
    fn queued_controls_wait_for_ack_and_seek_restores_pause_before_next_key() {
        let mut state = Controls::new();
        state.push(Action::Toggle);
        assert_eq!(state.next(), None);
        assert_eq!(state.started(), Some(None));
        assert_eq!(state.next(), Some(Action::Toggle));
        state.begin(Pending::Paused);
        assert_eq!(state.next(), None);
        assert!(state.paused());
        state.begin(Pending::Started {
            restore_pause: true,
            target: Some(SEEK_STEP),
        });
        state.push(Action::Toggle);
        assert_eq!(state.started(), Some(Some(SEEK_STEP)));
        assert_eq!(state.next(), Some(Action::Pause));
        state.begin(Pending::Paused);
        assert!(state.paused());
        assert_eq!(state.next(), Some(Action::Toggle));
    }
    #[test]
    fn queue_is_bounded_and_quit_bypasses_pending_start_and_full_queue() {
        let mut state = Controls::new();
        for _ in 0..MAX_QUEUE {
            assert!(state.push(Action::Volume(1)));
        }
        assert!(!state.push(Action::Seek(1)));
        assert!(state.push(Action::Stop));
        assert_eq!(state.next(), Some(Action::Stop));
    }
    #[test]
    fn long_pause_has_no_deadline_but_unacknowledged_transitions_do() {
        let mut state = Controls::new();
        state.pending = Some((
            Pending::Started {
                restore_pause: false,
                target: None,
            },
            Instant::now() - Duration::from_secs(16),
        ));
        assert!(state.expired());
        state.pending = None;
        state.mode = Mode::Paused;
        assert!(!state.expired());
        state.pending = Some((Pending::Paused, Instant::now() - Duration::from_secs(6)));
        assert!(state.expired());
    }
}
