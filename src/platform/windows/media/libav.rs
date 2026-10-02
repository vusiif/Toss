//! The software HEVC decode boundary (Slice 1).
//!
//! One rule makes this module what the design called for (§5): **FFmpeg
//! native types stop here.** `AVFrame`, `AVFormatContext`, `AVCodecContext`
//! and friends exist only inside `hevc_shim.c`; what crosses out is plain
//! C data — a session handle and a `BgraFrame` of bytes Toss can point at.
//! No `av*` function is called from anywhere else in Toss (§32's narrow
//! FFI, one file deep).
//!
//! The typed-error mapping is the fallback consumer's contract (design §9):
//!
//! ```text
//! shim -1 / -2  (nothing recognised / no decodable stream)
//!     → TossError::UnsupportedFormat        → exit 3 at the §24 boundary
//! shim -3       (recognised, bytes damaged)
//!     → TossError::CorruptInput             → exit 6 at the §24 boundary
//! shim -4 / unknown
//!     → TossError::other                    → exit 1
//! ```
//!
//! **Exit numbers are never written here** — only the typed variants; the
//! boundary projection happens in `TossError::exit_code`, so the taxonomy
//! and the exit contract cannot drift apart (7.6-A revision).
//!
//! Slice 1 scope: decode capability + its tests. No window, no timing, no
//! seek, no MF fallback wiring — those are Slices 2 and 3.

use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};

use crate::core::error::TossError;

/// Error codes `hevc_shim.c` returns; the two files are one contract.
mod shim_code {
    pub(super) const RECOGNIZE: i32 = -1;
    pub(super) const NO_VIDEO: i32 = -2;
    pub(super) const DATA: i32 = -3;
    // INTERNAL is the shim's -4: it falls through to `other` below.
}

/// One frame as the shim hands it over — mirrors `TossHevcFrame`
/// field-for-field. `long` is 4 bytes under MSVC's LLP64, which is the
/// only ABI this module exists on (the gates that include this file).
#[repr(C)]
#[derive(Default)]
struct ShimFrame {
    width: i32,
    height: i32,
    stride: i32,
    bytes: i32,
    src_pix_fmt: i32,
    index: i32,
}

unsafe extern "C" {
    fn toss_hevc_open(path: *const c_char, err_out: *mut i32) -> *mut c_void;
    fn toss_hevc_next(session: *mut c_void, out: *mut ShimFrame, bgra: *mut *const u8) -> i32;
    fn toss_hevc_close(session: *mut c_void);
}

/// One decoded picture, BGRA8, borrowed from the reader that produced it.
///
/// The pixels live in the reader's own reusable buffer (design §6): the
/// borrow ends at the next `next_frame` or when the reader drops, and the
/// slice's length is checked against the stride the shim reported rather
/// than trusted.
pub(super) struct BgraFrame<'a> {
    pub(super) width: u32,
    pub(super) height: u32,
    /// Bytes per row: `width * 4` (BGRA8).
    pub(super) stride: u32,
    pub(super) data: &'a [u8],
    /// 0-based position of this frame in the presentation.
    pub(super) index: u64,
    /// The pixel format the *decoder* produced, as FFmpeg's numeric id
    /// (`62` = `yuv420p10le`, the 10-bit Main10 format that is 365/366 of
    /// the corpus). Diagnostic evidence that swscale really converted a
    /// 10-bit source — no FFmpeg type crosses the boundary, only this id.
    pub(super) src_pix_fmt: i32,
}

/// A local HEVC file opened for software decoding.
///
/// RAII, one level: `open` owns the shim session, `Drop` closes it, and
/// every path out of this module releases exactly once — the same shape
/// `Session`/`Window` use on the MF side.
#[derive(Debug)]
pub(super) struct HevcReader {
    session: *mut c_void,
    path: PathBuf,
}

