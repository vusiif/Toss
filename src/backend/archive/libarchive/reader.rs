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
        // Pinned before the first reader exists, so every later conversion
        // libarchive performs has a UTF-8 target (§22).
        pin_utf8_locale();

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
        if code < 0 {
            return Err(self.open_failure(code));
        }
        Ok(())
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

    /// Identify `path`: which container it is, and what sits on top of it (§41).
    ///
    /// Reads one header, because libarchive settles on a format while bidding
    /// for it and `archive_format` reports only what has been settled. An
    /// empty but valid archive reaches `ARCHIVE_EOF` and is still identified
    /// — no members is not the same as no format.
    pub fn probe(
        path: &std::path::Path,
    ) -> Result<crate::backend::archive::types::ArchiveProbe, ArchiveError> {
        let mut reader = Self::new()?;
        reader.register(ArchiveFormat::Unknown);
        reader.open(path)?;

        let mut entry = std::ptr::null_mut();
        // SAFETY: `reader.inner` is live; `entry` is an out-parameter that
        // libarchive fills with a pointer it owns and that stays valid until
        // the next header call or the reader is freed.
        let code = unsafe { raw::archive_read_next_header(reader.inner.as_ptr(), &mut entry) };
        if code < 0 {
            return Err(reader.read_failure(code, "probe"));
        }

        Ok(reader.describe())
    }

    /// Enumerate the members of `path` without reading their contents (§45).
    ///
    /// Read-only on purpose: listing is the safer surface to discover a
    /// damaged archive on, before anything is written to disk.
    pub fn list(
        path: &std::path::Path,
    ) -> Result<Vec<crate::backend::archive::types::ArchiveEntry>, ArchiveError> {
        let mut reader = Self::new()?;
        reader.register(ArchiveFormat::Unknown);
        reader.open(path)?;

        let mut entries = Vec::new();
        let mut entry = std::ptr::null_mut();

        loop {
            // SAFETY: as in `probe`; `entry` is an out-parameter owned by the
            // reader and only borrowed for the calls that follow.
            let code = unsafe { raw::archive_read_next_header(reader.inner.as_ptr(), &mut entry) };
            if code == raw::ARCHIVE_EOF {
                break;
            }
            if code < 0 {
                return Err(reader.read_failure(code, "list"));
            }

            entries.push(reader.describe_entry(entry));

            // SAFETY: same live reader; skipping only advances its position.
            let skipped = unsafe { raw::archive_read_data_skip(reader.inner.as_ptr()) };
            if skipped < 0 {
                return Err(reader.read_failure(skipped, "list"));
            }
        }

        Ok(entries)
    }

    /// What the reader has settled on so far.
    fn describe(&self) -> crate::backend::archive::types::ArchiveProbe {
        // SAFETY: `self.inner` is live for the duration of these calls.
        let (format, filter) = unsafe {
            (
                raw::archive_format(self.inner.as_ptr()),
                raw::archive_filter_count(self.inner.as_ptr()),
            )
        };

        crate::backend::archive::types::ArchiveProbe {
            format: match format {
                raw::ARCHIVE_FORMAT_ZIP => ArchiveFormat::Zip,
                raw::ARCHIVE_FORMAT_7ZIP => ArchiveFormat::SevenZip,
                // RAR4 and RAR5 carry distinct codes but are one format here
                // (§6), so both land on the same domain value.
                raw::ARCHIVE_FORMAT_RAR | raw::ARCHIVE_FORMAT_RAR_V5 => ArchiveFormat::Rar,
                _ => ArchiveFormat::Unknown,
            },
            compression: compression_of(self.inner.as_ptr(), filter),
        }
    }

    fn describe_entry(
        &self,
        entry: *mut raw::ArchiveEntry,
    ) -> crate::backend::archive::types::ArchiveEntry {
        crate::backend::archive::types::ArchiveEntry {
            path: entry_path(entry),
            // SAFETY: `entry` came from the reader and has not been advanced
            // past, so it is valid for these calls.
            is_dir: unsafe { raw::archive_entry_filetype(entry) & raw::AE_IFDIR } != 0,
            // SAFETY: same borrowed entry; `-1` is libarchive's way of
            // saying "size not recorded", which is why it is checked first.
            size: unsafe {
                if raw::archive_entry_size_is_set(entry) == 0 {
                    None
                } else {
                    let size = raw::archive_entry_size(entry);
                    (size >= 0).then_some(size as u64)
                }
            },
        }
    }

    /// Classify a failure that happened *after* the container was accepted.
    ///
    /// At that point the format has already been recognised, so anything
    /// going wrong is about the contents — which is §24's corrupt-input case
    /// (exit code 6), not "unsupported".
    fn read_failure(&self, code: std::os::raw::c_int, what: &str) -> ArchiveError {
        let detail = self.error_string().unwrap_or_default();
        ArchiveError::CorruptArchive(if detail.is_empty() {
            format!("{what} failed with status {code}")
        } else {
            format!("{what}: {detail}")
        })
    }

    /// Classify a failure from opening, where the two interesting answers —
    /// "I do not know this format" and "something else went wrong" — have to
    /// stay apart (§42).
    fn open_failure(&self, code: std::os::raw::c_int) -> ArchiveError {
        let detail = self.error_string().unwrap_or_default();

        // libarchive reports an input it cannot bid on with this exact
        // sentence. It does not localise its messages, so matching on the
        // English text is stable rather than a guess.
        if detail.contains("Unrecognized archive format") {
            return ArchiveError::UnsupportedFormat;
        }

        ArchiveError::BackendFailure(if detail.is_empty() {
            format!("open failed with status {code}")
        } else {
            detail
        })
    }
}

