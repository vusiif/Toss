//! The window, its state and its message loop.
//!
//! M1 built the window and M2 could decode; this is the first milestone that
//! puts pixels on screen. What the window procedure touches is a borrow of
//! [`ViewerState`], never its owner — the ownership story is in one place
//! below, because a `HWND` slot is exactly where a double free would hide.

use std::mem::size_of;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, COLOR_WINDOW, DIB_RGB_COLORS, EndPaint,
    GetSysColorBrush, HDC, InvalidateRect, PAINTSTRUCT, RGBQUAD, SRCCOPY, StretchDIBits,
    UpdateWindow,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_LEFT, VK_RIGHT};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW, GetWindowLongPtrW, IDC_ARROW,
    LoadCursorW, MSG, PostQuitMessage, RegisterClassExW, SW_SHOW, SWP_NOMOVE, SWP_NOZORDER,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage,
    WM_CAPTURECHANGED, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WNDCLASS_STYLES, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{HSTRING, PCWSTR, w};

use super::decode::Decoded;
use crate::core::error::TossError;
use crate::platform::image::{ViewRequest, nav, pan, zoom};

/// Class name for every window this process creates.
const CLASS_NAME: PCWSTR = w!("TossImageViewer");

/// What belongs to *this* window.
///
/// Owned by [`run`] on its stack; the `GWLP_USERDATA` slot holds an address
/// of that box and nothing more. Freeing it is `run`'s job on every path out,
/// which is what makes the three ways a window can end — creation refused,
/// the loop failing, the window closing — all reduce to "one box, one drop".
///
/// `zoom` is the *cursor* into the ladder Toss defines (`platform::image`);
/// the ladder itself is a rule and does not live here. `origin` is the cursor
/// into the legal range Toss defines for panning; the range itself is a rule
/// and does not live here either. That is the split `IMAGE_VIEWER.md` §4
/// draws: the wheel and the mouse belong to this half, the levels and the
/// edges they move between do not.
struct ViewerState {
    image: Decoded,
    /// The whole browsing session, and where in it the window is. The set
    /// and its order were decided before any window existed
    /// (`handlers::image`); what lives here is only the cursor into them,
    /// moved by the arrow keys through `nav`'s rule.
    request: ViewRequest,
    zoom: usize,
    /// Where the picture's top-left corner sits in client coordinates.
    /// Zero at start and whenever the picture fits; the range it is allowed
    /// to reach is `pan::clamp`'s answer, not this struct's guess.
    origin: (i32, i32),
    /// Present only while the left button is held. The single source of truth
    /// for "a drag is happening": `SetCapture` is what makes the moves arrive,
    /// but this is what decides they mean anything.
    drag: Option<Drag>,
}

/// One drag in progress: where the button went down, and where the picture
/// was at that moment.
///
/// Both ends are kept rather than only the starting point, so a move is the
/// distance travelled added to where the drag *began*. Recomputing from the
/// previous message instead would drift by whatever the queue dropped
/// between two mouse moves — the picture would slip away from under the
/// cursor instead of staying pinned to it.
#[derive(Clone, Copy)]
struct Drag {
    at: (i32, i32),
    origin: (i32, i32),
}

