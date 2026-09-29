//! Owned handle for a libarchive writer.
//!
//! The mirror of [`super::reader::Reader`]: same rule, different direction
//! (§24). Nothing outside this module sees a `*mut Archive` or a
//! `*mut ArchiveEntry`, and both are released on every path — including the
//! ones where a header write fails halfway through.
//!
//! The container is chosen here rather than by the caller, because §46 wants
//! creation and extraction under one abstraction; the caller only decides
//! *which file* to produce (§21).

use std::ffi::CStr;
use std::io::Read;
use std::os::raw::{c_long, c_uint};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::time::UNIX_EPOCH;

use super::raw;
use crate::backend::archive::types::ArchiveError;

/// Owned handle for a libarchive writer.
pub struct Writer {
    inner: NonNull<raw::Archive>,
    /// Kept so a failed write can name the destination it could not produce.
    output: PathBuf,
}

impl Writer {
    /// Open `output` for writing a 7z archive.
    pub fn seven_zip(output: PathBuf) -> Result<Self, ArchiveError> {
        // SAFETY: `archive_write_new` returns null or a handle only
        // `archive_write_free` may take back; every early return below frees
        // it before propagating, so a failure never leaks one.
        unsafe {
            let archive = raw::archive_write_new();
            let inner = NonNull::new(archive).ok_or_else(|| {
                ArchiveError::BackendFailure("archive_write_new returned null".to_owned())
            })?;

            let format = raw::archive_write_set_format_7zip(inner.as_ptr());
            if format < 0 {
                let detail = error_string(inner.as_ptr());
                raw::archive_write_free(inner.as_ptr());
                return Err(ArchiveError::BackendFailure(format!(
                    "cannot select the 7z format: {detail}"
                )));
            }

            let opened = open_path(inner.as_ptr(), &output);
            if opened < 0 {
                let detail = error_string(inner.as_ptr());
                raw::archive_write_free(inner.as_ptr());
                return Err(ArchiveError::BackendFailure(format!(
                    "cannot create {}: {detail}",
                    output.display()
                )));
            }

            Ok(Self { inner, output })
        }
    }

    /// Add a directory member. No data follows a directory header.
    pub fn add_directory(
        &self,
        relative: &Path,
        modified: Option<(i64, c_long)>,
    ) -> Result<(), ArchiveError> {
        self.add_member(relative, Some(0), true, modified)
    }

    /// Add a regular file and stream `stream` in, returning bytes written.
    ///
    /// `relative` is the name as it should appear inside the archive — Toss
    /// decides that (§16); the backend only records what it is told.
    /// `modified` is the timestamp of the file on disk, carried by the caller
    /// rather than looked up from `relative`, which is a name that exists only
    /// inside the archive and would resolve to nothing (§30).
    pub fn add_file(
        &self,
        relative: &Path,
        stream: &mut dyn Read,
        size: u64,
        modified: Option<(i64, c_long)>,
    ) -> Result<u64, ArchiveError> {
        self.add_member(relative, Some(size), false, modified)?;

        let mut buffer = vec![0_u8; 64 * 1024];
        let mut written = 0_u64;

        loop {
            let read = stream
                .read(&mut buffer)
                .map_err(|err| ArchiveError::BackendFailure(format!("{relative:?}: {err}")))?;
            if read == 0 {
                break;
            }

            // SAFETY: `self.inner` is a live writer and `buffer` holds
            // `read` valid bytes for the duration of the call.
            let code = unsafe {
                raw::archive_write_data(self.inner.as_ptr(), buffer.as_ptr().cast(), read)
            };
            if code < 0 {
                return Err(self.write_failure(code, "write data"));
            }
            written += code as u64;
        }

        Ok(written)
    }

    /// Flush and close the archive.
    ///
    /// Errors surface here rather than being swallowed by `Drop`: the caller
    /// has to know whether the file on disk is complete.
    pub fn finish(&mut self) -> Result<(), ArchiveError> {
        // SAFETY: `self.inner` is live for the duration of the call.
        let code = unsafe { raw::archive_write_close(self.inner.as_ptr()) };
        if code < 0 {
            return Err(self.write_failure(code, "close"));
        }
        Ok(())
    }

