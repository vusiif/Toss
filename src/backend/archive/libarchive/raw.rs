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

    /// Register exactly the readers v0.1's milestone calls for (§18.1), rather
    /// than `support_format_all`, which would also link every other format
    /// libarchive happens to know and charge its size to Toss (§3, §37).
    pub fn archive_read_support_format_zip(archive: *mut Archive) -> c_int;
    pub fn archive_read_support_format_7zip(archive: *mut Archive) -> c_int;
    /// RAR 4.x.
    pub fn archive_read_support_format_rar(archive: *mut Archive) -> c_int;
    /// RAR 5.x — a separate reader from the above, so both must be named.
    pub fn archive_read_support_format_rar5(archive: *mut Archive) -> c_int;

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

    /// Wide-character form, Windows only.
    ///
    /// A Windows path need not be valid UTF-8 — it is WTF-16 — so widening it
    /// and calling this is the only way to open every path the OS accepts.
    /// Going through `to_str()` instead would reject paths Toss was asked to
    /// handle (§22). `wchar_t` is 16 bits under MSVC, hence `u16`.
    #[cfg(windows)]
    pub fn archive_read_open_filename_w(
        archive: *mut Archive,
        filename: *const u16,
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

    /// Wide-character form of the member name, Windows only. Needed for the
    /// same reason as the wide open: an entry name is not required to be
    /// valid UTF-8 (§22).
    #[cfg(windows)]
    pub fn archive_entry_pathname_w(entry: *mut ArchiveEntry) -> *const u16;

    /// Uncompressed size in bytes, or -1 when the archive does not say.
    pub fn archive_entry_size(entry: *mut ArchiveEntry) -> i64;

    /// Whether `archive_entry_size` carries a real value.
    pub fn archive_entry_size_is_set(entry: *mut ArchiveEntry) -> c_int;

    /// The type bits of the current entry — `AE_IFDIR` and friends.
    ///
    /// The C type is `__LA_MODE_T`, which libarchive defines per platform:
    /// `unsigned short` under MSVC, `mode_t` (`unsigned int` on glibc)
    /// elsewhere. Declaring it the same on both would read whatever happens
    /// to sit in the upper bits of the return register, so the signature is
    /// cfg'd to match what the callee actually returns (§31). macOS uses a
    /// 16-bit `mode_t` and needs a third branch when it becomes a target
    /// (§36); it is not one yet.
    #[cfg(windows)]
    pub fn archive_entry_filetype(entry: *mut ArchiveEntry) -> u16;
    #[cfg(not(windows))]
    pub fn archive_entry_filetype(entry: *mut ArchiveEntry) -> u32;

    /// Format of the entry just returned, as an `ARCHIVE_FORMAT_*` value.
    pub fn archive_format(archive: *mut Archive) -> c_int;

    /// Short lowercase name for the format, e.g. `"zip"`. Borrowed from
    /// `archive`; copy before freeing.
    pub fn archive_format_name(archive: *mut Archive) -> *const c_char;

    /// Number of filters in the current stack — 0 for a bare archive, 1 for
    /// something like `foo.tar.gz`. This is the compression half of §6's
    /// format/compression split.
    pub fn archive_filter_count(archive: *mut Archive) -> c_int;

    /// Name of filter `index`, e.g. `"gzip"`. Borrowed from `archive`.
    pub fn archive_filter_name(archive: *mut Archive, index: c_int) -> *const c_char;

    /// Discard the remainder of the current entry so the next header call
    /// advances. Cheaper than reading data we only intend to count past.
    pub fn archive_read_data_skip(archive: *mut Archive) -> c_int;
}

/// Container formats Toss recognises (§6). Kept here because these are
/// libarchive's numbering, and §4 keeps libarchive concepts inside this
/// directory — the domain's own [`ArchiveFormat`] lives one level up.
pub const ARCHIVE_FORMAT_ZIP: c_int = 0x50000;
pub const ARCHIVE_FORMAT_7ZIP: c_int = 0xE0000;
pub const ARCHIVE_FORMAT_RAR: c_int = 0xD0000;
/// RAR 5.x reports a distinct code from RAR 4.x, even though Toss treats
/// them as one format (§6).
pub const ARCHIVE_FORMAT_RAR_V5: c_int = 0x100000;

/// `AE_IFDIR` as returned by [`archive_entry_filetype`].
#[cfg(windows)]
pub const AE_IFDIR: u16 = 0o040000;
#[cfg(not(windows))]
pub const AE_IFDIR: u32 = 0o040000;