/// Show `image` in a native window and run until it closes.
pub fn run(request: &ViewRequest, image: Decoded) -> Result<(), TossError> {
    // Before anything that could depend on it: the API only accepts the
    // setting ahead of first use, so it has to be first. A failure is not
    // fatal — a manifest may already have set it, and the window then simply
    // inherits whatever the process was given (IMAGE_VIEWER.md §2).
    //
    // SAFETY: the argument is a constant the system defines, and the returned
    // pointer is the previous context — deliberately dropped, because restoring
    // it would undo the setting for the rest of the process.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    let instance = module()?;
    let title = title_of(request);

    register_class(instance)?;

    // The single allocation this viewer makes for its own bookkeeping. It
    // lives on this stack frame for the whole message loop, so every address
    // the window is later handed stays valid until nothing can read it.
    let state = Box::new(ViewerState {
        image,
        request: request.clone(),
        zoom: zoom::START,
        origin: (0, 0),
        drag: None,
    });
    let (width, height) = outer_size(&state.image)?;

    // SAFETY: the class is registered, the title outlives the call, and no
    // parameter here is a pointer the window keeps beyond creation — the
    // state is attached separately, below.
    let window = unsafe {
        CreateWindowExW(
            Default::default(),
            CLASS_NAME,
            &title,
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            width,
            height,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|err| super::failed("open a window", err))?;

    // The window is told where the state lives *after* it exists, rather than
    // through creation data and `WM_NCCREATE`, because nothing that reads the
    // slot can run before this line: any message the system sent during
    // `CreateWindowExW` found an empty slot and fell through to the default
    // handling, and `ShowWindow` — the first call that can ask for a paint —
    // is next. One attachment point instead of two is one fewer place for the
    // two to disagree.
    //
    // SAFETY: `state` is a live box that outlives the loop below, and
    // `GWLP_USERDATA` is the slot Windows reserves for exactly this use.
    unsafe {
        SetWindowLongPtrW(
            window,
            GWLP_USERDATA,
            std::ptr::from_ref(&*state).addr() as isize,
        );
    }

    // SAFETY: `window` was just handed to us by the successful create above,
    // so it is live for exactly these calls. Both return a BOOL that reports
    // prior state rather than success, so there is nothing to act on.
    unsafe {
        let _ = ShowWindow(window, SW_SHOW);
        let _ = UpdateWindow(window);
    }

    if let Err(err) = pump() {
        // The loop stopped for its own reason, not because the window went
        // away, so the slot still points into `state`. Detach it before
        // `state` drops — the window is torn down by process exit, and there
        // is no further loop to service a paint against a freed address.
        //
        // SAFETY: `window` is the handle the create above returned.
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
        }
        return Err(err);
    }

    // The class is deliberately not unregistered: the process is about to
    // return, and dragging `UnregisterClassW` in to "tidy up" would widen the
    // Windows surface for nothing (IMAGE_VIEWER.md §6).
    Ok(())
}

/// The current instance, as the `HINSTANCE` window creation wants.
fn module() -> Result<HINSTANCE, TossError> {
    // SAFETY: a null module name asks for the calling process, which always
    // has one.
    let module =
        unsafe { GetModuleHandleW(None) }.map_err(|err| super::failed("find itself", err))?;

    Ok(HINSTANCE(module.0))
}

/// The window title: the file being viewed, so a taskbar of one says what it
/// is showing.
fn title_of(request: &ViewRequest) -> HSTRING {
    let name = request
        .current()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Toss".to_owned());

    HSTRING::from(name)
}

/// The outer window size that leaves the client area exactly `image`-sized.
///
/// `CreateWindowExW`'s width and height are the *outer* rectangle, so the
/// frame would eat the image's bottom and right edge if they were passed
/// through as they stand. This asks the system for the outer size that yields
/// the client size wanted — arithmetic about the frame, not fit logic: the
/// image is still drawn one to one and nothing is scaled (IMAGE_VIEWER.md
/// §11).
///
/// Asked once more whenever browsing lands on a picture of a different size
/// (`browse`), so the client rectangle keeps matching the picture it shows.
/// Fit-*to*-window — shrinking the frame around a picture larger than the
/// screen — remains §11's "not yet".
fn outer_size(image: &Decoded) -> Result<(i32, i32), TossError> {
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: image.width as i32,
        bottom: image.height as i32,
    };

    // SAFETY: `frame` is writable storage the system fills in; the style and
    // extended style describe the window being created and there is no menu.
    unsafe { AdjustWindowRectEx(&mut frame, WS_OVERLAPPEDWINDOW, false, Default::default()) }
        .map_err(|err| super::failed("size the window", err))?;

    Ok((frame.right - frame.left, frame.bottom - frame.top))
}