/// The container-level filter, which is the only compression fact libarchive
/// exposes through its public API (§6).
///
/// Per-entry methods — the Deflate inside a ZIP member, the LZMA inside a 7z
/// member — live in libarchive's private structures and have no accessor, so
/// reporting `Store` here would claim something Toss cannot see. The honest
/// answer for an unfiltered container is `Unknown`, and the honest answer for
/// a filter we have no variant for is `Unknown` too; adding variants is done
/// when a format needs them (§6: do not add every variant before it is needed).
fn compression_of(
    archive: *mut raw::Archive,
    filter_count: std::os::raw::c_int,
) -> crate::backend::archive::types::CompressionMethod {
    use crate::backend::archive::types::CompressionMethod;

    if filter_count <= 0 {
        return CompressionMethod::Unknown;
    }

    // SAFETY: `archive` is live and `filter_count - 1` is in range, since it
    // was read from this same reader a moment ago.
    let name = unsafe { raw::archive_filter_name(archive, filter_count - 1) };
    if name.is_null() {
        return CompressionMethod::Unknown;
    }

    // SAFETY: a string returned by libarchive, valid while the reader lives —
    // and copied out immediately by `to_string_lossy`.
    match unsafe { std::ffi::CStr::from_ptr(name) }
        .to_string_lossy()
        .as_ref()
    {
        "lzma" => CompressionMethod::Lzma,
        "lzma2" => CompressionMethod::Lzma2,
        _ => CompressionMethod::Unknown,
    }
}

