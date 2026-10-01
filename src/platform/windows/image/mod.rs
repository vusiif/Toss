//! The Windows implementation of the image-viewing capability.

mod decode;
mod viewer;

use windows::Win32::System::Console::{GetConsoleProcessList, GetConsoleWindow};
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
use windows::core::Error;

use crate::core::error::TossError;
use crate::platform::image::ViewRequest;

/// Show `request` in a native window.
///
/// The request arrives already decided — which images, which one — so nothing
/// here re-derives it (`IMAGE_VIEWER.md` §4). What this half owns is the
/// window it is shown in, and whether there is anything to show in it.
pub fn view(request: &ViewRequest) -> Result<(), TossError> {
    let Some(path) = request.current() else {
        return Err(TossError::other("image viewer was given no image to open"));
    };

    // Decode before the window exists (§3): an image that cannot be opened
    // needs no window at all. What comes back is pixels Toss owns — every WIC
    // object behind it has already been released — so the renderer consumes a
    // plain buffer and the two halves stay separable: WIC decodes, GDI
    // renders, and neither holds the other's types (IMAGE_VIEWER.md §2).
    let image = decode::decode(path)?;

    // The decode succeeded, so this run is going to open a window: this is
    // the viewer path, and the only place the console may be taken away.
    // Deciding here rather than at start-up is what keeps a failure *before*
    // this line — a broken picture, a bad command line — visible on the
    // console the user was given, and keeps every CLI path (`extract`,
    // `pack`, the info fallback) from having its console touched at all.
    hide_owned_console();

    viewer::run(request, image)
}

/// Hide the console Toss is running in — when Toss is the only process
/// attached to it.
///
/// Dropping an image on `toss.exe` starts a console application, so Windows
/// hands it a console window nobody asked for and nobody can read: what that
/// path is for is the viewer. But a console Toss *shares* belongs to
/// whoever else is on it — a shell, a script host, a test harness — and
/// taking that away would be taking the user's terminal. So the condition is
/// stated as a fact about the console, not as a guess about how Toss was
/// started: **one attached process means it is Toss's own console, and this
/// hides only that.**
///
/// What it deliberately does *not* claim is which way of starting Toss
/// produced such a console. That inference is not needed for the decision,
/// and pretending to own a launch detector this does not have would make the
/// contract wider than the API it rests on.
///
/// The subsystem stays CUI either way. `cmd` and PowerShell decide whether
/// to wait for Toss and how to read its exit code from that, and this
/// presentation detail is not worth trading against the exit-code contract
/// §24 builds the tool around — measured, not assumed: a `windows`
/// subsystem build returns from PowerShell in ~12 ms with no
/// `$LASTEXITCODE` at all, where the CUI build waits and reports.
fn hide_owned_console() {
    // SAFETY: both calls take only plain values and this thread's identity;
    // the process list buffer is writable storage this function owns and
    // the returned `HWND` is borrowed for the `ShowWindow` call only, never
    // retained. Two entries are enough to answer the one question asked
    // (`== 1`), and a console that does not exist answers 0 — which is
    // "not ours to hide", the correct direction to fail in.
    unsafe {
        let mut attached = [0_u32; 2];

        if GetConsoleProcessList(&mut attached) != 1 {
            return;
        }

        let console = GetConsoleWindow();
        if console.0.is_null() {
            return;
        }

        // The window goes away with the process when it exits, so hiding it
        // is a presentation change for this run and nothing to restore.
        let _ = ShowWindow(console, SW_HIDE);
    }
}

/// Turn a `windows-rs` failure into something Toss can say (§23).
///
/// The HRESULT never leaves this module: a user reads a sentence, and the
/// platform layer is the only place that knows it came from a platform.
fn failed(step: &str, err: Error) -> TossError {
    TossError::other(format!("image viewer could not {step}: {err}"))
}

#[cfg(test)]
mod tests {
    use super::hide_owned_console;

    #[test]
    fn hiding_a_console_the_test_harness_is_still_using_does_nothing() {
        // The one property this function can be poked at in-process: a test
        // binary runs attached to the same console as the harness that is
        // reading its output, so the count is never 1 here and the console
        // must be left exactly as it was. If the condition ever inverted,
        // this test would take the harness's console away and the run would
        // go quiet rather than merely fail — which is a loud enough signal,
        // and there is no honest way to assert "the window is still visible"
        // from inside the process that would have hidden it.
        hide_owned_console();
    }
}