impl HevcReader {
    /// Open `path` and prepare its video stream for decoding.
    ///
    /// The path crosses as UTF-8 (§22): a Windows path's bytes are already
    /// what FFmpeg wants, and an interior NUL is refused rather than
    /// truncated — the same rule `backend::wide_path` applies to MF.
    pub(super) fn open(path: &Path) -> Result<Self, TossError> {
        let mut bytes = path.as_os_str().as_encoded_bytes().to_vec();
        if bytes.contains(&0) {
            return Err(TossError::invalid_arguments(format!(
                "path cannot be opened: {}",
                path.display()
            )));
        }
        bytes.push(0);

        let mut code: i32 = 0;
        // SAFETY: `bytes` is a NUL-terminated UTF-8 buffer owned by this
        // call and lives until the shim's open returns; `code` is local
        // out-storage. The returned handle, if non-null, is owned by the
        // reader and released by `Drop` → `toss_hevc_close`.
        let session = unsafe { toss_hevc_open(bytes.as_ptr() as *const c_char, &mut code) };

        if session.is_null() {
            return Err(from_shim(code, path));
        }

        Ok(Self {
            session,
            path: path.to_path_buf(),
        })
    }

    /// Decode the next picture into BGRA, or `Ok(None)` at the end.
    ///
    /// Errors arrive classified by the shim and are re-typed here (§9's
    /// mapping, at the top of this file); the returned frame borrows this
    /// reader, so `&mut self` is the whole concurrency story in Slice 1 —
    /// no locks, no workers.
    pub(super) fn next_frame(&mut self) -> Result<Option<BgraFrame<'_>>, TossError> {
        let mut raw = ShimFrame::default();
        let mut bgra: *const u8 = std::ptr::null();

        // SAFETY: `self.session` is alive until this reader drops (the
        // guarantee `Drop` states below); both out-pointers are locals
        // the shim fills only on success.
        let code = unsafe { toss_hevc_next(self.session, &mut raw, &mut bgra) };

        match code {
            0 => Ok(None),
            1 => {
                let len = usize::try_from(raw.bytes).map_err(|_| {
                    TossError::other("software decode reported a negative frame size")
                })?;
                if bgra.is_null() || raw.stride < raw.width.saturating_mul(4) {
                    return Err(TossError::other(
                        "software decode handed back an inconsistent frame",
                    ));
                }
                // SAFETY: on code 1 the shim promises `bgra` points at
                // `bytes` readable pixels in its own buffer, valid until
                // the next call on this session — which cannot happen
                // while `&mut self` is held.
                let data = unsafe { std::slice::from_raw_parts(bgra, len) };
                Ok(Some(BgraFrame {
                    width: raw.width.max(0) as u32,
                    height: raw.height.max(0) as u32,
                    stride: raw.stride.max(0) as u32,
                    data,
                    index: raw.index.max(0) as u64,
                    src_pix_fmt: raw.src_pix_fmt,
                }))
            }
            other => Err(from_shim(other, &self.path)),
        }
    }
}

impl Drop for HevcReader {
    fn drop(&mut self) {
        // SAFETY: the session came from `toss_hevc_open` and this runs
        // exactly once; `close` is the only way the handle goes away.
        unsafe { toss_hevc_close(self.session) };
    }
}

