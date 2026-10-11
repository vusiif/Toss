//! Software HEVC playback: the third link of the degradation chain.
//!
//! The design (`docs/Phase76_Fallback_Boundary_Design.md` §2, §7, §8) puts
//! this file at the tail of `backend::play`'s attempt chain: Media
//! Foundation plays first, and only a typed `TossError::UnsupportedFormat`
//! — "this machine has no decoder for that" — earns a second opinion from
//! the bundled FFmpeg. This module is that second opinion's window.
//!
//! One thread does everything: the caller's thread is the UI thread *and*
//! the decode thread (§7 — correctness first, a worker is decided by
//! measurement later, never assumed). The message loop is the same skeleton
//! `player.rs` uses, with three arms changed:
//!
//! - `WM_PAINT` draws the *visible* BGRA buffer with GDI `StretchDIBits`,
//!   letterboxed into the client rect, after clearing the whole client black
//!   so moved margins never keep old video pixels.
//! - `WM_TIMER` advances the presentation clock, promotes a decoded-ahead
//!   picture to visible exactly when its PTS is reached, services a pending
//!   seek in bounded preroll steps, and returns to the queue: a tick never
//!   drains the whole file at once, so input stays responsive.
//! - `F` is pure geometry — there is no renderer interface to ask, because
//!   the picture is Toss's own buffer.
//!
//! **Two frame buffers, and only two.** `visible` holds the picture on
//! screen; `pending` holds the one decoded ahead. Promotion swaps the two,
//! so both allocations live for the whole session — a bounded pair, never a
//! per-frame `Vec`. Painting reads `visible` only; the presentation clock
//! and the seek position derive from `visible`'s PTS, never the lookahead's.
//!
//! v1 is silent video only: no audio stream is opened, `Up`/`Down` do
//! nothing, and the console says so plainly rather than implying sound.

use std::mem::size_of;
use std::path::Path;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLACK_BRUSH, BeginPaint, DIB_RGB_COLORS, EndPaint,
    FillRect, GetDC, GetStockObject, HALFTONE, HBRUSH, HDC, InvalidateRect, PAINTSTRUCT, RGBQUAD,
    ReleaseDC, SRCCOPY, SetBrushOrgEx, SetStretchBltMode, StretchDIBits, UpdateWindow,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RIGHT, VK_SPACE, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetClientRect, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, HWND_NOTOPMOST,
    HWND_TOPMOST, IDC_ARROW, IsWindow, LoadCursorW, MSG, PM_REMOVE, PeekMessageW, PostQuitMessage,
    RegisterClassExW, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, SetForegroundWindow, SetTimer,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, WM_DESTROY, WM_KEYDOWN,
    WM_NCDESTROY, WM_PAINT, WM_QUIT, WM_TIMER, WNDCLASS_STYLES, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{HSTRING, PCWSTR, w};

use super::libav::HevcReader;
use crate::core::error::TossError;
use crate::core::log;

/// Class name for a software playback window. Distinct from the MF player's
/// (`TossMediaPlayer`): the two window procedures answer different messages
/// with different state, and one class cannot host both.
const CLASS_NAME: PCWSTR = w!("TossSoftwarePlayer");

/// The fixed window size, the same 640×480 the MF path uses — a later
/// milestone fits the window to the frame; this one letterboxes into it.
const WIDTH: i32 = 640;
const HEIGHT: i32 = 480;

/// The timer that drives pacing and decoding, in milliseconds. Short enough
/// that a 60 fps file is not visibly late, long enough not to spin.
const TIMER_DECODE: usize = 1;
const DECODE_INTERVAL_MS: u32 = 10;

/// How many pictures one timer tick may promote or decode-ahead before
/// returning to the queue. "Bounded decode work": after a pause or a slow
/// tick the catch-up never monopolises the thread, so Esc still lands.
const MAX_FRAMES_PER_TICK: u32 = 4;

/// How many preroll pictures one tick may decode while completing a seek.
/// The same bound, for the same reason: a seek across a long GOP stays
/// interruptible instead of draining the whole GOP in one key press.
const MAX_PREROLL_PER_TICK: u32 = 4;

/// One seek step: five seconds, fixed (D2), in 100-ns units.
const SEEK_STEP: i64 = 5 * 10_000_000;

/// The `F` key, as a virtual-key code (there is no `VK_F`).
const KEY_FULLSCREEN: u16 = b'F' as u16;

/// 100-ns units as a `Duration`, saturating at zero.
fn units(value: i64) -> Duration {
    Duration::from_nanos((value.max(0) as u64).saturating_mul(100))
}

