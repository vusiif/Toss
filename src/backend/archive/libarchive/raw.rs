//! Raw libarchive bindings — the narrowest point of the whole subsystem.
//!
//! Everything in this file is a libarchive concept, and §4 requires that
//! none of it leaves `backend/archive/libarchive/`: no `archive*`, no
//! `ARCHIVE_OK`, no format constants may be seen by a handler or by core.
//! The safe wrapper built on top of this (§45 step 4) is where ownership,
//! lifetimes and translation into Toss's types happen.
//!
//! Declarations are hand-written rather than generated. The surface Toss
//! needs is small, and hand-writing it keeps build.rs free of bindgen and
//! of the libclang dependency that comes with it (§15, §32).
//!
//! # Safety contract (§31)
//!
//! Every function here follows libarchive's own rules:
//!
//! * A `*mut Archive` is created by [`archive_read_new`] and must be passed
//!   to [`archive_read_free`] exactly once, on every path including error
//!   paths. Nothing else may free it.
//! * The returned pointer is valid until that `archive_read_free` call and
//!   is not thread-safe: one archive belongs to one thread.
//! * `archive_read_free` is the only cleanup, and it also closes any file
//!   the archive had open.
//! * Strings returned by libarchive are borrowed from the `Archive` and
//!   become dangling as soon as it is freed; they must be copied before
//!   cleanup, never stored.

use std::os::raw::{c_char, c_int, c_void};

/// Opaque libarchive reader. Never constructed in Rust; only ever obtained
/// from [`archive_read_new`] and passed back by pointer.
#[repr(C)]
pub struct Archive {
    _private: [u8; 0],
}

/// Opaque description of one archive member, owned by the `Archive` it was
/// handed out by. Valid only until the next `archive_read_next_header` call
/// or until the archive is freed.
#[repr(C)]
pub struct ArchiveEntry {
    _private: [u8; 0],
}

pub const ARCHIVE_EOF: c_int = 1;
pub const ARCHIVE_OK: c_int = 0;
pub const ARCHIVE_RETRY: c_int = -10;
pub const ARCHIVE_WARN: c_int = -20;
pub const ARCHIVE_FAILED: c_int = -25;
pub const ARCHIVE_FATAL: c_int = -30;

// The `unsafe` marks the declarations as foreign code whose invariants Rust
// cannot check; it does not make any call safe. Every call site still needs
// its own justification (§31).
unsafe extern "C" {
    /// Allocate and initialise a reader. Returns null only if allocation
    /// itself failed; the caller must free whatever non-null value it gets.
    pub fn archive_read_new() -> *mut Archive;

    /// Release an archive and everything it owns, including any file it has
    /// open. Safe to call on an archive that failed part-way through.
    pub fn archive_read_free(archive: *mut Archive) -> c_int;

    /// Ask the reader to recognise every format and filter it knows about.
    /// Without this the reader only handles raw streams.
    pub fn archive_read_support_format_all(archive: *mut Archive) -> c_int;
    pub fn archive_read_support_filter_all(archive: *mut Archive) -> c_int;

    /// Open a file by path. `block_size` of 0 lets libarchive choose.
    ///
    /// `filename` must remain valid for the duration of the call only;
    /// libarchive copies it. It is a C string, so the caller must not pass a
    /// path containing interior NUL bytes.
    pub fn archive_read_open_filename(
        archive: *mut Archive,
        filename: *const c_char,
        block_size: usize,
    ) -> c_int;

    /// Advance to the next member, filling `entry`. Returns [`ARCHIVE_EOF`]
    /// when there is nothing left. `entry` is borrowed from `archive`.
    pub fn archive_read_next_header(archive: *mut Archive, entry: *mut *mut ArchiveEntry) -> c_int;

    /// Read member data into `buffer`, returning the number of bytes read.
    pub fn archive_read_data(archive: *mut Archive, buffer: *mut c_void, length: usize) -> isize;

    /// Description of the most recent error, or null if there was none.
    /// Borrowed from `archive`: copy it before freeing.
    pub fn archive_error_string(archive: *mut Archive) -> *const c_char;

    /// Numeric `errno` for the most recent error, or 0.
    pub fn archive_errno(archive: *mut Archive) -> c_int;

    /// Pathname as recorded inside the archive. Borrowed from `entry`, which
    /// is itself borrowed from `archive`.
    pub fn archive_entry_pathname(entry: *mut ArchiveEntry) -> *const c_char;

    /// Uncompressed size in bytes, or -1 when the archive does not say.
    pub fn archive_entry_size(entry: *mut ArchiveEntry) -> i64;
}
