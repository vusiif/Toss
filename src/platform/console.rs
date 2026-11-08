//! Reuse or allocate a console for a terminal-driven file viewer.
#[cfg(windows)]
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
#[cfg(not(windows))]
pub fn ensure() {}