/// The presentation clock: elapsed media time, frozen while paused. A plain
/// value so the headless tests can drive it with fake time; production feeds
/// it the elapsed span from a monotonic `Instant`.
#[derive(Debug, Default)]
struct Clock {
    media: Duration,
    paused: bool,
}

impl Clock {
    /// Advance by `elapsed` unless paused.
    fn advance(&mut self, elapsed: Duration) {
        if !self.paused {
            self.media = self.media.saturating_add(elapsed);
        }
    }

    /// Jump the clock, used when a seek lands: the clock restarts at the
    /// position asked for, so the sought picture is due at once.
    fn seek_to(&mut self, media: Duration) {
        self.media = media;
    }
}

/// The pacing rule, as a free function so the tests exercise exactly what the
/// window does: a decoded-ahead picture is shown when the clock reaches its
/// PTS, not a tick before.
fn due(media: Duration, pts_100ns: i64) -> bool {
    media >= units(pts_100ns)
}

/// Whether a decoded picture lies wholly before `target` — the preroll a seek
/// discards. The picture covering the target (or the first one after it) is
/// kept, which is what keeps a Right-arrow from flashing seconds-old frames.
fn frame_before(pts: i64, dur: i64, target: i64) -> bool {
    pts < target && pts.saturating_add(dur.max(0)) <= target
}

/// One picture in the two-slot buffer: the bytes, the shape, and when it is
/// due. Its `Vec` is reused in place; it is only ever grown through
/// `try_reserve`, so an allocation failure becomes a `TossError` rather than
/// an allocator abort.
#[derive(Default)]
struct Frame {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    pts: i64,
    dur: i64,
}

impl Frame {
    fn set_pixels(
        &mut self,
        data: &[u8],
        width: u32,
        height: u32,
        pts: i64,
        dur: i64,
    ) -> Result<(), TossError> {
        if self.pixels.len() < data.len() {
            let extra = data.len() - self.pixels.len();
            self.pixels.try_reserve(extra).map_err(|_| {
                TossError::other("software playback could not allocate a video frame buffer")
            })?;
        }
        self.pixels.resize(data.len(), 0);
        self.pixels.copy_from_slice(data);
        self.width = width;
        self.height = height;
        self.pts = pts;
        self.dur = dur;
        Ok(())
    }
}

/// A seek the timer is still working through. `started` records that
/// `av_seek_frame` has run, so the key handler never blocks on it.
struct PendingSeek {
    target: i64,
    started: bool,
}

/// A full-screen toggle reduced to plain data.
///
/// Staged inside the state borrow and returned whole, so **no reference or
/// pointer to the playback state** crosses out of `handle_key`: the reentrant
/// window calls run only after the borrow has ended.
struct FullscreenPlan {
    /// `true` to cover the monitor, `false` to restore `restore`.
    turning_on: bool,
    /// The saved rectangle to restore when `turning_on` is false.
    restore: (i32, i32, i32, i32),
}

/// What a key press asked for. Returned rather than acted on, so the window
/// procedure can drop every state reference before it calls anything that can
/// re-enter this procedure.
enum KeyAction {
    Consumed,
    Close,
    Fullscreen(FullscreenPlan),
    Unhandled,
}

/// Everything a running software playback owns, in the same shape as
/// `player::PlayerState`: owned by [`run`] on its stack, addressed through
/// `GWLP_USERDATA`, dropped exactly once.
struct SoftwareState {
    reader: HevcReader,
    /// The picture on screen and the one decoded ahead — the bounded pair.
    visible: Frame,
    pending: Frame,
    visible_valid: bool,
    pending_valid: bool,
    /// The decoder answered `Ok(None)`: no more pictures, only the wait for
    /// the last one's time to pass before a clean exit.
    exhausted: bool,
    clock: Clock,
    last_tick: Instant,
    /// The file's length in 100-ns units, for clamping a seek. `i64::MAX`
    /// when the container did not say, which clamps nothing.
    duration: i64,
    /// A seek still being serviced, if any.
    seeking: Option<PendingSeek>,
    saved_rect: (i32, i32, i32, i32),
    fullscreen: bool,
    /// A failure the window procedure cannot return, carried out of the
    /// loop to [`run`] — the same escape hatch `player.rs` uses.
    error: Option<TossError>,
}