/// Install the window procedure under `CLASS_NAME`.
///
/// The cursor is the system arrow rather than a null handle, because a window
/// that reports "no idea what the mouse is doing over me" is a window people
/// think has frozen.
fn register_class(instance: HINSTANCE) -> Result<(), TossError> {
    // SAFETY: the system cursor is a shared, immortal handle; nothing here
    // owns it.
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }
        .map_err(|err| super::failed("load the arrow cursor", err))?;

    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
        lpfnWndProc: Some(window_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: Default::default(),
        hCursor: cursor,
        // SAFETY: `GetSysColorBrush` only indexes a table the system owns and
        // documents as returning an immortal handle.
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        lpszMenuName: PCWSTR::null(),
        lpszClassName: CLASS_NAME,
        hIconSm: Default::default(),
    };

    // SAFETY: every field above is initialised, `lpszClassName` and `hInstance`
    // outlive the call, and `lpfnWndProc` names a real function — the whole
    // point of building the struct here rather than assembling it elsewhere.
    let registered = unsafe { RegisterClassExW(&class) };

    // Zero on failure. The reason lives in the thread's last error, but a
    // class registration can only fail for reasons the process cannot act on
    // (§23: an understandable message rather than a silent zero).
    if registered == 0 {
        return Err(TossError::other(
            "image viewer could not register its window class",
        ));
    }

    Ok(())
}