/// The member's name, kept as an `OsString` so no encoding decision is made
/// here (§22): Windows reads wide, everything else reads raw bytes.
fn entry_path(entry: *mut raw::ArchiveEntry) -> std::path::PathBuf {
    if entry.is_null() {
        return std::path::PathBuf::new();
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;

        // SAFETY: `entry` is borrowed from a live reader and the returned
        // pointer is valid until the reader advances or is freed.
        let wide = unsafe { raw::archive_entry_pathname_w(entry) };
        if wide.is_null() {
            return std::path::PathBuf::new();
        }

        // SAFETY: `wide` is NUL-terminated by libarchive's contract.
        let length = unsafe {
            let mut length = 0;
            while *wide.add(length) != 0 {
                length += 1;
            }
            length
        };

        // SAFETY: `wide` points at at least `length + 1` readable u16s.
        std::ffi::OsString::from_wide(unsafe { std::slice::from_raw_parts(wide, length) }).into()
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::ffi::OsStringExt;

        // SAFETY: `entry` is borrowed from a live reader; the C string is
        // valid until the reader advances or is freed.
        let bytes = unsafe { raw::archive_entry_pathname(entry) };
        if bytes.is_null() {
            return std::path::PathBuf::new();
        }

        // SAFETY: NUL-terminated by libarchive's contract.
        std::ffi::OsString::from_vec(
            unsafe { std::ffi::CStr::from_ptr(bytes) }
                .to_bytes()
                .to_vec(),
        )
        .into()
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

/// Tell the C library its character set is UTF-8. Linux only.
///
/// libarchive converts archive entry names into the C library's current
/// locale and reports a failure rather than mangling them — so under the `C`
/// locale that every Rust process starts in (Rust never calls `setlocale`),
/// an archive holding a non-ASCII name is unreadable. A RAR4 member with a
/// Unicode name is what surfaced it: `Pathname cannot be converted from
/// UTF-16BE to current locale`, returned as `ARCHIVE_WARN` from the header
/// read and therefore indistinguishable from real damage unless the locale
/// is fixed first.
///
/// Pinning it makes Toss behave the same on a desktop and on a server whose
/// locale was never configured, which matters for a tool that exists to work
/// on machines it did not set up (§2). Toss does not otherwise consult the
/// locale: dates are printed as UTC and numbers are plain, so nothing else
/// changes.
///
/// macOS and the BSDs already default to UTF-8, and Windows goes through the
/// wide entry point in `open_path` and never converts (§22), so neither needs
/// a branch here — least of all one nobody can test.
#[cfg(target_os = "linux")]
fn pin_utf8_locale() {
    use std::os::raw::{c_char, c_int};
    use std::sync::Once;

    // `LC_ALL` as defined in `<locale.h>`: both glibc and musl use 6.
    const LC_ALL: c_int = 6;

    static ONCE: Once = Once::new();

    unsafe extern "C" {
        fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
    }

    ONCE.call_once(|| {
        // SAFETY: `c"C.UTF-8"` is a NUL-terminated literal that outlives the
        // call, and the pointer it returns belongs to the C library, which we
        // deliberately ignore. `setlocale` is not thread-safe by contract,
        // and `Once` guarantees this body runs exactly once.
        unsafe {
            setlocale(LC_ALL, c"C.UTF-8".as_ptr());
        }
    });
}

#[cfg(not(target_os = "linux"))]
fn pin_utf8_locale() {}

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

    /// Serialises the tests that construct a `Reader`.
    ///
    /// **Every test in this module must call [`exclusive`] first**, including
    /// those that never look at [`live`]. The counter is process-wide and
    /// `Reader::new` bumps it, so a `probe` test running alongside a cleanup
    /// test is enough for the latter to see a reader it did not create.
    ///
    /// Rust runs tests on threads, which is why this failed on Linux and
    /// passed on Windows: the overlap is a matter of timing, and a
    /// timing-dependent assertion is no assertion at all.
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

            // The container was never recognised, so this is "unsupported",
            // not "corrupt" — the two must stay apart for routing (§42).
            assert!(
                matches!(
                    err,
                    crate::backend::archive::ArchiveError::UnsupportedFormat
                ),
                "expected a refusal to recognise the format, got {err:?}"
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

    fn corpus(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("corpus")
            .join("archive")
            .join(relative)
    }

    // ---- §45 step 5: probe and list ----

    #[test]
    fn probe_identifies_every_container_v01_claims_to_read() {
        let _serial = exclusive();
        // §18.1 names ZIP, 7z and the RAR family; RAR4 and RAR5 are one
        // format here even though libarchive reports two codes (§6).
        let cases = [
            ("valid/simple.zip", ArchiveFormat::Zip),
            ("valid/simple.7z", ArchiveFormat::SevenZip),
            ("valid/simple-rar4.rar", ArchiveFormat::Rar),
            ("valid/simple-rar5.rar", ArchiveFormat::Rar),
        ];

        for (name, expected) in cases {
            let probe = Reader::probe(&corpus(name)).unwrap_or_else(|err| panic!("{name}: {err}"));
            assert_eq!(
                probe.format, expected,
                "{name} was read as the wrong format"
            );
        }
    }

    #[test]
    fn list_reports_every_member_with_a_usable_name() {
        let _serial = exclusive();
        for name in [
            "valid/simple.zip",
            "valid/simple.7z",
            "valid/simple-rar4.rar",
            "valid/simple-rar5.rar",
        ] {
            let entries = Reader::list(&corpus(name)).unwrap_or_else(|err| panic!("{name}: {err}"));
            assert!(!entries.is_empty(), "{name} listed nothing");

            for entry in &entries {
                assert!(
                    !entry.path.as_os_str().is_empty(),
                    "{name} produced a member with no name"
                );
            }
        }
    }

    #[test]
    fn list_keeps_non_ascii_entry_names_intact() {
        let _serial = exclusive();
        // The names are whatever the archive recorded; converting them
        // through `str` on the way out is exactly what must not happen (§22).
        let entries = Reader::list(&corpus("unicode/utf8-paths.zip")).expect("the archive lists");

        assert!(!entries.is_empty());
        let any_non_ascii = entries
            .iter()
            .any(|entry| !entry.path.to_string_lossy().is_ascii());
        assert!(
            any_non_ascii,
            "expected at least one non-ASCII member, got: {:?}",
            entries.iter().map(|e| &e.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_malformed_archive_is_reported_as_corrupt_rather_than_as_a_refusal() {
        let _serial = exclusive();
        // §42: "recognised but invalid" is a different statement from
        // "unsupported", and routing must be able to tell them apart.
        for name in ["corrupt/malformed.zip", "corrupt/malformed.7z"] {
            let err = Reader::list(&corpus(name)).expect_err("the sample is damaged");

            assert!(
                matches!(
                    err,
                    crate::backend::archive::ArchiveError::CorruptArchive(_)
                ),
                "{name} should read as corrupt, got {err:?}"
            );
        }
    }

    #[test]
    fn a_file_that_is_not_an_archive_at_all_is_unsupported_not_corrupt() {
        let _serial = exclusive();
        let err =
            Reader::probe(&manifest(NOT_AN_ARCHIVE)).expect_err("Cargo.toml is not an archive");

        assert!(
            matches!(
                err,
                crate::backend::archive::ArchiveError::UnsupportedFormat
            ),
            "got {err:?}"
        );
    }
}
