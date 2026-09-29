//! Owned handle for a libarchive reader.
//!
//! This is the seam §24 asks for: raw bindings below, safe wrapper here,
//! backend above. Nothing outside `backend/archive/libarchive/` can see a
//! `*mut Archive`, because [`Reader`] does not hand one out — it exposes the
//! operations that need it instead (§4, §32).
//!
//! The whole point of owning the handle in a type with a `Drop` impl is that
//! cleanup does not depend on the caller remembering. An error returned from
//! `open`, an early `return`, or a panic unwinding out of a method all release
//! the reader, because releasing is tied to the value leaving scope rather
//! than to a code path (§31).

use std::ptr::NonNull;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::backend::archive::types::{ArchiveError, ArchiveFormat};

use super::raw;

/// Readers currently alive, so tests can assert that both the success and the
/// failure path really release one.
///
/// Present only in test builds: shipping a counter so the tests can count
/// would be the wrong trade for a project that records binary size (§16).
#[cfg(test)]
static LIVE: AtomicUsize = AtomicUsize::new(0);

/// An open-for-reading libarchive archive.
///
/// Never `Copy` and never `Clone`: two owners would mean two calls to
/// `archive_read_free` on one pointer.
pub struct Reader {
    inner: NonNull<raw::Archive>,
}

impl Reader {
    /// Allocate a reader.
    ///
    /// Returns [`ArchiveError::BackendFailure`] rather than panicking if
    /// libarchive cannot allocate, because running out of memory is an
    /// ordinary failure here and not a broken invariant (§23).
    pub fn new() -> Result<Self, ArchiveError> {
        // SAFETY: `archive_read_new` returns null or a reader that only
        // `archive_read_free` may take back. Null is rejected below, so from
        // here on `inner` is never null and every drop frees exactly once.
        let archive = unsafe { raw::archive_read_new() };

        let inner = NonNull::new(archive).ok_or_else(|| {
            ArchiveError::BackendFailure("archive_read_new returned null".to_owned())
        })?;

        #[cfg(test)]
        LIVE.fetch_add(1, Ordering::SeqCst);

        Ok(Self { inner })
    }

    /// Ask the reader to handle one of the formats Toss supports (§18.1).
    ///
    /// [`ArchiveFormat::Unknown`] registers every reader we have, because an
    /// input whose type is not yet known is precisely the case where being
    /// able to recognise any of them is the point.
    pub fn register(&mut self, format: ArchiveFormat) {
        // SAFETY: `self.inner` is a live reader owned by `self`, and these
        // registration calls only mutate its own state.
        unsafe {
            match format {
                ArchiveFormat::Zip => {
                    raw::archive_read_support_format_zip(self.inner.as_ptr());
                }
                ArchiveFormat::SevenZip => {
                    raw::archive_read_support_format_7zip(self.inner.as_ptr());
                }
                ArchiveFormat::Rar => {
                    // RAR4 and RAR5 are separate readers in libarchive, but
                    // one format from Toss's point of view (§6).
                    raw::archive_read_support_format_rar(self.inner.as_ptr());
                    raw::archive_read_support_format_rar5(self.inner.as_ptr());
                }
                ArchiveFormat::Unknown => {
                    raw::archive_read_support_format_zip(self.inner.as_ptr());
                    raw::archive_read_support_format_7zip(self.inner.as_ptr());
                    raw::archive_read_support_format_rar(self.inner.as_ptr());
                    raw::archive_read_support_format_rar5(self.inner.as_ptr());
                }
            }
        }
    }

    /// Open `path` for reading.
    ///
    /// The path is passed through without requiring UTF-8: Windows goes via
    /// the wide entry point and everything else via raw bytes, so a path the
    /// operating system accepted is a path Toss can open (§22).
    pub fn open(&mut self, path: &std::path::Path) -> Result<(), ArchiveError> {
        let code = open_path(self.inner.as_ptr(), path);
        self.check(code, "open")
    }

    /// libarchive's own description of the most recent failure, if any.
    ///
    /// Copied into an owned `String` so it can outlive the reader; libarchive
    /// keeps the text and hands it back dangling the moment the reader is
    /// freed (§31).
    pub fn error_string(&self) -> Option<String> {
        // SAFETY: `self.inner` is live for the duration of this call.
        unsafe {
            let text = raw::archive_error_string(self.inner.as_ptr());
            if text.is_null() {
                return None;
            }
            Some(
                std::ffi::CStr::from_ptr(text)
                    .to_string_lossy()
                    .into_owned(),
            )
        }
    }