    /// Add everything under `source`, naming members relative to it.
    ///
    /// Returns `(entries written, entries skipped)`.
    ///
    /// Symbolic links and anything that is neither a file nor a directory are
    /// skipped rather than followed: following a link could read outside the
    /// tree the user pointed at, and storing one would produce an archive
    /// that Toss's own extraction refuses to restore (§17). Both sides leave
    /// them out and both report the count, so the omission is visible.
    ///
    /// Children are sorted, so the same directory produces the same archive
    /// byte for byte — otherwise every run would differ by directory order
    /// alone and no diff of two outputs could be trusted.
    pub fn add_tree(&self, source: &Path) -> Result<(u64, u64), ArchiveError> {
        let mut written = 0_u64;
        let mut skipped = 0_u64;
        self.walk(source, source, &mut written, &mut skipped)?;
        Ok((written, skipped))
    }

    fn walk(
        &self,
        root: &Path,
        directory: &Path,
        written: &mut u64,
        skipped: &mut u64,
    ) -> Result<(), ArchiveError> {
        let read = std::fs::read_dir(directory).map_err(|err| io_failure(directory, &err))?;

        let mut children: Vec<_> = read.flatten().collect();
        children.sort_by_key(|entry| entry.file_name());

        for child in children {
            let path = child.path();
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };

            // Not `metadata()`: that follows links, and a link is exactly the
            // case this loop must decide about rather than fall through (§17).
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(err) => return Err(io_failure(&path, &err)),
            };

            if metadata.is_symlink() || !metadata.is_dir() && !metadata.is_file() {
                *skipped += 1;
                continue;
            }

            // Taken from the file on disk here, because `relative` exists only
            // inside the archive: `modified_of(relative)` would have been
            // resolved against the current directory and silently left every
            // timestamp unset.
            let modified = modified_from(&metadata);

            if metadata.is_dir() {
                self.add_directory(relative, modified)?;
                self.walk(root, &path, written, skipped)?;
                *written += 1;
                continue;
            }

            let mut stream = std::fs::File::open(&path).map_err(|err| io_failure(&path, &err))?;
            self.add_file(relative, &mut stream, metadata.len(), modified)?;
            *written += 1;
        }

        Ok(())
    }

    fn add_member(
        &self,
        relative: &Path,
        size: Option<u64>,
        is_dir: bool,
        modified: Option<(i64, c_long)>,
    ) -> Result<(), ArchiveError> {
        let entry = PendingEntry::new()?;
        set_entry_name(&entry, relative)?;
        let filetype = if is_dir { raw::AE_IFDIR } else { raw::AE_IFREG };

        // SAFETY: `self.inner` and `entry.raw` are both live; the writer does
        // not take ownership of the entry, so `PendingEntry` still frees it.
        unsafe {
            raw::archive_entry_set_size(entry.raw, size.unwrap_or(0) as i64);
            raw::archive_entry_set_filetype(entry.raw, filetype as c_uint);
            if let Some((seconds, nanos)) = modified {
                raw::archive_entry_set_mtime(entry.raw, seconds, nanos);
            }

            let code = raw::archive_write_header(self.inner.as_ptr(), entry.raw);
            if code < 0 {
                return Err(self.write_failure(code, "write header"));
            }
        }

        Ok(())
    }

    /// `code` is whatever the failed call returned: `int` for the header and
    /// close family, `la_ssize_t` for the data write (§31).
    fn write_failure(&self, code: impl std::fmt::Display, what: &str) -> ArchiveError {
        // SAFETY: `self.inner` is a live writer; `error_string` copies the
        // text out before anything can be freed (§31).
        let detail = unsafe { error_string(self.inner.as_ptr()) };

        // A refused open and a read-only destination both arrive as a
        // libarchive message; §24 gives permission problems their own exit
        // code because that is the one a user can act on.
        let lower = detail.to_ascii_lowercase();
        if lower.contains("permission denied") || lower.contains("read-only") {
            return ArchiveError::PermissionDenied(self.output.clone());
        }

        ArchiveError::BackendFailure(if detail.is_empty() {
            format!("{what} failed with status {code}")
        } else {
            format!("{what}: {detail}")
        })
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        // SAFETY: `inner` came from `archive_write_new`, this type is neither
        // `Copy` nor `Clone`, so the free runs exactly once. The header says
        // freeing implies a close, so a writer dropped without `finish()` —
        // an error path — still flushes.
        unsafe {
            raw::archive_write_free(self.inner.as_ptr());
        }
    }
}

/// An entry being described, freed whether or not its header was accepted.
///
/// `archive_write_header` does not take ownership, so without this the entry
/// would leak on exactly the path §31 exists to make safe.
struct PendingEntry {
    raw: *mut raw::ArchiveEntry,
}

impl PendingEntry {
    fn new() -> Result<Self, ArchiveError> {
        // SAFETY: `archive_entry_new` returns null or a handle only
        // `archive_entry_free` may take.
        let entry = unsafe { raw::archive_entry_new() };
        let raw = NonNull::new(entry)
            .ok_or_else(|| ArchiveError::BackendFailure("archive_entry_new returned null".into()))?
            .as_ptr();

        Ok(Self { raw })
    }
}

