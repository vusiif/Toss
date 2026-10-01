//! The Windows implementation of media playback, on Media Foundation.
//!
//! Two halves, for the same reason the image viewer has them: this file owns
//! *which number a failure becomes* (§24), and `backend.rs` owns *how a
//! session is kept alive* (Phase 7's `MEDIA_PLAYBACK.md` §7 — raw MF stays
//! behind the wrapper, and every `unsafe` of the phase lives there).

mod backend;
mod player;

use std::path::Path;

use windows::Win32::Media::MediaFoundation::{
    MF_E_CANNOT_PARSE_BYTESTREAM, MF_E_INVALIDMEDIATYPE, MF_E_NO_MORE_TYPES,
    MF_E_UNSUPPORTED_BYTESTREAM_TYPE,
};
use windows::core::{Error, HRESULT};

use crate::core::error::TossError;

/// Play `path` through Media Foundation, or say which of §24's numbers the
/// attempt earned.
pub fn play(path: &Path) -> Result<(), TossError> {
    backend::play(path)
}

/// A `windows-rs` failure, classified the same way an HRESULT from the event
/// pump is (`from_hresult`) — there is one rule for "MF said no", whether it
/// said it while opening or while playing (§23).
/// The current instance, as an `HINSTANCE` window creation wants.
///
/// Failures name the file being played because that is the operation that
/// was under way — the same wording the viewer uses for the same call.
fn module(path: &Path) -> Result<windows::Win32::Foundation::HINSTANCE, TossError> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    // SAFETY: a null module name asks for the calling process, which always
    // has one.
    let handle =
        unsafe { GetModuleHandleW(None) }.map_err(|err| failed(path, "find itself", err))?;

    Ok(windows::Win32::Foundation::HINSTANCE(handle.0))
}
fn failed(path: &Path, step: &str, err: Error) -> TossError {
    from_hresult(path, step, err.code())
}

/// Which of §24's numbers an MF failure earns.
///
/// The rule follows `decode.rs` in Phase 6 M7, because the situation is the
/// same: dispatch has already decided this file *claims* to be media (§6 —
/// extension or magic), so by the time MF refuses it, the claim and the
/// bytes have disagreed.
///
/// - **"There is no such thing here at all" → 3.** MF found nothing it
///   recognises in the bytes — no bytestream handler, no parsable format.
///   That is §24's unsupported format, and it is also what a file fed to
///   this backend by hand rather than by classification looks like.
/// - **"The file is not there / is shut against us" → 4 / 5.** Availability,
///   not content: a script waiting for a lock to clear has no use for being
///   told the media is broken.
/// - **Everything else → 6.** The extension said media, MF read the bytes
///   and refused them: corrupt or incomplete, which is exactly what
///   `truncated.mp4` is. Any *systemic* failure (MF failed to start, the
///   renderer could not be reached) also lands here rather than as a silent
///   generic 1 — deliberately: the caller handed Toss a file, and the number
///   it gets back has to be about that file being unusable.
fn from_hresult(path: &Path, step: &str, code: HRESULT) -> TossError {
    use windows::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND,
    };

    // A Win32 code travelling in an HRESULT: facility 5, code in the low
    // word — computed rather than imported, as in `decode.rs`.
    const fn from_win32(code: u32) -> i32 {
        (0x8007_0000_u32 | code) as i32
    }

    if code.0 == from_win32(ERROR_ACCESS_DENIED.0) {
        return TossError::PermissionDenied(path.to_path_buf());
    }
    if code.0 == from_win32(ERROR_FILE_NOT_FOUND.0) || code.0 == from_win32(ERROR_PATH_NOT_FOUND.0)
    {
        return TossError::InputNotFound(path.to_path_buf());
    }

    if code == MF_E_UNSUPPORTED_BYTESTREAM_TYPE
        || code == MF_E_CANNOT_PARSE_BYTESTREAM
        || code == MF_E_NO_MORE_TYPES
        || code == MF_E_INVALIDMEDIATYPE
    {
        return TossError::UnsupportedFormat(path.to_path_buf());
    }

    TossError::CorruptInput {
        path: path.to_path_buf(),
        context: format!("{step}: {code:?}"),
    }
}
