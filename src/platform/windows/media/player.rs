//! The video window, and the event pump that runs on it.
//!
//! P7-B (`MEDIA_PLAYBACK.md` §9): a window exists here because two platform
//! facts make it unavoidable — `MFCreateVideoRendererActivate` takes an hwnd
//! (there is no EVR without a window somewhere), and a video needs a message
//! pump to refresh at all. Neither fact is a reason to add *controls*:
//! keyboard handling, seeking and fullscreen are P7-C, and an audio-only file
//! never comes here (D2: no dummy window).
//!
//! The event pump is what this file exists to host. P7-A blocked on
//! `GetEvent`; a window that blocks cannot repaint, so the pump runs off a
//! timer and consumes events non-blockingly (`MF_EVENT_FLAG_NO_WAIT`). The
//! state rides in `GWLP_USERDATA` under the same rules Phase 6 established
//! for the viewer: each arm of the window procedure takes its own reference
//! and lets go before anything that could re-enter it.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH, UpdateWindow};
use windows::Win32::Media::MediaFoundation::{
    IMFMediaSession, MEEndOfPresentation, MF_EVENT_FLAG_NO_WAIT,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RIGHT, VK_SPACE, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWL_STYLE,
    GWLP_USERDATA, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, IDC_ARROW,
    IsWindow, LoadCursorW, MSG, PostQuitMessage, RegisterClassExW, SM_CXSCREEN, SM_CYSCREEN,
    SW_SHOW, SWP_FRAMECHANGED, SWP_NOZORDER, SetForegroundWindow, SetTimer, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, TranslateMessage, WM_DESTROY, WM_KEYDOWN, WM_NCDESTROY, WM_TIMER,
    WNDCLASS_STYLES, WNDCLASSEXW, WS_OVERLAPPEDWINDOW, WS_POPUP,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::core::error::TossError;

/// Class name for the media window.
const CLASS_NAME: PCWSTR = w!("TossMediaPlayer");

/// The window's size. Deliberately *not* the video's frame size: the packed
/// `MF_MT_FRAME_SIZE` attribute wants a two-value read this milestone did not
/// want to get subtly wrong, and the EVR letterboxes the picture into
/// whatever client area it is given — fitting the window to the frame is a
/// later milestone's decision (and fullscreen's).
const WIDTH: i32 = 640;
const HEIGHT: i32 = 480;

/// The timer that drives the event pump, in milliseconds.
const TIMER_PUMP: usize = 1;
const PUMP_INTERVAL_MS: u32 = 50;

/// One seek step: five seconds, fixed (MEDIA_PLAYBACK.md §3 recorded it
/// before this existed). No acceleration curve, no percentage of duration
/// — a step a person and a script can both predict (D2).
const SEEK_STEP: i64 = 5 * 10_000_000;

/// One volume step: ten percent of full scale. The backend clamps the
/// result into 0.0..=1.0, so a held key cannot ask for an illegal level.
const VOLUME_STEP: f32 = 0.1;

/// The F key, as a virtual-key code. There is no VK_F — VK_F1..
/// VK_F24 exist — so the letter itself is named here.
const KEY_FULLSCREEN: u16 = b'F' as u16;

/// What belongs to *this* playback window, in the same shape as Phase 6's
/// `ViewerState`: owned by [`run`] on its stack, addressed by the
/// `GWLP_USERDATA` slot, dropped exactly once.
struct PlayerState {
    session: IMFMediaSession,
    path: PathBuf,
    /// Whether playback is paused — Space's other half. The session's own
    /// transitions are asynchronous; this tracks what *Toss asked for*,
    /// which is exactly what the next key press needs to know.
    paused: bool,
    /// Whether the window is covering the screen, plus the style and
    /// rectangle to give back when F is pressed again — restored exactly,
    /// not recomputed, so two toggles land where they started.
    fullscreen: bool,
    saved_style: isize,
    saved_rect: (i32, i32, i32, i32),
    /// The presentation's length, read from the presentation descriptor at
    /// open — a seek is clamped into it so asking for past-the-end lands on
    /// the end (and ends) instead of hanging with nothing to play.
    duration: i64,
    /// A failure the event pump saw, carried out of the message loop — the
    /// same way the viewer carries a decode failure: the window procedure
    /// cannot return a `TossError`, so the state holds it until `run` can.
    error: Option<TossError>,
}

/// A window, created here and destroyed either by its own `WM_CLOSE` or, on
/// an error path, by this value's `Drop`.
pub(super) struct Window {
    handle: HWND,
}

impl Window {
    /// Register the class once per process and create the window.
    pub(super) fn new(path: &Path) -> Result<Self, TossError> {
        // The same call Phase 6's viewer makes, for the same reason and with
        // the same tolerance: a manifest may already have set it, and a
        // failure leaves whatever the process was given. Without it the
        // window is drawn through a DPI-virtualised scale, which on a modern
        // display is a blurry picture nobody asked for.
        //
        // SAFETY: the argument is a constant the system defines, and the
        // returned previous context is deliberately dropped — restoring it
        // would undo the setting for the rest of the process.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }

        let instance = super::module(path)?;
        register_class(path, instance)?;

        let title = title_of(path);

        // SAFETY: the class is registered, the title outlives the call, and
        // no parameter here is a pointer the window keeps beyond creation —
        // state is attached later, by [`run`].
        let handle = unsafe {
            CreateWindowExW(
                Default::default(),
                CLASS_NAME,
                &title,
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                WIDTH,
                HEIGHT,
                None,
                None,
                Some(instance),
                None,
            )
        }
        .map_err(|err| super::failed(path, "open a media window", err))?;

        Ok(Self { handle })
    }

    pub(super) fn handle(&self) -> HWND {
        self.handle
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // The normal path destroys the window through its own `WM_CLOSE`
        // long before this runs; what is left here is the error path, where
        // `run` bailed out with the window still up. Destroying it is what
        // keeps a failed play from leaving a stray window until process exit.
        //
        // SAFETY: `IsWindow` is the test — a destroyed handle fails it, and
        // a live one is valid to destroy from the thread that created it.
        unsafe {
            if IsWindow(Some(self.handle)).as_bool() {
                let _ = DestroyWindow(self.handle);
            }
        }
    }
}

/// Pump messages until the window is gone, then report what the pump saw.
///
/// The session arrives by clone (a COM reference, not a copy): the state
/// handed to `GWLP_USERDATA` holds it for the life of the loop, and the
/// caller keeps its own — which is what lets `play` close the session after
/// the window has gone, in its own order.
pub(super) fn run(
    window: Window,
    session: IMFMediaSession,
    path: &Path,
    duration: i64,
) -> Result<(), TossError> {
    let mut state = Box::new(PlayerState {
        session,
        path: path.to_path_buf(),
        error: None,
        paused: false,
        fullscreen: false,
        saved_style: 0,
        saved_rect: (0, 0, 0, 0),
        duration,
    });

    // SAFETY: `state` is a live box that outlives the loop below, and
    // `GWLP_USERDATA` is the slot Windows reserves for exactly this use.
    // Attaching after creation, as Phase 6 does: anything that arrived
    // during `CreateWindowExW` found an empty slot and fell through.
    unsafe {
        SetWindowLongPtrW(
            window.handle,
            GWLP_USERDATA,
            std::ptr::from_ref(&*state).addr() as isize,
        );
    }

    // SAFETY: both calls take the live handle this window owns; `SetTimer`
    // answers with the previous timer id (none of ours) and `ShowWindow`
    // with the prior visibility — neither is an outcome to act on. The
    // timer dies with the window, so there is nothing to pair by hand.
    unsafe {
        let _ = SetTimer(Some(window.handle), TIMER_PUMP, PUMP_INTERVAL_MS, None);
        let _ = ShowWindow(window.handle, SW_SHOW);
        // The picture window has to *hold* the keyboard, not merely exist:
        // keys go to whichever window has focus, and on the dropped-file
        // path this console was created first. Phase 6's viewer asked for
        // the foreground for the same reason.
        let _ = SetForegroundWindow(window.handle);
        let _ = UpdateWindow(window.handle);
    }

    let mut message = MSG::default();
    loop {
        // SAFETY: `message` is valid writable storage and no other loop
        // runs in this thread. Positive = a message, zero = `WM_QUIT`,
        // negative = failure — read as a number, never as a boolean.
        let arrived = unsafe { GetMessageW(&mut message, None, 0, 0) };

        if arrived.0 < 0 {
            return Err(TossError::other("media playback lost its message queue"));
        }
        if arrived.0 == 0 {
            break;
        }

        // SAFETY: `message` holds what `GetMessageW` just filled in.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    // The window is gone (its `WM_NCDESTROY` cleared the slot), so `state`
    // is nobody's but this frame's: read the outcome and drop it. `window`'s
    // own `Drop` sweeps anything an error path left standing.
    let outcome = state.error.take();
    drop(state);

    match outcome {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// The window procedure: host the timer, and nothing else.
///
/// There is no key handling here (P7-C) and no painting — the EVR owns the
/// picture, the class brush owns the black behind it, and `WM_CLOSE` through
/// `DefWindowProcW` is what a person gets from the title-bar cross.
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the system calls this with a live window; neither plain
    // parameter is dereferenced; the slot only ever holds an address `run`
    // put there, and `run` outlives every message this can be called for.
    unsafe {
        // The window is going away: leave no address behind. The state
        // belongs to `run`, so this detaches rather than releases — the
        // same rule as Phase 6's viewer.
        if message == WM_NCDESTROY {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            return DefWindowProcW(window, message, wparam, lparam);
        }

        let slot = GetWindowLongPtrW(window, GWLP_USERDATA);

        match message {
            // The pump. Every timer tick drains whatever events MF is
            // willing to hand over — non-blockingly, because a message loop
            // that blocks stops repainting, which is the whole reason this
            // window exists.
            WM_TIMER if slot != 0 => {
                let state = &mut *(slot as *mut PlayerState);
                pump(state, window);
                LRESULT(0)
            }

            // D2's controls, one match on the virtual-key code. Every
            // outcome that can fail reports through the state and takes the
            // window down — a window procedure cannot return a `TossError`,
            // so this is the same route the event pump uses, and `run`
            // reports whatever was left there.
            WM_KEYDOWN if slot != 0 => {
                let state = &mut *(slot as *mut PlayerState);
                let key = wparam.0 as u16;

                match key {
                    // Play/pause: VT_EMPTY on Start means "from here", and
                    // `paused` remembers which half of the toggle the next
                    // press is for.
                    k if k == VK_SPACE.0 => {
                        let outcome = if state.paused {
                            super::backend::resume(&state.session, &state.path)
                        } else {
                            super::backend::pause(&state.session, &state.path)
                        };
                        match outcome {
                            Ok(()) => state.paused = !state.paused,
                            Err(err) => fail(state, window, err),
                        }
                    }

                    // Seek by the fixed step, from where the clock is now.
                    // A step is added in 100-ns units; the backend clamps
                    // the result at zero, and running off the end is MF's
                    // answer to give.
                    k if k == VK_RIGHT.0 || k == VK_LEFT.0 => {
                        let direction = if key == VK_RIGHT.0 { 1 } else { -1 };
                        let current = super::backend::current_time(&state.session, &state.path);
                        let outcome = current.and_then(|now| {
                            super::backend::seek_to(
                                &state.session,
                                &state.path,
                                (now + SEEK_STEP * direction).min(state.duration),
                            )
                        });
                        if let Err(err) = outcome {
                            fail(state, window, err);
                        }
                    }

                    // Volume by a tenth of full scale, clamped in the
                    // backend. There is no on-screen readout: D2 ruled out
                    // an OSD, and the console line that could carry it
                    // belongs to the companion milestone, not this one.
                    k if k == VK_UP.0 || k == VK_DOWN.0 => {
                        let direction = if key == VK_UP.0 { 1.0_f32 } else { -1.0_f32 };
                        let level = super::backend::volume(&state.session, &state.path);
                        let outcome = level.and_then(|level| {
                            super::backend::set_volume(
                                &state.session,
                                &state.path,
                                level + VOLUME_STEP * direction,
                            )
                        });
                        if let Err(err) = outcome {
                            fail(state, window, err);
                        }
                    }

                    // Fullscreen: cover the screen, or give back the exact
                    // style and rectangle the window came with.
                    KEY_FULLSCREEN => toggle_fullscreen(window, state),

                    // Stop and leave — `DefWindowProc` would do the same
                    // for the cross, and `play` closes the session on the
                    // way out either way. Esc is D2's named exit.
                    k if k == VK_ESCAPE.0 => {
                        // This is the window the key arrived at, live for the
                        // whole loop; the same handle every arm here uses.
                        let _ = DestroyWindow(window);
                    }

                    // Every other key belongs to whoever else wants it.
                    _ => return DefWindowProcW(window, message, wparam, lparam),
                }

                LRESULT(0)
            }

            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }

            // `WM_CLOSE` — the title-bar cross — is deliberately left to
            // `DefWindowProcW`, which turns it into the destroy handled
            // above. Writing that chain by hand would be the same rule
            // twice (Phase 6 says so too).
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

/// Record a failure this procedure cannot return, and take the window down
/// so `run` can report it — the same escape hatch the event pump uses.
fn fail(state: &mut PlayerState, window: HWND, err: TossError) {
    state.error = Some(err);

    // SAFETY: the window is the one this message arrived at, live for the
    // whole loop.
    unsafe {
        let _ = DestroyWindow(window);
    }
}

/// F: cover the screen, or hand back the window's previous shape exactly.
///
/// A save/restore pair rather than a recomputed "best fit": what goes back
/// is the style and rectangle the window had before, so two toggles land
/// precisely where they started. D2's fullscreen, and nothing layered on
/// top of it — no OSD, no controls drawn over the picture (§3's non-goals
/// apply to the pixels as much as to the product).
fn toggle_fullscreen(window: HWND, state: &mut PlayerState) {
    if state.fullscreen {
        let (left, top, right, bottom) = state.saved_rect;

        // SAFETY: `window` is live; restoring the style and repositioning
        // with `SWP_FRAMECHANGED` is the documented pair — Windows
        // recalculates the frame around the style the window is given back.
        unsafe {
            SetWindowLongPtrW(window, GWL_STYLE, state.saved_style);
            let _ = SetWindowPos(
                window,
                None,
                left,
                top,
                right - left,
                bottom - top,
                SWP_FRAMECHANGED | SWP_NOZORDER,
            );
        }
    } else {
        // SAFETY: as above; the metrics are the primary screen's size in
        // pixels, which is the currency this DPI-aware window is measured
        // in (the same call Phase 6's viewer makes for its own cap).
        unsafe {
            state.saved_style = GetWindowLongPtrW(window, GWL_STYLE);

            let mut rect = RECT::default();
            let _ = GetWindowRect(window, &mut rect);
            state.saved_rect = (rect.left, rect.top, rect.right, rect.bottom);

            SetWindowLongPtrW(window, GWL_STYLE, WS_POPUP.0 as isize);
            let _ = SetWindowPos(
                window,
                None,
                0,
                0,
                GetSystemMetrics(SM_CXSCREEN),
                GetSystemMetrics(SM_CYSCREEN),
                SWP_FRAMECHANGED | SWP_NOZORDER,
            );
        }
    }

    state.fullscreen = !state.fullscreen;
}

/// Consume every event MF is willing to hand over right now.
///
/// Two outcomes end the window: the presentation's end (the normal case —
/// close it and the loop winds down) and an event that carries a failure
/// (record it in the state, then close it, so `run` can report it). All the
/// progress events in between — geometry updated, session started, streams
/// created — are exactly that: progress, and ignored here.
///
/// `GetEvent` with `NO_WAIT` returning an error means *nothing is waiting*,
/// which is the answer most timer ticks get; the failures that matter arrive
/// as events with a failing **status**, which is the branch below.
fn pump(state: &mut PlayerState, window: HWND) {
    loop {
        // SAFETY: the session is owned by `state`, which outlives this call,
        // and the event handed back is owned by this loop until it drops.
        let event = match unsafe { state.session.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
            Ok(event) => event,
            Err(_) => return,
        };

        // SAFETY: both calls only read the event. `GetType` answers which
        // event this is; `GetStatus`'s `Result` is whether the *read*
        // worked, and the `HRESULT` inside it is the event's own verdict —
        // which is where a playback failure hides, the same split P7-A read
        // on its blocking pump.
        let (kind, status) = unsafe {
            let kind = match event.GetType() {
                Ok(kind) => kind,
                Err(err) => {
                    state.error = Some(super::failed(&state.path, "read a media event", err));
                    let _ = DestroyWindow(window);
                    return;
                }
            };
            let status = match event.GetStatus() {
                Ok(status) => status,
                Err(err) => {
                    state.error = Some(super::failed(&state.path, "read an event's status", err));
                    let _ = DestroyWindow(window);
                    return;
                }
            };
            (kind, status)
        };

        if status.is_err() {
            state.error = Some(super::from_hresult(&state.path, "play the file", status));
            // SAFETY: the window is the one this pump was given, live for
            // the whole loop; destroying it is what ends that loop.
            unsafe {
                let _ = DestroyWindow(window);
            };
            return;
        }

        if kind == MEEndOfPresentation.0 as u32 {
            // SAFETY: as above — the normal end of the presentation.
            unsafe {
                let _ = DestroyWindow(window);
            };
            return;
        }
    }
}

/// Install the window procedure under `CLASS_NAME`, with a black background
/// (the EVR draws over it; anything unpainted should read as *off*, not as a
/// white flash) and the system arrow — a window that reports "no idea what
/// the mouse is doing" reads as frozen.
fn register_class(
    path: &Path,
    instance: windows::Win32::Foundation::HINSTANCE,
) -> Result<(), TossError> {
    // SAFETY: the stock brush and the system cursor are shared, immortal
    // handles the system documents as never needing release by us.
    unsafe {
        let cursor = LoadCursorW(None, IDC_ARROW)
            .map_err(|err| super::failed(path, "load the arrow cursor", err))?;
        let background = HBRUSH(GetStockObject(BLACK_BRUSH).0);

        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: Default::default(),
            hCursor: cursor,
            hbrBackground: background,
            lpszMenuName: PCWSTR::null(),
            lpszClassName: CLASS_NAME,
            hIconSm: Default::default(),
        };

        if RegisterClassExW(&class) == 0 {
            return Err(TossError::other(
                "media playback could not register its window class",
            ));
        }
    }

    Ok(())
}

/// The window title: the file being played, so a taskbar of one says what it
/// is showing.
fn title_of(path: &Path) -> HSTRING {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    HSTRING::from(name)
}
