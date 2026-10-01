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

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH, UpdateWindow};
use windows::Win32::Media::MediaFoundation::{
    IMFMediaSession, MEEndOfPresentation, MF_EVENT_FLAG_NO_WAIT,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetMessageW, GetWindowLongPtrW, IDC_ARROW, IsWindow, LoadCursorW, MSG, PostQuitMessage,
    RegisterClassExW, SW_SHOW, SetTimer, SetWindowLongPtrW, ShowWindow, TranslateMessage,
    WM_DESTROY, WM_NCDESTROY, WM_TIMER, WNDCLASS_STYLES, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
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

/// What belongs to *this* playback window, in the same shape as Phase 6's
/// `ViewerState`: owned by [`run`] on its stack, addressed by the
/// `GWLP_USERDATA` slot, dropped exactly once.
struct PlayerState {
    session: IMFMediaSession,
    path: PathBuf,
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
pub(super) fn run(window: Window, session: IMFMediaSession, path: &Path) -> Result<(), TossError> {
    let mut state = Box::new(PlayerState {
        session,
        path: path.to_path_buf(),
        error: None,
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
