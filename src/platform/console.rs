//! Console ownership: reuse or allocate a console for a terminal-driven file
//! viewer, and keep a console Toss alone owns open long enough to read a
//! failure.

/// Reuse or allocate a console for a terminal-driven file viewer.
#[cfg(all(windows, feature = "spreadsheet"))]
pub fn ensure() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleWindow() -> *mut std::ffi::c_void;
        fn GetStdHandle(kind: u32) -> *mut std::ffi::c_void;
        fn GetFileType(handle: *mut std::ffi::c_void) -> u32;
        fn AttachConsole(pid: u32) -> i32;
        fn AllocConsole() -> i32;
    }
    // SAFETY: These kernel32 calls only inspect process-owned standard handles
    // and attach/allocate the calling process's console; no caller pointers used.
    unsafe {
        let output = GetStdHandle(-11i32 as u32);
        // Preserve redirected disk/pipe output; do not create a window for scripts.
        if matches!(GetFileType(output), 1 | 3) {
            return;
        }
        if GetConsoleWindow().is_null() && AttachConsole(u32::MAX) == 0 {
            AllocConsole();
        }
    }
}
#[cfg(all(not(windows), feature = "spreadsheet"))]
pub fn ensure() {}

/// After a failure, wait for Enter if the console window belongs to Toss alone.
///
/// Explorer double-click and drag-and-drop start Toss in a console window of
/// its own, which Windows closes the moment the process exits, taking the
/// `Error:` line with it. Waiting happens only when every condition holds:
///
/// - Toss is the only process attached to the console, so no shell or script
///   is waiting on it (a terminal session always attaches the shell too);
/// - stdin is that console, so a keypress can actually end the wait;
/// - stderr is that console, so the message being kept is on screen.
///
/// Anything else — a terminal, redirected streams, the internal converter
/// worker, a script — returns immediately with output and exit code untouched.
#[cfg(windows)]
pub fn hold_owned_window_on_failure() {
    use std::io::{BufRead, Write};

    type Handle = *mut std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(kind: u32) -> Handle;
        fn GetConsoleMode(handle: Handle, mode: *mut u32) -> i32;
        fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
        fn FlushConsoleInputBuffer(handle: Handle) -> i32;
    }
    const STD_INPUT: u32 = -10i32 as u32;
    const STD_ERROR: u32 = -12i32 as u32;

    // SAFETY: Standard handles are owned by this process; `mode` and `list`
    // are valid local buffers whose lengths are passed exactly.
    let input = unsafe {
        let input = GetStdHandle(STD_INPUT);
        let error = GetStdHandle(STD_ERROR);
        let mut mode = 0u32;
        if GetConsoleMode(input, &mut mode) == 0 || GetConsoleMode(error, &mut mode) == 0 {
            return;
        }
        let mut list = [0u32; 2];
        if GetConsoleProcessList(list.as_mut_ptr(), list.len() as u32) != 1 {
            return;
        }
        input
    };
    // Keys typed while Toss was working must not end the wait unseen.
    // SAFETY: `input` was just confirmed to be a console input handle.
    unsafe {
        FlushConsoleInputBuffer(input);
    }
    eprint!("\nPress Enter to close this window.");
    let _ = std::io::stderr().flush();
    let _ = std::io::stdin().lock().read_line(&mut String::new());
}
#[cfg(not(windows))]
pub fn hold_owned_window_on_failure() {}

#[cfg(test)]
mod tests {
    /// Under the test harness the console, if any, is shared with cargo and
    /// stdin is usually not a console, so this must return without waiting.
    #[test]
    fn does_not_wait_when_console_is_shared_or_absent() {
        super::hold_owned_window_on_failure();
    }
}