/// Run until the window says it is gone.
///
/// `GetMessageW` answers with more than a truth value: positive for a message,
/// zero for `WM_QUIT`, negative for failure. Reading it as a boolean would
/// take the negative case as "keep going" and spin forever on an error.
fn pump() -> Result<(), TossError> {
    let mut message = MSG::default();

    loop {
        // SAFETY: `message` is valid writable storage for the call, and no
        // other message loop runs in this process.
        let arrived = unsafe { GetMessageW(&mut message, None, 0, 0) };

        if arrived.0 < 0 {
            return Err(TossError::other("image viewer lost its message queue"));
        }
        if arrived.0 == 0 {
            return Ok(());
        }

        // SAFETY: `message` holds a message `GetMessageW` just filled in and
        // the call does not retain it.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// The window procedure.
///
/// It borrows [`ViewerState`] through the `GWLP_USERDATA` slot and never
/// frees it — the slot is cleared when the window dies so nothing afterwards
/// can read a stale address, and the box itself is dropped by [`run`], once,
/// whether the window opened, failed to open or closed.
///
/// Each arm takes its own reference and lets go before anything that could
/// re-enter this procedure. `DefWindowProcW` is free to send messages back at
/// the same window, so a borrow held across it would be a second reference to
/// the same state — and with a mutable one, undefined behaviour.
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the system calls this with a live window for the whole call,
    // neither plain parameter is dereferenced, and the slot read below only
    // ever holds an address `run` put there — `run` outlives every message
    // this procedure can be called for.
    unsafe {
        // The window is going away. Leave no address behind for a later
        // message to find; the state belongs to `run`, so this detaches
        // rather than releases.
        if message == WM_NCDESTROY {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            return DefWindowProcW(window, message, wparam, lparam);
        }

        let slot = GetWindowLongPtrW(window, GWLP_USERDATA);

        match message {
            WM_PAINT if slot != 0 => {
                let state = &*(slot as *const ViewerState);
                let mut paint = PAINTSTRUCT::default();

                // SAFETY: `paint` is writable storage the system fills in and
                // `BeginPaint`/`EndPaint` are called as a pair on the same
                // window, which is the whole of their contract.
                let dc = BeginPaint(window, &mut paint);
                render(state, dc);
                let _ = EndPaint(window, &paint);

                LRESULT(0)
            }

            WM_MOUSEWHEEL if slot != 0 => {
                let state = &mut *(slot as *mut ViewerState);

                // The wheel's distance is the *high* word of `wparam` and it
                // is signed: read the low half instead, or read the word as
                // unsigned, and a scroll down becomes a scroll up.
                let wheel = (wparam.0 >> 16) as u16 as i16;

                let next = if wheel > 0 {
                    zoom::step_up(state.zoom)
                } else if wheel < 0 {
                    zoom::step_down(state.zoom)
                } else {
                    state.zoom
                };

                if next != state.zoom {
                    state.zoom = next;

                    // The ladder moved, so the range the origin was clamped
                    // into moved with it: shrinking can leave the picture
                    // smaller than the position it was dragged to, and the
                    // edge the window would then show is background. Read it
                    // back against the size it has *now*, before anything
                    // paints.
                    let content = drawn_size(&state.image, zoom::factor(state.zoom));
                    state.origin = pan::clamp(state.origin, content, client_size(window));

                    // SAFETY: the window is live; a null rectangle means the
                    // whole client area, which is exactly what a zoom
                    // changes — and the erase flag clears what the smaller
                    // picture no longer covers.
                    let _ = InvalidateRect(Some(window), None, true);
                }

                LRESULT(0)
            }

            WM_LBUTTONDOWN if slot != 0 => {
                let state = &mut *(slot as *mut ViewerState);

                // Both halves are read now rather than at the first move: a
                // drag is measured from where the button went down, so a
                // queue that coalesces the first few moves cannot lose them.
                state.drag = Some(Drag {
                    at: point_of(lparam),
                    origin: state.origin,
                });

                // The mouse is captured so the moves — and the release —
                // keep arriving even when the cursor leaves the window. That
                // is the whole difference between a drag that follows the
                // picture past the edge and one that silently stops at it.
                //
                // SAFETY: `window` is the window this message was delivered
                // to, so it is live; the return is the window that held the
                // mouse before this call, which nothing here has a use for.
                let _ = SetCapture(window);

                LRESULT(0)
            }

            WM_MOUSEMOVE if slot != 0 => {
                let state = &mut *(slot as *mut ViewerState);

                // Not every move is a drag — the mouse also just travels
                // over the window — and a move without a button held has
                // nothing to add to the origin.
                let Some(drag) = state.drag else {
                    return LRESULT(0);
                };

                let cursor = point_of(lparam);
                let delta = (cursor.0 - drag.at.0, cursor.1 - drag.at.1);
                let content = drawn_size(&state.image, zoom::factor(state.zoom));
                let next = pan::dragged(drag.origin, delta, content, client_size(window));

                if next != state.origin {
                    state.origin = next;

                    // SAFETY: the window is live and the rectangle is null
                    // for the whole client area, which is what a drag can
                    // touch anywhere. The erase flag is kept from the zoom
                    // path deliberately: an origin can move a picture that
                    // did not fill the window, and the strip it leaves
                    // behind has to be cleared rather than smeared.
                    let _ = InvalidateRect(Some(window), None, true);
                }

                LRESULT(0)
            }

            WM_LBUTTONUP if slot != 0 => {
                let state = &mut *(slot as *mut ViewerState);
                state.drag = None;

                // SAFETY: this releases whatever the calling thread has
                // captured — the same thread the `SetCapture` above runs
                // on — and releasing a mouse nobody holds is a documented
                // no-op rather than an error.
                let _ = ReleaseCapture();

                LRESULT(0)
            }

            WM_CAPTURECHANGED if slot != 0 => {
                // The mouse was taken away — another window, a modal dialog,
                // the system on Alt+Tab. The drag is over whether or not a
                // button-up ever arrives, and leaving the state set would
                // make the *next* move over this window drag the picture
                // with no button held at all.
                let state = &mut *(slot as *mut ViewerState);
                state.drag = None;

                LRESULT(0)
            }

            WM_KEYDOWN if slot != 0 => {
                let state = &mut *(slot as *mut ViewerState);

                // Only the two arrows are Toss's — §18.3 asks for
                // previous/next and nothing more — and every other key keeps
                // the system's handling, because a viewer that swallows the
                // keys it does not understand breaks whatever feature needs
                // them next.
                let forward = match u16::try_from(wparam.0) {
                    Ok(key) if key == VK_RIGHT.0 => true,
                    Ok(key) if key == VK_LEFT.0 => false,
                    _ => return DefWindowProcW(window, message, wparam, lparam),
                };

                browse(state, window, forward);

                LRESULT(0)
            }

            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }

            // `WM_CLOSE` — the title-bar cross, Alt+F4 — is deliberately left
            // to `DefWindowProcW`, which turns it into a destroy, which posts
            // the `WM_DESTROY` handled above. Repeating that chain by hand
            // would be the same rule written twice.
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

/// Move the viewer to the image beside this one, in `forward`'s direction.
///
/// Everything that changes together changes here: the index, the pixels, the
/// zoom level, the pan origin, the window's size and the title. They are one
/// step rather than six because they are one *change of subject* — a viewer
/// showing the previous picture with the current picture's zoom, origin and
/// title would be showing a mixture of two images.
///
/// Nothing is reported to the caller because there is nowhere to report to:
/// the window procedure answers a message, and `nav` has already decided
/// that a set of one image simply stays where it is. A picture that refuses
/// to decode is left alone rather than replacing what is on screen — the
/// error channel for a failure *while browsing* belongs to M7, and until it
/// exists the safe answer is to show nothing new rather than something
/// wrong.
fn browse(state: &mut ViewerState, window: HWND, forward: bool) {
    let count = state.request.images.len();
    let index = if forward {
        nav::next(state.request.index, count)
    } else {
        nav::previous(state.request.index, count)
    };

    // A set of one lands on itself, and so does a set of nothing. In both
    // cases the window already shows everything there is to show.
    if index == state.request.index {
        return;
    }

    // The path is taken by value: the decode below runs before anything on
    // `state` may be written, and holding a borrow across that would be a
    // borrow of the whole request (§ the window procedure's own rule about
    // references and what may re-enter).
    let Some(path) = state.request.images.get(index).cloned() else {
        return;
    };
    let Ok(image) = super::decode::decode(&path) else {
        return;
    };

    state.request.index = index;
    state.image = image;
    state.zoom = zoom::START;
    state.origin = (0, 0);
    state.drag = None;

    // The window was sized around the picture it opened on, and the next
    // picture is under no obligation to be that size: without this, a small
    // image sits in the corner of a large one's window and a large one is
    // cropped by a frame sized for something else. Keeping the position
    // (`SWP_NOMOVE`) and the stacking order (`SWP_NOZORDER`) means only the
    // client rectangle follows the picture — which is the M3 invariant every
    // calculation in this file still assumes.
    if let Ok((width, height)) = outer_size(&state.image) {
        // SAFETY: `window` is live for the whole message loop, and the
        // flags say the position and z-order arguments are ignored, so the
        // size is the only thing being asked for.
        unsafe {
            let _ = SetWindowPos(window, None, 0, 0, width, height, SWP_NOMOVE | SWP_NOZORDER);
        }
    }

    // The title names what the window is showing, so it has to follow the
    // picture instead of the one it opened on.
    let title = title_of(&state.request);

    // SAFETY: `window` is live and `title` outlives the call — it is built
    // from a path the state owns.
    unsafe {
        let _ = SetWindowTextW(window, &title);
    }

    // SAFETY: `window` is live; a null rectangle means the whole client
    // area, which is what a new picture replaces everywhere at once.
    unsafe {
        let _ = InvalidateRect(Some(window), None, true);
    }
}

/// Draw `state`'s pixels into `dc` at the current zoom and pan origin.
///
/// The *source* rectangle is always the whole image and the *destination* is
/// its scaled size starting at the origin — which is the whole of what
/// zooming and panning are here: GDI does the interpolation, Toss supplies
/// the ladder and the edges. Content past the client edge is clipped by the
/// device context, so an origin of zero is M3's picture exactly, and a
/// negative one is that same picture slid under the window's corner with
/// nothing but the frame of the window hiding what went past.
fn render(state: &ViewerState, dc: HDC) {
    let image = &state.image;
    let source = (image.width as i32, image.height as i32);
    let destination = drawn_size(image, zoom::factor(state.zoom));
    let info = bitmap_info(image);

    // SAFETY: `dc` came from the `BeginPaint` a line above and stays valid
    // until `EndPaint`; `info` describes `pixels`, which live as long as
    // `state`, which lives as long as the loop; `StretchDIBits` reads the
    // buffer during the call and does not keep it.
    unsafe {
        // Zero means nothing was drawn. M4 has no recovery to attempt and no
        // way to report one that a person would see — the pixel probe is what
        // notices, and M7 gives this a real failure path rather than a
        // swallowed return code.
        let _ = StretchDIBits(
            dc,
            state.origin.0,
            state.origin.1,
            destination.0,
            destination.1,
            0,
            0,
            source.0,
            source.1,
            Some(image.pixels.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}

/// The client-space point a mouse message carries in `lparam`.
///
/// The coordinates travel in the low half as an x and the next half as a y,
/// each a *signed* 16-bit value: a drag to the left of the origin is
/// negative, and reading either word as unsigned turns it into a point tens
/// of thousands of pixels away — which is how a picture jumps off to the
/// right the first time someone drags it left.
fn point_of(lparam: LPARAM) -> (i32, i32) {
    (
        lparam.0 as u16 as i16 as i32,
        (lparam.0 >> 16) as u16 as i16 as i32,
    )
}

/// The window's client size, in pixels, as the picture's legal range needs
/// it.
///
/// Asked at the moment a drag or a zoom needs it rather than cached, because
/// the window can be resized between two events and a stale viewport would
/// settle the origin somewhere the current one does not allow. A failed call
/// leaves the rectangle at zero, which clamps the origin to nothing to pan —
/// the safe direction, rather than a panic (§23).
fn client_size(window: HWND) -> (i32, i32) {
    let mut rect = RECT::default();

    // SAFETY: `rect` is writable storage the call fills in, and `window` is
    // the live window the message this is being answered for arrived for.
    unsafe {
        let _ = GetClientRect(window, &mut rect);
    }

    (rect.right - rect.left, rect.bottom - rect.top)
}

/// The size an image is drawn at, in client pixels.
///
/// Rounded, never zero and never past what an `i32` device coordinate holds:
/// the scale is a `f64` and the dimensions came from a file, so this is
/// arithmetic on numbers Toss did not choose (§23). `drawn_size(image, 1.0)`
/// is the image's own size — M3's behaviour, unchanged at the default level.
fn drawn_size(image: &Decoded, scale: f64) -> (i32, i32) {
    let side = |length: u32| -> i32 {
        (f64::from(length) * scale)
            .round()
            .clamp(1.0, f64::from(i32::MAX)) as i32
    };

    (side(image.width), side(image.height))
}

/// The `BITMAPINFO` that describes `image`'s buffer to GDI.
///
/// Its own function because the header is the part where a wrong sign or a
/// wrong bit count produces a garbled picture with no error anywhere — so it
/// is asserted directly rather than inferred from a paint that appeared to
/// work.
fn bitmap_info(image: &Decoded) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: image.width as i32,
            // Negative means top-down, which is the order WIC handed the
            // pixels over in. A positive height makes GDI read the buffer
            // from the bottom up, and the picture arrives flipped without
            // anything failing.
            biHeight: -(image.height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            // Legal to leave zero for `BI_RGB`. The length that matters is
            // the buffer's, checked against these dimensions in the test
            // below rather than restated in the header.
            biSizeImage: 0,
            ..BITMAPINFOHEADER::default()
        },
        // 32-bit `BI_RGB` reads no colour table, so one placeholder entry
        // satisfies the struct's shape and nothing consults it.
        bmiColors: [RGBQUAD::default()],
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;
    use std::path::{Path, PathBuf};

    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::Graphics::Gdi::BITMAPINFOHEADER;

    use super::super::decode::{Decoded, decode};
    use super::{bitmap_info, drawn_size, outer_size, point_of};
    use crate::platform::image::{pan, zoom};

    fn sample(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("corpus")
            .join("image")
            .join(name)
    }

    /// The two ends of the range that matter: an odd, tiny image where a
    /// stride mistake is most likely, and the one a person looks at.
    const CASES: [(&str, u32, u32); 2] = [("odd.png", 7, 5), ("panel.png", 320, 200)];

    #[test]
    fn the_header_describes_the_buffer_it_is_given() {
        for (name, width, height) in CASES {
            let image = decode(&sample(name)).unwrap_or_else(|err| panic!("{name}: {err}"));
            let header = bitmap_info(&image).bmiHeader;

            assert_eq!(header.biSize as usize, size_of::<BITMAPINFOHEADER>());
            assert_eq!(header.biWidth, width as i32, "{name}: width");
            assert_eq!(
                header.biHeight,
                -(height as i32),
                "{name}: a positive height means GDI reads the buffer upside down"
            );
            assert_eq!(header.biPlanes, 1, "{name}");
            assert_eq!(header.biBitCount, 32, "{name}: WIC hands over PBGRA");

            // What GDI will actually read: `abs(biHeight)` rows of
            // `width * 4` bytes. A buffer of any other length means the
            // header and the pixels have drifted apart, which is the one
            // bug this pairing is here to catch.
            assert_eq!(
                image.pixels.len(),
                (width * height * 4) as usize,
                "{name}: the buffer is not the size the header describes"
            );
        }
    }

    #[test]
    fn the_window_is_bigger_than_the_picture_it_shows() {
        // `CreateWindowExW` takes an outer rectangle. Passing the image's
        // own size through would leave the client area smaller than the
        // picture and silently clip its bottom and right edge — a bug that
        // looks like a rendering problem rather than a sizing one.
        let image = decode(&sample("odd.png")).expect("the sample decodes");
        let (width, height) = outer_size(&image).expect("the frame can be measured");

        assert!(
            width > 7,
            "the horizontal frame was not accounted for: {width}"
        );
        assert!(
            height > 5,
            "the vertical frame was not accounted for: {height}"
        );
    }

    #[test]
    fn zooming_scales_the_destination_and_leaves_the_source_alone() {
        // The whole of what a zoom is: the destination rectangle moves and
        // the source stays the whole image, so nothing is cropped before GDI
        // sees it — cropping at the client edge is panning's rule to decide
        // (M5), and this test is about neither.
        let image = decode(&sample("odd.png")).expect("the sample decodes");

        assert_eq!(
            drawn_size(&image, 1.0),
            (7, 5),
            "the default level has to be M3's behaviour exactly"
        );
        assert_eq!(
            drawn_size(&image, 0.25),
            (2, 1),
            "7*0.25 rounds to 2, 5*0.25 rounds to 1"
        );
        assert_eq!(drawn_size(&image, 4.0), (28, 20));
    }

    #[test]
    fn the_limit_the_rule_allows_is_where_the_picture_edge_lands() {
        // Two halves that have to agree: `pan::clamp` decides how far the
        // origin may travel, and `drawn_size` is what produced the content
        // size it is handed. If they ever disagree about that size, the
        // limit either stops short — a strip of picture nobody can reach —
        // or runs past it and shows background behind the edge.
        //
        // At the default level of the larger sample: 320x200 of picture in a
        // 40x30 window, so 280x170 of travel.
        let image = decode(&sample("panel.png")).expect("the sample decodes");
        let content = drawn_size(&image, zoom::factor(zoom::START));
        let viewport = (40, 30);

        assert_eq!(content, (320, 200), "the sample's own size at 1:1");

        let origin = pan::clamp((-10_000, -10_000), content, viewport);

        assert_eq!(
            origin,
            (viewport.0 - content.0, viewport.1 - content.1),
            "the far edge of the picture has to land exactly on the far edge of the window"
        );
        assert_eq!(origin.0 + content.0, viewport.0, "x: edge meets edge");
        assert_eq!(origin.1 + content.1, viewport.1, "y: edge meets edge");
    }

    #[test]
    fn a_mouse_message_carries_signed_client_coordinates() {
        // Both halves of `lparam` are signed 16-bit positions, and getting
        // the packing wrong is invisible until someone drags left of the
        // origin: read as unsigned, -5 becomes 65531 and the picture jumps
        // off the right-hand side of the screen.
        let pack = |x: i16, y: i16| LPARAM((x as u16 as isize) | ((y as u16 as isize) << 16));

        assert_eq!(point_of(pack(0, 0)), (0, 0));
        assert_eq!(point_of(pack(300, 200)), (300, 200));
        assert_eq!(
            point_of(pack(-5, -7)),
            (-5, -7),
            "up and to the left of the client origin stays negative"
        );
        assert_eq!(point_of(pack(7, -1)), (7, -1));
    }

    #[test]
    fn a_scale_that_would_draw_nothing_still_draws_one_pixel() {
        // Rounding a small side at the smallest level reaches zero, and
        // asking GDI for a zero-wide rectangle is asking for a picture that
        // never appears — with no error to say so.
        let tiny = Decoded {
            width: 1,
            height: 1,
            pixels: vec![0, 0, 0, 255],
        };

        assert_eq!(drawn_size(&tiny, 0.25), (1, 1));
    }

    #[test]
    fn a_scale_beyond_a_device_coordinate_stops_at_the_largest_one() {
        // The dimensions came from a file and the scale from the ladder, so
        // a huge image at four times size would ask for coordinates an `i32`
        // device position cannot hold. No panic, and no wrap into a negative
        // rectangle either (§23).
        let huge = Decoded {
            width: u32::MAX / 4,
            height: u32::MAX / 4,
            pixels: Vec::new(),
        };

        assert_eq!(drawn_size(&huge, 4.0), (i32::MAX, i32::MAX));
    }
}
