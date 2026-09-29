//! The window and its message loop — milestone 1, and nothing more.
//!
//! Create, show, pump, destroy. What is drawn *inside* arrives with the decode
//! and render milestones, which is why this file holds no pixel state: a
//! window with nothing to show needs no viewport, and inventing one now would
//! be the speculative abstraction §42 warns about (`IMAGE_VIEWER.md` §11).
//!
//! Every Windows type it touches stays here. What leaves this module is
//! `Result<(), TossError>` and nothing else.

use std::mem::size_of;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, GetSysColorBrush, UpdateWindow};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW,
    GetMessageW, IDC_ARROW, LoadCursorW, MSG, PostQuitMessage, RegisterClassExW, SW_SHOW,
    ShowWindow, TranslateMessage, WM_DESTROY, WNDCLASS_STYLES, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{HSTRING, PCWSTR, w};

use crate::core::error::TossError;
use crate::platform::image::ViewRequest;

/// Class name for every window this process creates.
const CLASS_NAME: PCWSTR = w!("TossImageViewer");

/// Show `request`, run until the window closes, and report how it went.
pub fn run(request: &ViewRequest) -> Result<(), TossError> {
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

    // SAFETY: the class is registered, the title outlives the call, and no
    // parameter here is a pointer the window keeps beyond creation.
    let window = unsafe {
        CreateWindowExW(
            Default::default(),
            CLASS_NAME,
            &title,
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|err| super::failed("open a window", err))?;

    // SAFETY: `window` was just handed to us by the successful create above,
    // so it is live for exactly these calls. Both return a BOOL that reports
    // prior state rather than success, so there is nothing to act on.
    unsafe {
        let _ = ShowWindow(window, SW_SHOW);
        let _ = UpdateWindow(window);
    }

    pump()?;

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
/// Almost nothing is handled here on purpose. `WM_CLOSE` — the title-bar cross,
/// Alt+F4 — is deliberately left to `DefWindowProcW`, which turns it into a
/// destroy, which posts the `WM_DESTROY` handled below; repeating that chain
/// by hand would be the same rule written twice.
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: the system calls this with a live window for the whole call,
    // and neither parameter is dereferenced here.
    unsafe {
        if message == WM_DESTROY {
            PostQuitMessage(0);
            return LRESULT(0);
        }

        DefWindowProcW(window, message, wparam, lparam)
    }
}
