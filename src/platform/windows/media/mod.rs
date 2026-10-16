//! The Windows implementation of media playback, on Media Foundation.
//!
//! Two halves, for the same reason the image viewer has them: this file owns
//! *which number a failure becomes* (§24), and `backend.rs` owns *how a
//! session is kept alive* (Phase 7's `MEDIA_PLAYBACK.md` §7 — raw MF stays
//! behind the wrapper, and every `unsafe` of the phase lives there).

mod backend;
mod dashboard;
mod player;

use std::path::Path;

use windows::Win32::Media::MediaFoundation::{
    MF_E_CANNOT_PARSE_BYTESTREAM, MF_E_INVALIDMEDIATYPE, MF_E_NO_MORE_TYPES,
    MF_E_TOPO_CODEC_NOT_FOUND, MF_E_TOPO_UNSUPPORTED, MF_E_UNSUPPORTED_BYTESTREAM_TYPE,
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

    // Three kinds of "no": the bytes are not a format we know, the
    // stream cannot be parsed — and, added after a real 10.8 GB movie was
    // refused: **this machine has no decoder for that stream**. The last
    // one is the machine talking ("your Windows lacks DTS"), not the file
    // being damaged, so it belongs to 3 with the others: §24's unsupported
    // format, never 6.
    if code == MF_E_UNSUPPORTED_BYTESTREAM_TYPE
        || code == MF_E_CANNOT_PARSE_BYTESTREAM
        || code == MF_E_NO_MORE_TYPES
        || code == MF_E_INVALIDMEDIATYPE
        || code == MF_E_TOPO_CODEC_NOT_FOUND
        || code == MF_E_TOPO_UNSUPPORTED
    {
        return TossError::UnsupportedFormat(path.to_path_buf());
    }

    TossError::CorruptInput {
        path: path.to_path_buf(),
        context: format!("{step}: {code:?}"),
    }
}

/// The console companion for media playback (§4 of `MEDIA_PLAYBACK.md`).
///
/// Follows the same format as the image viewer's companion (§12 of
/// `IMAGE_VIEWER.md`): reports file, format, duration, and the implemented
/// controls for video.
pub(super) fn summary(path: &Path, duration_100ns: u64, has_video: bool) -> String {
    let mut text = String::new();
    text.push_str("Toss — Media\n\n");

    let mut field = |label: &str, value: String| {
        text.push_str(&format!("{label:<12}{value}\n"));
    };

    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());

    field("File", file_name);
    field("Format", format_name(path).to_owned());
    field("Duration", format_duration(duration_100ns));

    text.push_str("\nPlaying\n");

    if has_video {
        text.push_str("\nControls\n");
        for (key, action) in [
            ("Space", "Play / pause"),
            ("Left/Right", "Seek"),
            ("Up/Down", "Volume"),
            ("F", "Fullscreen"),
            ("Esc", "Stop and close"),
        ] {
            text.push_str(&format!("  {key:<12}{action}\n"));
        }
    }

    text
}

/// Format duration given in 100-nanosecond units (`MF_PD_DURATION`) as MM:SS
/// or HH:MM:SS.
pub(super) fn format_duration(duration_100ns: u64) -> String {
    if duration_100ns == u64::MAX {
        return "unknown".to_owned();
    }
    let total_seconds = duration_100ns / 10_000_000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// The display name for the extension that identified this file as media.
pub(super) fn format_name(path: &Path) -> &'static str {
    let Some(extension) = path.extension() else {
        return "(none)";
    };

    match extension.to_string_lossy().to_ascii_lowercase().as_str() {
        "mp4" => "MP4",
        "mkv" => "MKV",
        "webm" => "WebM",
        "avi" => "AVI",
        "mov" => "MOV",
        "mp3" => "MP3",
        "flac" => "FLAC",
        "wav" => "WAV",
        _ => "(none)",
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{format_duration, format_name, summary};

    #[test]
    fn format_duration_converts_100ns_units() {
        assert_eq!(format_duration(u64::MAX), "unknown");
        assert_eq!(format_duration(0), "00:00");
        assert_eq!(format_duration(10_000_000), "00:01");
        // 222 seconds = 3 min 42 sec (from MEDIA_PLAYBACK.md §4 sample)
        assert_eq!(format_duration(222 * 10_000_000), "03:42");
        // 3665 seconds = 1 hour 1 min 5 sec
        assert_eq!(format_duration(3665 * 10_000_000), "01:01:05");
    }

    #[test]
    fn format_name_recognises_media_extensions() {
        assert_eq!(format_name(Path::new("song.mp3")), "MP3");
        assert_eq!(format_name(Path::new("clip.MP4")), "MP4");
        assert_eq!(format_name(Path::new("track.flac")), "FLAC");
        assert_eq!(format_name(Path::new("movie.mkv")), "MKV");
        assert_eq!(format_name(Path::new("video.webm")), "WebM");
        assert_eq!(format_name(Path::new("retro.avi")), "AVI");
        assert_eq!(format_name(Path::new("film.mov")), "MOV");
        assert_eq!(format_name(Path::new("sound.wav")), "WAV");
        assert_eq!(format_name(Path::new("unknown")), "(none)");
        assert_eq!(format_name(Path::new("unknown.foo")), "(none)");
    }

    #[test]
    fn media_summary_reports_expected_fields() {
        let text = summary(Path::new("song.mp3"), 222 * 10_000_000, false);
        assert!(text.contains("Toss — Media"), "{text}");
        assert!(text.contains("song.mp3"), "{text}");
        assert!(text.contains("MP3"), "{text}");
        assert!(text.contains("03:42"), "{text}");
        assert!(text.contains("Playing"), "{text}");
        // Audio-only does not list window controls
        assert!(!text.contains("Controls"), "{text}");

        let video_text = summary(Path::new("movie.mp4"), 120 * 10_000_000, true);
        assert!(video_text.contains("Toss — Media"), "{video_text}");
        assert!(video_text.contains("movie.mp4"), "{video_text}");
        assert!(video_text.contains("Controls"), "{video_text}");
        for key in ["Space", "Left/Right", "Up/Down", "F", "Esc"] {
            assert!(
                video_text.contains(key),
                "missing key {key:?} in video summary"
            );
        }
    }
}
