//! libarchive binding.
//!
//! Layering, bottom to top (§24, §32): [`raw`] holds the declarations,
//! [`reader`] and [`writer`] own their handles and lifetimes, and
//! [`LibarchiveBackend`] turns those into the operations
//! [`super::ArchiveBackend`] promises. Nothing above this module sees a
//! `*mut Archive` or an `ARCHIVE_OK` (§4).

pub mod raw;
pub mod reader;
pub mod writer;

use super::ArchiveBackend;
use super::types::{
    ArchiveCapabilities, ArchiveEntry, ArchiveError, ArchiveFormat, ArchiveInput, ArchiveProbe,
    CreateRequest, CreateResult, ExtractRequest, ExtractResult,
};
use reader::Reader;
use writer::Writer;

/// The libarchive implementation of [`ArchiveBackend`].
///
/// A unit struct on purpose: it holds no state, because every operation owns
/// its own reader or writer from creation to release. Registering it is the
/// single `cfg(feature = "archive")` in the archive domain (§12).
#[derive(Debug, Default, Clone, Copy)]
pub struct LibarchiveBackend;

impl ArchiveBackend for LibarchiveBackend {
    fn capabilities(&self) -> ArchiveCapabilities {
        ArchiveCapabilities {
            // Exactly what §18.1's milestone asks for. Not `support_format_all`:
            // naming a format here means Toss has admitted it (§3, §37).
            read: &[
                ArchiveFormat::Zip,
                ArchiveFormat::SevenZip,
                ArchiveFormat::Rar,
            ],
            // §18.2's default is 7z. Creation and extraction share this trait
            // by design (§46), so the write capability lives here too.
            write: &[ArchiveFormat::SevenZip],
        }
    }

    fn probe(&self, input: &ArchiveInput) -> Result<ArchiveProbe, ArchiveError> {
        Reader::probe(input.primary())
    }

    fn list(&self, input: &ArchiveInput) -> Result<Vec<ArchiveEntry>, ArchiveError> {
        Reader::list(input.primary())
    }

    fn extract(&self, request: &ExtractRequest) -> Result<ExtractResult, ArchiveError> {
        Reader::extract(request)
    }

    fn create(&self, request: &CreateRequest) -> Result<CreateResult, ArchiveError> {
        let mut writer = Writer::seven_zip(request.output.clone())?;

        // The walk happens here rather than in the handler: the members have
        // to be streamed into this writer as they are discovered, and splitting
        // discovery from writing would mean passing an open file list through
        // the domain types for no gain while there is one backend (§46, §42).
        let (entries, skipped) = writer.add_tree(&request.source)?;

        writer.finish()?;

        let bytes_written = std::fs::metadata(&request.output)
            .map(|meta| meta.len())
            .unwrap_or(0);

        Ok(CreateResult {
            entries,
            bytes_written,
            skipped,
        })
    }
}