    /// Translate a libarchive status code into Toss's error model (§23).
    ///
    /// `ARCHIVE_OK` and `ARCHIVE_EOF` are both successes. Everything negative
    /// becomes an error carrying libarchive's own words, so the caller is
    /// never handed a bare integer to interpret. Deciding which of Toss's
    /// categories a given failure belongs to — unsupported versus corrupt —
    /// is Step 5's job, once probing exists to tell them apart.
    fn check(&self, code: std::os::raw::c_int, what: &str) -> Result<(), ArchiveError> {
        if code >= 0 {
            return Ok(());
        }

        let detail = self.error_string().unwrap_or_default();
        Err(ArchiveError::BackendFailure(if detail.is_empty() {
            format!("{what} failed with status {code}")
        } else {
            format!("{what}: {detail}")
        }))
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        #[cfg(test)]
        LIVE.fetch_sub(1, Ordering::SeqCst);

        // SAFETY: `inner` was created once by `archive_read_new` and is only
        // ever dropped through this type, which is not `Copy` and not `Clone`,
        // so the free runs exactly once whatever path led here.
        unsafe {
            raw::archive_read_free(self.inner.as_ptr());
        }
    }
}

/// Platform-specific open, so neither branch has to convert the path to UTF-8.
///
/// The `cfg` lives here rather than in `core/` or `handlers/` because §12
/// allows platform conditionals in `backend/`.
fn open_path(archive: *mut raw::Archive, path: &std::path::Path) -> std::os::raw::c_int {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.push(0);

        // SAFETY: `archive` is a live reader owned by the caller, and `wide`
        // holds a NUL-terminated buffer that outlives this call.
        unsafe { raw::archive_read_open_filename_w(archive, wide.as_ptr(), 0) }
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::ffi::OsStrExt;

        let mut bytes = path.as_os_str().as_bytes().to_vec();
        // A path reaching here came from the OS or from argv, neither of
        // which can contain an interior NUL; refuse rather than truncate.
        if bytes.contains(&0) {
            return -30; // ARCHIVE_FATAL — checked below, never unwrapped.
        }
        bytes.push(0);

        // SAFETY: `archive` is a live reader owned by the caller, and `bytes`
        // holds a NUL-terminated buffer that outlives this call.
        unsafe { raw::archive_read_open_filename(archive, bytes.as_ptr().cast(), 0) }
    }
}

/// How many readers are alive right now. Test builds only.
#[cfg(test)]
fn live() -> usize {
    LIVE.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};

    use super::{Reader, live};
    use crate::backend::archive::types::ArchiveFormat;

    /// Serialises the tests that read `LIVE`.
    ///
    /// Rust runs tests on threads, and `LIVE` counts readers across the whole
    /// process: two of these overlapping would each see the other's reader and
    /// report a leak that never happened. This passed on Windows and failed on
    /// Linux, which is exactly what a timing-dependent assertion looks like.
    static GUARD: Mutex<()> = Mutex::new(());

    fn exclusive() -> MutexGuard<'static, ()> {
        GUARD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn manifest(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    /// An archive that really opens, and a file that really refuses to.
    const READABLE: &str = "third_party/libarchive/contrib/oss-fuzz/corpus.zip";
    const NOT_AN_ARCHIVE: &str = "Cargo.toml";

    #[test]
    fn a_reader_is_released_after_a_successful_open() {
        let _serial = exclusive();
        assert_eq!(live(), 0, "a previous test leaked a reader");

        {
            let mut reader = Reader::new().expect("libarchive allocates");
            assert_eq!(live(), 1);

            reader.register(ArchiveFormat::Zip);
            reader
                .open(&manifest(READABLE))
                .expect("a real archive opens");
        }

        assert_eq!(live(), 0, "the success path must release the reader");
    }

    #[test]
    fn a_reader_is_released_after_a_failed_open() {
        let _serial = exclusive();
        assert_eq!(live(), 0, "a previous test leaked a reader");

        {
            let mut reader = Reader::new().expect("libarchive allocates");
            assert_eq!(live(), 1);

            reader.register(ArchiveFormat::Zip);
            let err = reader
                .open(&manifest(NOT_AN_ARCHIVE))
                .expect_err("a plain file is not an archive");

            // The refusal keeps libarchive's own words rather than a bare
            // integer, so the message reaches the user intact (§23).
            assert!(
                matches!(&err, crate::backend::archive::ArchiveError::BackendFailure(text)
                    if text.contains("Unrecognized archive format")),
                "expected libarchive's explanation, got {err:?}"
            );
        }

        assert_eq!(live(), 0, "the failure path must release the reader too");
    }

    #[test]
    fn a_reader_refuses_a_path_it_cannot_open_without_panic() {
        let _serial = exclusive();
        let mut reader = Reader::new().expect("libarchive allocates");
        reader.register(ArchiveFormat::Zip);

        let err = reader
            .open(&manifest("no-such-file-for-the-reader"))
            .expect_err("the file does not exist");
        assert!(
            matches!(
                err,
                crate::backend::archive::ArchiveError::BackendFailure(_)
            ),
            "got {err:?}"
        );
    }
}