/// Play `path` with the bundled software decoder, in a window.
///
/// Returns the same typed errors the rest of the media path uses: a file
/// neither FFmpeg's container nor its HEVC decoder recognises is an
/// `UnsupportedFormat` (the chain's terminal verdict), damaged bytes are a
/// `CorruptInput`, and a real internal failure is `other`.
pub(super) fn play(path: &Path) -> Result<(), TossError> {
    // SAFETY: the argument is a constant the system defines, and the prior
    // context is deliberately dropped — restoring it would undo the setting
    // for the rest of the process (the same call the viewer and MF player make).
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let reader = HevcReader::open(path)?;
    let duration = reader.duration_100ns();

    let mut state = Box::new(SoftwareState {
        reader,
        visible: Frame::default(),
        pending: Frame::default(),
        visible_valid: false,
        pending_valid: false,
        exhausted: false,
        clock: Clock::default(),
        last_tick: Instant::now(),
        duration,
        seeking: None,
        saved_rect: (0, 0, 0, 0),
        fullscreen: false,
        error: None,
    });

    // Prime the first picture before a window exists: a file whose very
    // first frame cannot be decoded needs no window at all — the same rule
    // the image viewer applies to an undecodable image (design §2). It is
    // promoted straight to visible so the first paint shows it.
    if decode_pending(&mut state)? {
        promote(&mut state);
    } else {
        state.exhausted = true;
    }
    if state.visible_valid {
        state.clock.seek_to(units(state.visible.pts));
    }
    state.last_tick = Instant::now();

    let instance = super::module(path)?;
    register_class(path, instance)?;
    let window = Window::new(path, instance)?;

    log::out(&notice(path));

    run(state, window)
}

/// The one console line this path prints before its window.
///
/// Toss's console is the textual companion, and it stays honest by the same
/// rule the image viewer follows: it lists the controls that exist and no
/// others. "silent" is not buried — this build opens no audio stream, and a
/// notice that implied otherwise would be the kind of promise §10 forbids.
fn notice(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    format!(
        "Toss — Software Video\n\n\
         File       {name}\n\
         Decoder    bundled HEVC (software, video only — silent)\n\n\
         Controls\n  \
         Space       Play / pause\n  \
         Left/Right  Seek ±5s\n  \
         F           Full screen\n  \
         Esc         Stop and close\n"
    )
}

/// A software window, created here and destroyed by its `Drop` if the loop
/// never got to it — the same safety net `player::Window` provides.
struct Window {
    handle: HWND,
}

impl Window {
    fn new(path: &Path, instance: HINSTANCE) -> Result<Self, TossError> {
        let title: HSTRING = title_of(path);
        // SAFETY: the class is registered, the title outlives the call, and
        // no parameter is a pointer the window keeps beyond creation —
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
        .map_err(|err| super::failed(path, "open a software playback window", err))?;

        Ok(Self { handle })
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: `IsWindow` is the test — a destroyed handle fails it, and
        // a live one is valid to destroy from the thread that created it.
        unsafe {
            if IsWindow(Some(self.handle)).as_bool() {
                let _ = DestroyWindow(self.handle);
            }
        }
    }
}

