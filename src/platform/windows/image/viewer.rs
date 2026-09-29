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
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DispatchMessageW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, IDC_ARROW, LoadCursorW, MSG,
    PostQuitMessage, RegisterClassExW, SW_SHOW, SetWindowLongPtrW, ShowWindow, TranslateMessage,
    WM_DESTROY, WM_MOUSEWHEEL, WM_NCDESTROY, WM_PAINT, WNDCLASS_STYLES, WNDCLASSEXW,
    WS_OVERLAPPEDWINDOW,
};
use windows::core::{HSTRING, PCWSTR, w};

use super::decode::Decoded;
use crate::core::error::TossError;
use crate::platform::image::{ViewRequest, zoom};

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
/// the ladder itself is a rule and does not live here. That is the split
/// `IMAGE_VIEWER.md` §4 draws: the wheel event belongs to this half, the
/// levels it moves between do not.
struct ViewerState {
    image: Decoded,
    zoom: usize,
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
        zoom: zoom::START,
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
/// §11). Growing the window around a very large image is M4's question.
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

                    // SAFETY: the window is live; a null rectangle means the
                    // whole client area, which is exactly what a zoom
                    // changes — and the erase flag clears what the smaller
                    // picture no longer covers.
                    let _ = InvalidateRect(Some(window), None, true);
                }

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

/// Draw `state`'s pixels into `dc` at the current zoom, anchored at the top
/// left.
///
/// The *source* rectangle is always the whole image and the *destination* is
/// its scaled size, which is the whole of what zooming is here: GDI does the
/// interpolation, Toss supplies the ladder. Content past the client edge is
/// clipped — panning is M5, and inventing a viewport now would be the §42
/// abstraction this milestone is supposed to avoid.
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
            0,
            0,
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

    use windows::Win32::Graphics::Gdi::BITMAPINFOHEADER;

    use super::super::decode::{Decoded, decode};
    use super::{bitmap_info, drawn_size, outer_size};

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
        // sees it — cropping at the client edge is panning's problem (M5).
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