impl Drop for PendingEntry {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `archive_entry_new` and this type owns it
        // exactly once; the writer borrowed it only for `archive_write_header`.
        unsafe {
            raw::archive_entry_free(self.raw);
        }
    }
}

/// Give libarchive the member name, in the form it will not re-interpret on
/// the way into the archive.
///
/// On Unix the bytes pass through untouched: they already *are* the native
/// character set libarchive converts from, so a name the filesystem accepted
/// survives (§22).
///
/// On Windows they must not. The 7z container stores UTF-16, and libarchive
/// reaches it by converting the multibyte pathname **from the current ANSI
/// code page** — which is never UTF-8. Feeding it UTF-8 bytes mangled every
/// non-ASCII name in the archive, which is precisely what §22 exists to
/// prevent; passing wide characters takes that conversion out of the locale's
/// hands. A name holding an interior NUL has no C-string form at all, so it
/// is refused rather than written back truncated (§16, §23).
fn set_entry_name(entry: &PendingEntry, path: &Path) -> Result<(), ArchiveError> {
    #[cfg(not(windows))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;

        let name = CString::new(path.as_os_str().as_bytes())
            .map_err(|err| ArchiveError::BackendFailure(format!("{}: {err}", path.display())))?;

        // SAFETY: `entry` is a live handle only `archive_entry_free` may take,
        // and `name` is NUL-terminated and outlives the call.
        unsafe { raw::archive_entry_set_pathname(entry.raw, name.as_ptr()) };
        Ok(())
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(ArchiveError::BackendFailure(format!(
                "path cannot be represented in an archive: {}",
                path.display()
            )));
        }
        wide.push(0);

        // SAFETY: `entry` is a live handle only `archive_entry_free` may take,
        // and `wide` is NUL-terminated and outlives the call.
        unsafe { raw::archive_entry_copy_pathname_w(entry.raw, wide.as_ptr()) };
        Ok(())
    }
}

fn open_path(archive: *mut raw::Archive, output: &Path) -> std::os::raw::c_int {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        let mut wide: Vec<u16> = output.as_os_str().encode_wide().collect();
        wide.push(0);

        // SAFETY: `archive` is a live writer and `wide` is NUL-terminated and
        // outlives the call.
        unsafe { raw::archive_write_open_filename_w(archive, wide.as_ptr()) }
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::ffi::OsStrExt;

        let mut bytes = output.as_os_str().as_bytes().to_vec();
        if bytes.contains(&0) {
            return -30; // ARCHIVE_FATAL — checked below, never unwrapped.
        }
        bytes.push(0);

        // SAFETY: `archive` is a live writer and `bytes` is NUL-terminated
        // and outlives the call.
        unsafe { raw::archive_write_open_filename(archive, bytes.as_ptr().cast()) }
    }
}

/// When this entry was last modified, as `(seconds, nanos)` since the epoch.
///
/// Absent rather than defaulted to "now": a timestamp Toss does not know is
/// better left unset than invented (§30). A time before the epoch has no
/// `duration_since` answer and is left unset too, rather than wrapping.
fn modified_from(metadata: &std::fs::Metadata) -> Option<(i64, c_long)> {
    let modified = metadata.modified().ok()?;
    let since_epoch = modified.duration_since(UNIX_EPOCH).ok()?;

    Some((
        since_epoch.as_secs() as i64,
        since_epoch.subsec_nanos() as c_long,
    ))
}

/// An IO failure during the walk, phrased as the archive error §23 asks for.
///
/// Permission problems get their own variant because §24 gives them their own
/// exit code, and "you may not read that folder" is the one a user can act on.
fn io_failure(path: &Path, err: &std::io::Error) -> ArchiveError {
    if err.kind() == std::io::ErrorKind::PermissionDenied {
        return ArchiveError::PermissionDenied(path.to_path_buf());
    }

    ArchiveError::BackendFailure(format!("{}: {err}", path.display()))
}

/// Borrow libarchive's own words while the writer is still alive (§31).
///
/// # Safety
///
/// `archive` must be a live, non-freed writer.
unsafe fn error_string(archive: *mut raw::Archive) -> String {
    // SAFETY: the caller guarantees the writer is alive, and this copies the
    // text out before anything is freed.
    unsafe {
        let text = raw::archive_error_string(archive);
        if text.is_null() {
            return String::new();
        }
        CStr::from_ptr(text).to_string_lossy().into_owned()
    }
}