/// Tell the C library its character set is UTF-8. Linux only.
///
/// libarchive converts archive entry names into the C library's current
/// locale and reports a failure rather than mangling them — so under the `C`
/// locale that every Rust process starts in (Rust never calls `setlocale`),
/// a non-ASCII name is unusable. A RAR4 member with a Unicode name is what
/// surfaced it on the read side: `Pathname cannot be converted from UTF-16BE
/// to current locale`, returned as `ARCHIVE_WARN` from the header read and
/// therefore indistinguishable from real damage unless the locale is fixed
/// first. Writing met the same wall from the other direction — a 7z refuses
/// `A filename cannot be converted to UTF-16LE` — and a directory to compress
/// may never construct a reader at all, so both entry points call this rather
/// than one of them depending on the other having run (§22).
///
/// Pinning it makes Toss behave the same on a desktop and on a server whose
/// locale was never configured, which matters for a tool that exists to work
/// on machines it did not set up (§2). Toss does not otherwise consult the
/// locale: dates are printed as UTC and numbers are plain, so nothing else
/// changes.
///
/// macOS and the BSDs already default to UTF-8, and Windows reaches both the
/// file and the member name through wide entry points that never convert
/// (§22), so neither needs a branch here — least of all one nobody can test.
#[cfg(target_os = "linux")]
pub(super) fn pin_utf8_locale() {
    use std::os::raw::{c_char, c_int};
    use std::sync::Once;

    // `LC_ALL` as defined in `<locale.h>`: both glibc and musl use 6. It is
    // not 0 here — that is `LC_CTYPE` on glibc and only `LC_ALL` under MSVC.
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
pub(super) fn pin_utf8_locale() {}

#[cfg(test)]
mod tests {
    use super::raw;
    use std::ffi::CString;
    use std::ptr;

    /// A real archive that ships with the vendored tree, so the test needs no
    /// fixture of its own and the bytes are upstream's (§29).
    const ARCHIVE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/third_party/libarchive/contrib/oss-fuzz/corpus.zip"
    );

    /// Something that is definitely not an archive.
    const NOT_AN_ARCHIVE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");

    /// Copy libarchive's own words out while the reader is still alive.
    ///
    /// # Safety
    ///
    /// `archive` must be a non-null reader that has not been freed yet. The
    /// returned string is owned by that reader, so it is copied here rather
    /// than handed out — storing it would dangle the moment the archive is
    /// released (§31).
    unsafe fn error_string(archive: *mut raw::Archive) -> String {
        // SAFETY: the caller upholds the liveness requirement above, and
        // `CStr::from_ptr` only reads memory libarchive owns for as long as
        // this copy takes.
        unsafe {
            let text = raw::archive_error_string(archive);
            if text.is_null() {
                return String::new();
            }
            std::ffi::CStr::from_ptr(text)
                .to_string_lossy()
                .into_owned()
        }
    }

    fn open(archive: *mut raw::Archive, path: &str) -> std::os::raw::c_int {
        let path = CString::new(path).expect("fixture path contains no interior NUL");
        // SAFETY: `archive` is a live reader owned by the caller, and the
        // CString outlives this call, which is the only thing that reads it.
        unsafe { raw::archive_read_open_filename(archive, path.as_ptr(), 0) }
    }

    fn new_reader() -> *mut raw::Archive {
        // SAFETY: a fresh reader is returned to the caller, who is
        // responsible for freeing it exactly once.
        unsafe {
            let archive = raw::archive_read_new();
            assert!(!archive.is_null(), "archive_read_new returned null");
            raw::archive_read_support_format_all(archive);
            raw::archive_read_support_filter_all(archive);
            archive
        }
    }

    /// §45 step 3: build, link, open a real archive, read a header, clean up.
    #[test]
    fn a_real_archive_opens_reads_a_header_and_frees() {
        let archive = new_reader();

        // SAFETY: freed exactly once at the end, after everything borrowed
        // from it has been copied out.
        unsafe {
            let opened = open(archive, ARCHIVE);
            assert_eq!(
                opened,
                raw::ARCHIVE_OK,
                "a real archive must open: {}",
                error_string(archive)
            );

            let mut entry: *mut raw::ArchiveEntry = ptr::null_mut();
            let header = raw::archive_read_next_header(archive, &mut entry);
            assert!(
                header == raw::ARCHIVE_OK || header == raw::ARCHIVE_EOF,
                "reading a header must succeed, got {header}: {}",
                error_string(archive)
            );

            if header == raw::ARCHIVE_OK {
                assert!(!entry.is_null(), "a readable header hands back an entry");
                let name = raw::archive_entry_pathname(entry);
                assert!(!name.is_null(), "an entry must have a name");
                let name = std::ffi::CStr::from_ptr(name).to_string_lossy();
                assert!(!name.is_empty(), "an entry name must not be blank");
            }

            assert_eq!(
                raw::archive_read_free(archive),
                raw::ARCHIVE_OK,
                "cleanup must succeed on the happy path"
            );
        }
    }

    /// libarchive probes the format as it opens, so a plain file is refused
    /// there rather than at the first header — and it says why (§7: an
    /// unrecognised input must produce an intelligible message, not a crash).
    #[test]
    fn a_non_archive_is_refused_with_a_readable_reason() {
        let archive = new_reader();

        // SAFETY: freed exactly once below, after the borrowed error text
        // has been copied into an owned String.
        unsafe {
            let opened = open(archive, NOT_AN_ARCHIVE);
            assert_eq!(
                opened,
                raw::ARCHIVE_FATAL,
                "a plain file must not be accepted as an archive"
            );

            let reason = error_string(archive);
            assert!(
                reason.contains("Unrecognized archive format"),
                "the refusal must name the problem, got: {reason}"
            );

            assert_eq!(
                raw::archive_read_free(archive),
                raw::ARCHIVE_OK,
                "cleanup must succeed after a rejected open"
            );
        }
    }

    /// Cleanup must not depend on the archive having been used.
    #[test]
    fn a_reader_can_be_freed_without_ever_being_opened() {
        let archive = new_reader();

        // SAFETY: the only free is `archive_read_free`, called once, on a
        // pointer `archive_read_new` handed over.
        unsafe {
            assert_eq!(
                raw::archive_read_free(archive),
                raw::ARCHIVE_OK,
                "an untouched reader must still release cleanly"
            );
        }
    }
}