/// Pump messages until the window is gone, then report what it saw.
///
/// Every way out — a clean end, a playback error, a dead message queue — goes
/// through [`detach_and_destroy`] while `state` is still alive, so no message
/// can reach a freed state and a live window is never left standing.
fn run(mut state: Box<SoftwareState>, window: Window) -> Result<(), TossError> {
    // Discard a stale `WM_QUIT` left by a previous attempt's torn-down window
    // *before* attaching or showing this one. Draining after `ShowWindow`/
    // `UpdateWindow` could throw away this window's own quit.
    drain_quit();

    // SAFETY: `state` is a live box that outlives the loop below, and
    // `GWLP_USERDATA` is the slot Windows reserves for exactly this use.
    let timer = unsafe {
        SetWindowLongPtrW(
            window.handle,
            GWLP_USERDATA,
            std::ptr::from_ref(&*state).addr() as isize,
        );
        // SAFETY: the timer dies with the window; a zero return means it was
        // never installed, which is a real failure, not a detail to ignore.
        SetTimer(Some(window.handle), TIMER_DECODE, DECODE_INTERVAL_MS, None)
    };
    if timer == 0 {
        detach_and_destroy(window.handle);
        return Err(TossError::other(
            "software playback could not start its presentation timer",
        ));
    }

    // SAFETY: the window is live; these only show and focus it and mark it
    // for painting — none retains a pointer.
    unsafe {
        let _ = ShowWindow(window.handle, SW_SHOW);
        let _ = SetForegroundWindow(window.handle);
        let _ = UpdateWindow(window.handle);
    }

    let mut message = MSG::default();
    let mut failure: Option<TossError> = None;
    loop {
        // SAFETY: `message` is valid writable storage and no other loop runs
        // on this thread. Positive = a message, zero = `WM_QUIT`, negative =
        // failure — read as a number, never as a boolean.
        let arrived = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if arrived.0 < 0 {
            failure = Some(TossError::other("software playback lost its message queue"));
            break;
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

    // `state` is still alive here: detach it from possibly-live window first,
    // then destroy, so no reentrant message can touch it.
    detach_and_destroy(window.handle);

    let outcome = state.error.take().or(failure);
    drop(state);
    match outcome {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Detach the state pointer from `window` and destroy it if it is still live.
///
/// Called on every exit and error path, while the state is still alive: a
/// `WM_QUIT` can arrive with the window still standing, and dropping the
/// state before the window would leave a live procedure holding a dangling
/// address.
fn detach_and_destroy(window: HWND) {
    // SAFETY: `window` was created by this thread; clearing the slot stops
    // any later message from reaching the state, and `IsWindow` guards the
    // destroy so an already-closed window is not destroyed twice.
    unsafe {
        SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        if IsWindow(Some(window)).as_bool() {
            let _ = DestroyWindow(window);
        }
    }
}

/// Discard any `WM_QUIT` a previous window left in this thread's queue.
fn drain_quit() {
    let mut message = MSG::default();
    // SAFETY: `message` is writable storage; the filter is exactly `WM_QUIT`
    // and `PM_REMOVE` discards it — only quit messages are touched.
    unsafe { while PeekMessageW(&mut message, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool() {} }
}

/// The window procedure: paint, pace, and answer the keys.
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
        // belongs to `run`, so this detaches rather than releases.
        if message == WM_NCDESTROY {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            return DefWindowProcW(window, message, wparam, lparam);
        }

        let slot = GetWindowLongPtrW(window, GWLP_USERDATA);

        match message {
            WM_PAINT if slot != 0 => {
                let mut paint = PAINTSTRUCT::default();
                // SAFETY: `paint` is writable storage the system fills in and
                // `BeginPaint`/`EndPaint` are called as a pair on the same
                // window. `BeginPaint` runs before the state borrow so any
                // erase message it raises finds no reference held.
                let dc = BeginPaint(window, &mut paint);
                // A borrow only — painting never mutates the state, and no
                // Win32 call inside `render` can re-enter this procedure.
                let state = &*(slot as *const SoftwareState);
                render(state, window, dc);
                let _ = EndPaint(window, &paint);
                LRESULT(0)
            }
            // The pace maker. The state borrow is scoped so it is released
            // before `DestroyWindow`, which re-enters this procedure.
            WM_TIMER if slot != 0 => {
                let done = {
                    let state = &mut *(slot as *mut SoftwareState);
                    on_tick(state, window)
                };
                if done {
                    let _ = DestroyWindow(window);
                }
                LRESULT(0)
            }
            WM_KEYDOWN if slot != 0 => {
                let action = {
                    let state = &mut *(slot as *mut SoftwareState);
                    handle_key(state, window, wparam.0 as u16)
                };
                match action {
                    // Already a no-op; the arm below is for `Close`.
                    KeyAction::Consumed => LRESULT(0),
                    KeyAction::Close => {
                        let _ = DestroyWindow(window);
                        LRESULT(0)
                    }
                    // The state borrow ended with `action`; `plan` is plain
                    // data, so the reentrant calls below run with no borrow
                    // alive to alias.
                    KeyAction::Fullscreen(plan) => {
                        apply_fullscreen(window, plan);
                        LRESULT(0)
                    }
                    KeyAction::Unhandled => DefWindowProcW(window, message, wparam, lparam),
                }
            }
            // Only a window that actually ran has state attached; a window
            // destroyed before `run` (a failed creation path) must not post a
            // quit nobody is waiting for — that stale `WM_QUIT` is what would
            // break the fallback chain's *next* window.
            WM_DESTROY if slot != 0 => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            // `WM_CLOSE` — the title-bar cross — is left to `DefWindowProcW`,
            // which turns it into the destroy above.
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

/// Promote the decoded-ahead picture to visible, swapping the two buffers so
/// both allocations are reused rather than freed and remade.
fn promote(state: &mut SoftwareState) {
    std::mem::swap(&mut state.visible, &mut state.pending);
    state.visible_valid = true;
    state.pending_valid = false;
}

/// Decode the next picture into the `pending` buffer.
///
/// `Ok(true)` means `pending` now holds a picture; `Ok(false)` is the clean
/// end of the file.
fn decode_pending(state: &mut SoftwareState) -> Result<bool, TossError> {
    let Some(frame) = state.reader.next_frame()? else {
        return Ok(false);
    };
    state.pending.set_pixels(
        frame.data,
        frame.width,
        frame.height,
        frame.pts_100ns,
        frame.duration_100ns,
    )?;
    state.pending_valid = true;
    Ok(true)
}

/// Advance the clock, service any seek, then show every due picture within a
/// budget. Returns `true` when the window should close.
fn on_tick(state: &mut SoftwareState, window: HWND) -> bool {
    let now = Instant::now();
    let elapsed = now.duration_since(state.last_tick);
    state.last_tick = now;
    state.clock.advance(elapsed);

    // A pending seek is serviced first, and even while paused, so the sought
    // picture appears without resuming and Esc stays responsive throughout.
    if state.seeking.is_some() {
        return service_seek(state, window);
    }

    if state.exhausted {
        let end = units(state.visible.pts.saturating_add(state.visible.dur.max(0)));
        return state.clock.media >= end;
    }
    if state.clock.paused {
        return false;
    }

    let mut budget = MAX_FRAMES_PER_TICK;
    while budget > 0 {
        if !state.pending_valid {
            match decode_pending(state) {
                Ok(true) => {}
                Ok(false) => {
                    state.exhausted = true;
                    break;
                }
                Err(err) => {
                    state.error = Some(err);
                    return true;
                }
            }
        }
        if !due(state.clock.media, state.pending.pts) {
            break;
        }
        promote(state);
        present(state, window);
        budget -= 1;
    }

    false
}

/// Do a bounded slice of a pending seek. Returns `true` only on a fatal error.
///
/// The key handler never decodes: it records the target and returns, and this
/// runs a few preroll pictures per tick until the first picture at or after
/// the target is shown. A long GOP therefore never blocks the queue.
fn service_seek(state: &mut SoftwareState, window: HWND) -> bool {
    let (target, started) = match &state.seeking {
        Some(seek) => (seek.target, seek.started),
        None => return false,
    };

    if !started {
        if let Err(err) = state.reader.seek(target) {
            state.error = Some(err);
            state.seeking = None;
            return true;
        }
        if let Some(seek) = state.seeking.as_mut() {
            seek.started = true;
        }
    }

    let mut budget = MAX_PREROLL_PER_TICK;
    while budget > 0 {
        budget -= 1;
        match decode_pending(state) {
            Ok(true) => {}
            Ok(false) => {
                // Landed past the end: quiet EOF. Leave the last visible
                // picture on screen and let the exhausted rule close us.
                state.pending_valid = false;
                state.exhausted = true;
                state.clock.seek_to(units(target));
                state.seeking = None;
                return false;
            }
            Err(err) => {
                state.error = Some(err);
                state.seeking = None;
                return true;
            }
        }
        let pts = state.pending.pts;
        let dur = state.pending.dur.max(0);
        if frame_before(pts, dur, target) {
            continue; // preroll: decoded only to reach the target
        }
        promote(state);
        state.exhausted = false;
        state.clock.seek_to(units(target.max(pts)));
        state.seeking = None;
        present(state, window);
        return false;
    }

    false // budget spent; carry on next tick
}

/// Record a seek the timer will service. No Win32 and no decoding here.
fn request_seek(state: &mut SoftwareState, target: i64) {
    let ceiling = if state.duration > 0 {
        state.duration
    } else {
        i64::MAX
    };
    state.seeking = Some(PendingSeek {
        target: target.clamp(0, ceiling),
        started: false,
    });
}

/// Answer one key press. Returns what the window procedure should do next;
/// no window is destroyed and no long work is done inside here.
fn handle_key(state: &mut SoftwareState, window: HWND, key: u16) -> KeyAction {
    match key {
        // Space: the clock simply stops advancing while paused, so the
        // picture stays and the next press resumes from the same moment.
        k if k == VK_SPACE.0 => {
            state.clock.paused = !state.clock.paused;
            KeyAction::Consumed
        }
        // Left/Right: a fixed ±5 s step from the *visible* picture's time,
        // clamped into the file. The timer services it.
        k if k == VK_RIGHT.0 || k == VK_LEFT.0 => {
            let direction = if key == VK_RIGHT.0 { 1 } else { -1 };
            let current = if state.visible_valid {
                state.visible.pts
            } else {
                0
            };
            request_seek(
                state,
                current.saturating_add(SEEK_STEP.saturating_mul(direction)),
            );
            KeyAction::Consumed
        }
        // F: cover the screen, or hand the window back where it was. Only the
        // state is updated here; the window calls that can re-enter run later,
        // in `apply_fullscreen`, after this borrow ends.
        KEY_FULLSCREEN => {
            let turning_on = !state.fullscreen;
            let restore = state.saved_rect;
            if turning_on {
                // Reading the current geometry is a query, not a reentrant
                // call: it sends no message.
                let mut rect = RECT::default();
                // SAFETY: this reads only geometry into `rect`, which is
                // local storage.
                unsafe {
                    let _ = GetWindowRect(window, &mut rect);
                }
                state.saved_rect = (rect.left, rect.top, rect.right, rect.bottom);
                state.fullscreen = true;
            } else {
                state.fullscreen = false;
            }
            KeyAction::Fullscreen(FullscreenPlan {
                turning_on,
                restore,
            })
        }
        // Esc: stop and leave cleanly. The title-bar cross reaches the same
        // destroy through `DefWindowProcW`'s `WM_CLOSE`.
        k if k == VK_ESCAPE.0 => KeyAction::Close,
        // Up/Down: no audio in v1. Consumed silently rather than advertised
        // as a control that does nothing audible.
        k if k == VK_UP.0 || k == VK_DOWN.0 => KeyAction::Consumed,
        _ => KeyAction::Unhandled,
    }
}

/// Run the reentrant window calls for a staged full-screen toggle.
///
/// `plan` is plain data copied out of the state: **no reference to the
/// playback state is alive anywhere in this function**, so the
/// `SetWindowPos`/`SetForegroundWindow` calls — which can send window
/// messages straight back into [`window_proc`] — find no mutable borrow to
/// alias. The redraw is a plain `InvalidateRect`: the ordinary `WM_PAINT`
/// path re-reads the state through a shared borrow after every reentrant call
/// here has returned. Pure geometry — the MF path asks a renderer to switch
/// modes, but here the picture is Toss's own GDI buffer (§8.2).
fn apply_fullscreen(window: HWND, plan: FullscreenPlan) {
    // SAFETY: `window` is the live window the key arrived at; these calls set
    // geometry and z-order, retain no pointer, and run with no state borrow
    // alive.
    unsafe {
        if plan.turning_on {
            let _ = SetWindowPos(
                window,
                Some(HWND_TOPMOST),
                0,
                0,
                GetSystemMetrics(SM_CXSCREEN),
                GetSystemMetrics(SM_CYSCREEN),
                Default::default(),
            );
            let _ = SetForegroundWindow(window);
        } else {
            let (left, top, right, bottom) = plan.restore;
            let _ = SetWindowPos(
                window,
                Some(HWND_NOTOPMOST),
                left,
                top,
                right - left,
                bottom - top,
                Default::default(),
            );
        }
        // Repaint through the ordinary paint path (erase on: the letterbox
        // margins moved), which runs later and takes only a shared borrow.
        let _ = InvalidateRect(Some(window), None, false);
    }
}

/// Draw the current picture now, drawing straight to the window's DC.
///
/// Deliberately *not* `InvalidateRect` + `UpdateWindow`: the latter sends a
/// `WM_PAINT` synchronously, re-entering the window procedure while the
/// caller holds a mutable borrow of the state. Drawing here re-enters
/// nothing, so the visible picture is exactly the one the clock has reached —
/// never the decoded-ahead one in `pending`.
fn present(state: &SoftwareState, window: HWND) {
    if !state.visible_valid || state.visible.pixels.is_empty() {
        return;
    }

    // SAFETY: `window` is live for this call; `GetDC` returns a DC the paired
    // `ReleaseDC` gives back, and `render` only reads `state`.
    unsafe {
        let dc = GetDC(Some(window));
        if !dc.is_invalid() {
            render(state, window, dc);
            let _ = ReleaseDC(Some(window), dc);
        }
    }
}

/// Paint the *visible* buffer into `dc`, letterboxed, over a black client.
fn render(state: &SoftwareState, window: HWND, dc: HDC) {
    let mut client = RECT::default();
    // SAFETY: writable out-storage; the window is live for this message.
    unsafe {
        let _ = GetClientRect(window, &mut client);
    }

    // Clear the whole client black first: after a resize or a fullscreen
    // round trip the letterbox margins move, and stale video pixels there
    // must not survive. The class brush only erases newly-exposed regions,
    // which is not enough once the picture has shrunk.
    //
    // SAFETY: `dc` is valid for this call; the stock black brush is immortal.
    unsafe {
        let black = HBRUSH(GetStockObject(BLACK_BRUSH).0);
        let _ = FillRect(dc, &client, black);
    }

    if !state.visible_valid || state.visible.pixels.is_empty() {
        return;
    }

    let frame_width = state.visible.width as i32;
    let frame_height = state.visible.height as i32;
    if frame_width <= 0 || frame_height <= 0 {
        return;
    }

    let client_width = (client.right - client.left).max(1);
    let client_height = (client.bottom - client.top).max(1);

    // Aspect-preserving fit, centred: the largest rectangle of the frame's
    // shape that fits the client, with black bars where it does not.
    let scale = f64::min(
        f64::from(client_width) / f64::from(frame_width),
        f64::from(client_height) / f64::from(frame_height),
    );
    let draw_width = ((f64::from(frame_width) * scale).round() as i32).max(1);
    let draw_height = ((f64::from(frame_height) * scale).round() as i32).max(1);
    let x = (client_width - draw_width) / 2;
    let y = (client_height - draw_height) / 2;

    let info = bitmap_info(state.visible.width, state.visible.height);

    // SAFETY: `dc` came from `BeginPaint` or `GetDC` on this window and stays
    // valid for the call; `info` describes `pixels`, which lives as long as
    // the state; `StretchDIBits` reads the buffer during the call and keeps
    // no pointer.
    unsafe {
        // Averaging only when shrinking: enlarging has no samples to drop,
        // and the viewer's lesson is that HALFTONE smears an enlarged edge.
        if draw_width < frame_width || draw_height < frame_height {
            let _ = SetStretchBltMode(dc, HALFTONE);
            let _ = SetBrushOrgEx(dc, 0, 0, None);
        }
        let _ = StretchDIBits(
            dc,
            x,
            y,
            draw_width,
            draw_height,
            0,
            0,
            frame_width,
            frame_height,
            Some(state.visible.pixels.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}

/// The `BITMAPINFO` describing a BGRA8 buffer to GDI.
///
/// The negative height means top-down, the order swscale writes and GDI
/// would otherwise flip. Same header the viewer asserts directly.
fn bitmap_info(width: u32, height: u32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            biSizeImage: 0,
            ..BITMAPINFOHEADER::default()
        },
        // 32-bit BI_RGB reads no colour table; one placeholder satisfies the
        // struct's shape and nothing consults it.
        bmiColors: [RGBQUAD::default()],
    }
}

/// Install the window procedure under `CLASS_NAME`, tolerating the class
/// already existing (a second attempt in the same process).
fn register_class(path: &Path, instance: HINSTANCE) -> Result<(), TossError> {
    // SAFETY: the stock brush and system cursor are shared, immortal handles
    // the system documents as never needing release by us.
    unsafe {
        let cursor = LoadCursorW(None, IDC_ARROW)
            .map_err(|err| super::failed(path, "load the arrow cursor", err))?;
        let background = HBRUSH(GetStockObject(BLACK_BRUSH).0);
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
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
            // Already registered counts as success, read immediately after
            // the failed call — the same tolerance the MF player extends.
            if GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(TossError::other(
                    "software playback could not register its window class",
                ));
            }
        }
    }
    Ok(())
}

/// The window title: the file being played.
fn title_of(path: &Path) -> HSTRING {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    HSTRING::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A headless model of the window's promotion rule, built from the very
    /// same `Clock`, `due` and `frame_before` the window uses. It exists so
    /// the pacing, pause and lookahead behaviour is proved without Win32.
    struct Model {
        clock: Clock,
        upcoming: std::collections::VecDeque<(i64, i64)>,
        pending: Option<(i64, i64)>,
        visible_pts: Option<i64>,
        displayed: Vec<i64>,
    }

    impl Model {
        fn new(pictures: &[(i64, i64)]) -> Self {
            let mut model = Model {
                clock: Clock::default(),
                upcoming: pictures.iter().copied().collect(),
                pending: None,
                visible_pts: None,
                displayed: Vec::new(),
            };
            model.refill_pending();
            if let Some((pts, _)) = model.pending {
                model.clock.seek_to(units(pts));
            }
            model.promote();
            model
        }

        fn refill_pending(&mut self) {
            if self.pending.is_none() {
                self.pending = self.upcoming.pop_front();
            }
        }

        fn promote(&mut self) {
            while let Some((pts, _)) = self.pending {
                if !due(self.clock.media, pts) {
                    break;
                }
                self.visible_pts = Some(pts);
                self.displayed.push(pts);
                self.pending = None;
                self.refill_pending();
            }
        }

        fn tick(&mut self, elapsed: Duration) {
            self.clock.advance(elapsed);
            self.promote();
        }
    }

    #[test]
    fn units_are_one_hundred_nanoseconds() {
        assert_eq!(units(10_000_000), Duration::from_secs(1));
        assert_eq!(units(1_000_000), Duration::from_millis(100));
        assert_eq!(units(50_000_000), Duration::from_secs(5));
        assert_eq!(units(-5), Duration::ZERO);
    }

    #[test]
    fn the_clock_freezes_while_paused_and_jumps_on_seek() {
        let mut clock = Clock::default();
        clock.advance(Duration::from_millis(30));
        assert_eq!(clock.media, Duration::from_millis(30));

        clock.paused = true;
        clock.advance(Duration::from_millis(70));
        assert_eq!(
            clock.media,
            Duration::from_millis(30),
            "a paused clock must not advance"
        );

        clock.paused = false;
        clock.advance(Duration::from_millis(5));
        assert_eq!(clock.media, Duration::from_millis(35));

        clock.seek_to(Duration::from_millis(1));
        assert_eq!(clock.media, Duration::from_millis(1));
    }

    #[test]
    fn the_lookahead_is_not_shown_before_its_pts_and_pause_freezes_it() {
        // Three pictures 100 ms apart: 0, 100 ms, 200 ms.
        let mut model = Model::new(&[
            (0, 1_000_000),
            (1_000_000, 1_000_000),
            (2_000_000, 1_000_000),
        ]);

        assert_eq!(
            model.displayed,
            vec![0],
            "only the first picture is due at 0"
        );
        assert_eq!(model.visible_pts, Some(0));

        // The second picture is decoded ahead (`pending`) but must stay
        // hidden until the clock reaches it.
        assert_eq!(model.pending, Some((1_000_000, 1_000_000)));
        model.tick(Duration::from_millis(10));
        assert_eq!(model.displayed, vec![0], "10 ms is before the 100 ms PTS");
        assert_eq!(
            model.visible_pts,
            Some(0),
            "repaint keeps the visible picture"
        );

        model.tick(Duration::from_millis(90));
        assert_eq!(model.displayed, vec![0, 1_000_000], "due at 100 ms");

        // Pausing must freeze promotion even if wall time passes.
        model.clock.paused = true;
        model.tick(Duration::from_millis(5_000));
        assert_eq!(
            model.displayed,
            vec![0, 1_000_000],
            "a paused clock promotes nothing"
        );
        assert_eq!(model.visible_pts, Some(1_000_000));

        model.clock.paused = false;
        model.tick(Duration::from_millis(100));
        assert_eq!(model.displayed, vec![0, 1_000_000, 2_000_000]);
    }

    #[test]
    fn preroll_keeps_only_the_picture_covering_the_target() {
        // Wholly before the 500 ms target: discarded.
        assert!(frame_before(0, 1_000_000, 5_000_000));
        assert!(frame_before(4_500_000, 400_000, 5_000_000)); // ends at 4.9M
        assert!(frame_before(4_000_000, 1_000_000, 5_000_000)); // ends exactly at 5M
        // Covering the target, or landing on/after it: kept.
        assert!(!frame_before(4_500_000, 1_000_000, 5_000_000)); // 4.5M..5.5M covers 5M
        assert!(!frame_before(5_000_000, 1_000_000, 5_000_000)); // starts at 5M
        assert!(!frame_before(6_000_000, 1_000_000, 5_000_000)); // starts after it
    }

    #[test]
    fn growing_a_frame_buffer_reserves_rather_than_aborts() {
        let mut frame = Frame::default();
        frame
            .set_pixels(&[0u8; 16], 2, 2, 0, 1_000_000)
            .expect("small buffer");
        assert_eq!(frame.pixels.len(), 16);
        // Shrinking then growing reuses the same allocation without error.
        frame
            .set_pixels(&[0u8; 4], 1, 1, 1, 1_000_000)
            .expect("shrink");
        assert_eq!(frame.pixels.len(), 4);
        frame
            .set_pixels(&[7u8; 64], 4, 4, 2, 1_000_000)
            .expect("grow again");
        assert_eq!(frame.pixels.len(), 64);
        assert!(frame.pixels.iter().all(|&byte| byte == 7));
    }

    /// The direct software path, on a real desktop.
    ///
    /// Ignored rather than run by default: it opens a window and pumps a
    /// message loop until the one-second fixture ends, which no headless CI
    /// runner can do (the same reason the MF lifecycle test is ignored).
    /// There is no env var or CLI flag that selects software playback in
    /// production — this entry calls the function the fallback chain calls,
    /// so what it proves is the real path with no override.
    #[test]
    #[ignore = "opens a real window: run on a desktop with `cargo test -- --ignored`"]
    fn software_path_plays_the_hevc_fixture_to_its_end() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("corpus")
            .join("media")
            .join("hevc.mkv");

        let outcome = play(&fixture);
        assert!(outcome.is_ok(), "software playback failed: {outcome:?}");
    }
}
