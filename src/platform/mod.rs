//! Platform-specific code.
//!
//! This is one of the few places `#[cfg(target_os = ...)]` may appear (§12),
//! alongside `backend/` and the bootstrap. If platform conditionals start
//! showing up in `core/`, `handlers/`, `dispatch/` or `detection/`, the
//! abstraction boundary is wrong and needs rethinking rather than more `cfg`.
//!
//! The architectural invariant this module exists to protect (§10): removing
//! the entire Windows backend must never make the platform-independent core
//! impossible to compile. CI checks that by building on Linux as well.
//!
//! Phases 6 through 8 fill this module.

pub mod portable;

#[cfg(target_os = "windows")]
pub mod windows;

/// Let the C runtime read the process environment into its locale.
///
/// libarchive converts every member name through the C locale — that is how a
/// UTF-16LE entry gets built in a 7z, and how a UTF-8 name is read back out of
/// a zip (§22). Rust never calls `setlocale`, and neither does libarchive, so
/// the C runtime is still on `"C"`: ASCII. Under a locale like that, a
/// filename with any byte above 0x7F cannot be converted at all, and packing a
/// directory with a Chinese or accented name in it fails outright.
///
/// Called once from the bootstrap before anything touches an archive. If the
/// environment names a locale this machine does not have, the call returns
/// null and the locale stays ASCII; that surfaces later as libarchive
/// refusing to convert a named file, rather than as a startup failure for a
/// command that may have nothing to do with archives.
pub fn prepare() {
    const LC_ALL: std::os::raw::c_int = 0; // the same value in glibc and the MSVC CRT

    // SAFETY: `c""` is a static NUL-terminated literal, which is exactly what
    // `setlocale` asks for when it should take the setting from the
    // environment. The pointer it returns belongs to the C runtime: it is
    // dropped without being read, stored or freed (§31).
    unsafe {
        setlocale(LC_ALL, c"".as_ptr());
    }
}

unsafe extern "C" {
    /// C runtime locale selection. Declared here rather than in `backend/`
    /// because it is process setup for everything that follows, not a feature
    /// of one backend (§12, §31).
    fn setlocale(
        category: std::os::raw::c_int,
        locale: *const std::os::raw::c_char,
    ) -> *mut std::os::raw::c_char;
}