/// The shim's codes → Toss's typed errors (design §9). The exit number
/// each variant becomes is `exit_code`'s business, not this file's.
fn from_shim(code: i32, path: &Path) -> TossError {
    match code {
        shim_code::RECOGNIZE | shim_code::NO_VIDEO => {
            TossError::UnsupportedFormat(path.to_path_buf())
        }
        shim_code::DATA => TossError::CorruptInput {
            path: path.to_path_buf(),
            context: "software decode: the file's bytes are damaged".to_owned(),
        },
        _ => TossError::other(format!("software decode failed for {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // Generated by tests/corpus/media/generate.sh (libx265, `-pix_fmt
    // yuv420p10le` → Main10): the committed bytes are the fixture, and
    // running ffmpeg inside a test is exactly what generate.sh forbids.
    const MAIN10_MKV: &str = "tests/corpus/media/hevc.mkv";
    const MAIN10_MP4: &str = "tests/corpus/media/hevc.mp4";
    const MAIN10_CHINESE: &str = "tests/corpus/media/hevc 中文.mkv";

    /// `yuv420p10le`'s id in the vendored FFmpeg — pinned evidence that a
    /// 10-bit Main10 source reached swscale (design §8.1). Re-measured,
    /// never guessed, if a FFmpeg bump moves the enum.
    const PIX_FMT_YUV420P10LE: i32 = 62;

    /// Frames the one-second, 10 fps fixture contains (generate.sh's
    /// `rate=10:duration=1`); measured from the committed bytes with
    /// ffprobe before this constant existed.
    const FIXTURE_FRAMES: u64 = 10;

    /// A scratch file under the system temp directory, keyed by process id
    /// so one run's parallel tests never collide on a name.
    fn scratch(name: &str) -> PathBuf {
        let dir = scratch_dir();
        fs::create_dir_all(&dir).expect("temp dir for hevc tests");
        dir.join(name)
    }

    fn scratch_dir() -> PathBuf {
        std::env::temp_dir().join(format!("toss-hevc-{}", std::process::id()))
    }

    /// Decode every frame of `path`, checking the per-frame invariants the
    /// presentation path will rely on (design §8): BGRA layout, buffer
    /// length, gapless indices, and a first picture that is not empty.
    /// Returns `(frames, width, height, first src_pix_fmt)`.
    fn drain(path: &Path) -> (u64, u32, u32, i32) {
        let mut reader = HevcReader::open(path).expect("the fixture must open");
        let mut frames: u64 = 0;
        let (mut width, mut height, mut fmt) = (0, 0, -1);

        while let Some(frame) = reader.next_frame().expect("the fixture must decode") {
            assert_eq!(
                frame.stride,
                frame.width * 4,
                "every row is BGRA8: stride == width * 4"
            );
            assert_eq!(
                frame.data.len(),
                (frame.stride * frame.height) as usize,
                "the buffer must cover exactly the picture"
            );
            assert_eq!(frame.index, frames, "indices count from zero, without gaps");
            if frames == 0 {
                assert!(
                    frame.data.iter().any(|&byte| byte != 0),
                    "the first picture must not be empty"
                );
                (width, height, fmt) = (frame.width, frame.height, frame.src_pix_fmt);
            }
            frames += 1;
        }

        (frames, width, height, fmt)
    }

    #[test]
    fn main10_mkv_decodes_to_toss_bgra() {
        let (frames, width, height, fmt) = drain(Path::new(MAIN10_MKV));

        assert_eq!((width, height), (160, 120), "generate.sh's test picture");
        assert_eq!(
            fmt, PIX_FMT_YUV420P10LE,
            "the source really is 10-bit Main10"
        );
        assert_eq!(frames, FIXTURE_FRAMES);
    }

    #[test]
    fn main10_mp4_decodes_to_toss_bgra() {
        // The other demuxer the build keeps (mov): same picture, same
        // expectations — a container difference must not change the frame.
        let (frames, width, height, fmt) = drain(Path::new(MAIN10_MP4));

        assert_eq!((width, height), (160, 120));
        assert_eq!(fmt, PIX_FMT_YUV420P10LE);
        assert_eq!(frames, FIXTURE_FRAMES);
    }

    #[test]
    fn chinese_path_decodes_like_any_other() {
        // §22 + the spike's standing lesson: the path crosses as UTF-8 and
        // a Chinese filename must be no different from an ASCII one.
        let (frames, _, _, fmt) = drain(Path::new(MAIN10_CHINESE));

        assert_eq!(fmt, PIX_FMT_YUV420P10LE);
        assert_eq!(frames, FIXTURE_FRAMES);
    }

    #[test]
    fn truncated_mp4_header_reports_unsupported() {
        // moov sits at the end of generate.sh's mp4: a file cut inside its
        // header opens as *nothing this build recognises* → typed
        // UnsupportedFormat (exit 3 at the boundary) — the same verdict
        // image/truncated.png's lesson gave the M7 image path.
        let whole = fs::read(MAIN10_MP4).expect("fixture is readable");
        let cut = scratch("truncated-head.mp4");
        fs::write(&cut, &whole[..whole.len() / 3]).expect("write the cut");

        let err = HevcReader::open(&cut).expect_err("a headerless mp4 must not open");
        assert!(
            matches!(err, TossError::UnsupportedFormat(_)),
            "expected UnsupportedFormat, got {err:?}"
        );

        let _ = fs::remove_dir_all(scratch_dir());
    }

    #[test]
    fn truncated_mkv_tail_decodes_then_ends() {
        // matroska wears its header at the front: a file cut mid-stream
        // still opens, still decodes what is there, and ends cleanly —
        // frames first, then `Ok(None)`, never an error mid-delivery.
        let whole = fs::read(MAIN10_MKV).expect("fixture is readable");
        let cut = scratch("truncated-tail.mkv");
        fs::write(&cut, &whole[..whole.len() / 2]).expect("write the cut");

        let (frames, _, _, _) = drain(&cut);
        assert!(frames > 0, "the surviving clusters must still decode");

        let _ = fs::remove_dir_all(scratch_dir());
    }

    #[test]
    fn corrupt_bytes_report_corrupt_input() {
        // Recognised *and* damaged: the EBML header and the segment
        // header stay valid (bytes 0..52 — that is what makes the probe
        // score the container as matroska), while everything from the
        // Tracks element (offset 83, measured against generate.sh's
        // hevc.mkv: Info at 69, Tracks at 83) becomes garbage. The parser
        // logs errors and finds no video → typed CorruptInput (exit 6 at
        // the boundary). "6 is the file's verdict" is the rule design §3
        // keeps even on the software path — and killing the EBML header
        // instead would drop the file one class *earlier* (not recognised
        // at all → UnsupportedFormat), which is a different verdict.
        let mut bytes = fs::read(MAIN10_MKV).expect("fixture is readable");
        for byte in &mut bytes[83..512] {
            *byte = 0xAB;
        }
        let damaged = scratch("corrupt-header.mkv");
        fs::write(&damaged, bytes).expect("write the damage");

        let err =
            HevcReader::open(&damaged).expect_err("a file with destroyed tracks must not open");
        assert!(
            matches!(err, TossError::CorruptInput { .. }),
            "expected CorruptInput, got {err:?}"
        );

        let _ = fs::remove_dir_all(scratch_dir());
    }

    #[test]
    fn not_media_reports_unsupported() {
        // Dispatch decides what *claims* to be media; anything reaching
        // this reader that FFmpeg cannot open at all earns
        // UnsupportedFormat — never CorruptInput for a file we never read.
        let err =
            HevcReader::open(Path::new("Cargo.toml")).expect_err("Cargo.toml is not a media file");
        assert!(
            matches!(err, TossError::UnsupportedFormat(_)),
            "expected UnsupportedFormat, got {err:?}"
        );
        assert_eq!(
            err.exit_code(),
            crate::core::exit_code::ExitCode::UnsupportedFormat
        );
    }

    #[test]
    fn an_interior_nul_in_the_path_is_refused() {
        // §23: refuse rather than truncate — the same rule wide_path owns
        // on the MF side of this module's wall.
        let err =
            HevcReader::open(Path::new("clip\0.mkv")).expect_err("a NUL cannot be part of a path");
        assert!(matches!(err, TossError::InvalidArguments(_)), "got {err:?}");
    }
}
