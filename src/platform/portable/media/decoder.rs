//! The decode boundary of the portable player.
//!
//! One rule, the same one the Windows HEVC wrapper keeps: **FFmpeg's types
//! stop at `media_shim.c`.** What crosses into Rust is a session handle, a
//! description of the file and chunks of plain bytes — BGRA pictures and
//! interleaved 16-bit sound — so nothing above this file knows a codec
//! exists. Every `unsafe` of the decode path is in this file.
//!
//! The shim's error classes become typed errors here and only here:
//!
//! ```text
//! -1 / -2  (not a format this build knows / nothing in it this build plays)
//!     → TossError::UnsupportedFormat   → exit 3
//! -3       (recognised, bytes damaged)
//!     → TossError::CorruptInput        → exit 6
//! -4 / unknown
//!     → TossError::other               → exit 1
//! ```

use std::ffi::{CString, c_char, c_int, c_longlong, c_uchar};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::Once;

use crate::core::error::TossError;

/// Sample frames per second of every sound chunk.
pub(super) const RATE: u32 = 48_000;
/// Channels of every sound chunk, interleaved.
pub(super) const CHANNELS: u32 = 2;
/// Bytes of one sample frame: 16-bit samples.
pub(super) const FRAME_BYTES: usize = CHANNELS as usize * 2;
/// The unit of every time in this module: 100 ns, as on Windows.
pub(super) const SECOND: i64 = 10_000_000;

const KIND_VIDEO: c_int = 1;
const KIND_AUDIO: c_int = 2;

#[repr(C)]
struct Session {
    _opaque: [u8; 0],
}

#[repr(C)]
#[derive(Default)]
struct RawInfo {
    has_video: c_int,
    has_audio: c_int,
    skipped_video: c_int,
    skipped_audio: c_int,
    width: c_int,
    height: c_int,
    duration_100ns: c_longlong,
}

#[repr(C)]
struct RawChunk {
    kind: c_int,
    bytes: c_int,
    width: c_int,
    height: c_int,
    stride: c_int,
    frames: c_int,
    pts_100ns: c_longlong,
    duration_100ns: c_longlong,
    data: *const c_uchar,
}

unsafe extern "C" {
    fn toss_media_init();
    fn toss_media_open(utf8_path: *const c_char, err_out: *mut c_int) -> *mut Session;
    fn toss_media_info(session: *const Session, out: *mut RawInfo);
    fn toss_media_disable(session: *mut Session, kind: c_int);
    fn toss_media_set_size(session: *mut Session, width: c_int, height: c_int);
    fn toss_media_next(session: *mut Session, out: *mut RawChunk) -> c_int;
    fn toss_media_seek(session: *mut Session, pos_100ns: c_longlong) -> c_int;
    fn toss_media_close(session: *mut Session);
}

/// What a file holds, as far as this build can play it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Info {
    /// The picture's display size, when there is a picture to decode.
    pub(super) video: Option<(u32, u32)>,
    /// Whether there is sound to decode.
    pub(super) audio: bool,
    /// A picture stream exists but this build has no decoder for it.
    pub(super) skipped_video: bool,
    /// A sound stream exists but this build has no decoder for it.
    pub(super) skipped_audio: bool,
    /// Length of the presentation, when the container states one.
    pub(super) duration: Option<i64>,
}

/// One decoded piece of the presentation, borrowed from the session until
/// the next call into it.
pub(super) enum Chunk<'a> {
    Video {
        pts: i64,
        duration: i64,
        width: u32,
        height: u32,
        /// `width * height` BGRA pixels, rows top to bottom, no padding.
        pixels: &'a [u8],
    },
    Audio {
        pts: i64,
        duration: i64,
        /// Interleaved native-endian `i16` samples, [`CHANNELS`] per frame.
        samples: &'a [u8],
    },
}

/// One open file.
pub(super) struct Decoder {
    session: NonNull<Session>,
    path: PathBuf,
}

// SAFETY: the session is plain heap state with no thread affinity; the
// shim's contract is one thread *at a time*, which `&mut self` on every
// call that touches it enforces.
unsafe impl Send for Decoder {}

impl Decoder {
    /// Open `path` and the decoders for its best picture and sound streams.
    pub(super) fn open(path: &Path) -> Result<Self, TossError> {
        // Availability first, with the OS's own verdict: a missing or
        // locked file is not a statement about its contents.
        std::fs::File::open(path).map_err(|err| TossError::from_io(path, err))?;

        static INIT: Once = Once::new();
        // SAFETY: installs FFmpeg's log callback exactly once, before any
        // session exists.
        INIT.call_once(|| unsafe { toss_media_init() });

        let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            TossError::invalid_arguments(format!("{} is not a usable path", path.display()))
        })?;
        let mut code: c_int = 0;
        // SAFETY: both pointers are valid for the call; the shim copies
        // what it needs from the path.
        let session = unsafe { toss_media_open(c_path.as_ptr(), &mut code) };
        match NonNull::new(session) {
            Some(session) => Ok(Self {
                session,
                path: path.to_path_buf(),
            }),
            None => Err(from_shim(path, "open", code)),
        }
    }

    pub(super) fn info(&self) -> Info {
        let mut raw = RawInfo::default();
        // SAFETY: the session is open and `raw` is a valid out-parameter.
        unsafe { toss_media_info(self.session.as_ptr(), &mut raw) };
        let video = (raw.has_video != 0 && raw.width > 0 && raw.height > 0)
            .then_some((raw.width as u32, raw.height as u32));
        Info {
            video,
            audio: raw.has_audio != 0,
            skipped_video: raw.skipped_video != 0,
            skipped_audio: raw.skipped_audio != 0,
            duration: (raw.duration_100ns > 0).then_some(raw.duration_100ns),
        }
    }

    /// Stop decoding pictures for the rest of the session.
    pub(super) fn disable_video(&mut self) {
        // SAFETY: the session is open; the shim tolerates a stream that is
        // already absent.
        unsafe { toss_media_disable(self.session.as_ptr(), KIND_VIDEO) };
    }

    /// Stop decoding sound for the rest of the session.
    pub(super) fn disable_audio(&mut self) {
        // SAFETY: as in `disable_video`.
        unsafe { toss_media_disable(self.session.as_ptr(), KIND_AUDIO) };
    }

    /// The size pictures are produced at from the next chunk on.
    pub(super) fn set_size(&mut self, width: u32, height: u32) {
        let clamp = |value: u32| value.min(16_384) as c_int;
        // SAFETY: the session is open; the shim validates the numbers.
        unsafe { toss_media_set_size(self.session.as_ptr(), clamp(width), clamp(height)) };
    }

    /// The next chunk, `None` at the end of the presentation.
    pub(super) fn next(&mut self) -> Result<Option<Chunk<'_>>, TossError> {
        let mut raw = RawChunk {
            kind: 0,
            bytes: 0,
            width: 0,
            height: 0,
            stride: 0,
            frames: 0,
            pts_100ns: 0,
            duration_100ns: 0,
            data: std::ptr::null(),
        };
        // SAFETY: the session is open and `raw` is a valid out-parameter.
        let code = unsafe { toss_media_next(self.session.as_ptr(), &mut raw) };
        match code {
            0 => return Ok(None),
            1 => {}
            code => return Err(from_shim(&self.path, "decode", code)),
        }
        if raw.data.is_null() || raw.bytes <= 0 {
            return Err(TossError::other("media decoder returned an empty chunk"));
        }
        // SAFETY: on a `1` the shim guarantees `data` points at `bytes`
        // readable bytes owned by the session, valid until the next call
        // into it — which the returned borrow of `self` forbids.
        let data = unsafe { std::slice::from_raw_parts(raw.data, raw.bytes as usize) };
        match raw.kind {
            KIND_VIDEO => {
                let (width, height) = (raw.width.max(0) as u32, raw.height.max(0) as u32);
                if raw.stride != raw.width * 4
                    || data.len() != width as usize * height as usize * 4
                    || data.is_empty()
                {
                    return Err(TossError::other(
                        "media decoder returned a malformed picture",
                    ));
                }
                Ok(Some(Chunk::Video {
                    pts: raw.pts_100ns,
                    duration: raw.duration_100ns.max(1),
                    width,
                    height,
                    pixels: data,
                }))
            }
            KIND_AUDIO => {
                if data.len() % FRAME_BYTES != 0 || data.len() != raw.frames as usize * FRAME_BYTES
                {
                    return Err(TossError::other("media decoder returned malformed sound"));
                }
                Ok(Some(Chunk::Audio {
                    pts: raw.pts_100ns,
                    duration: raw.duration_100ns.max(1),
                    samples: data,
                }))
            }
            _ => Err(TossError::other("media decoder returned an unknown chunk")),
        }
    }

    /// Jump to the key frame at or before `position`. Chunks between that
    /// key frame and the position still come out; the caller drops them.
    pub(super) fn seek(&mut self, position: i64) -> Result<(), TossError> {
        // SAFETY: the session is open.
        let code = unsafe { toss_media_seek(self.session.as_ptr(), position.max(0)) };
        if code == 0 {
            Ok(())
        } else {
            Err(TossError::other(format!(
                "could not seek in {}",
                self.path.display()
            )))
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // SAFETY: the session came from `toss_media_open` and is closed
        // exactly once, here.
        unsafe { toss_media_close(self.session.as_ptr()) };
    }
}

fn from_shim(path: &Path, step: &str, code: c_int) -> TossError {
    match code {
        -1 | -2 => TossError::UnsupportedFormat(path.to_path_buf()),
        -3 => TossError::CorruptInput {
            path: path.to_path_buf(),
            context: format!("{step}: the media data is damaged or incomplete"),
        },
        code => TossError::other(format!(
            "media decoder failed to {step} {} (code {code})",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{Chunk, Decoder, FRAME_BYTES, SECOND};
    use crate::core::error::TossError;

    fn corpus(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("corpus")
            .join("media")
            .join(name)
    }

    /// Decode a whole file: (pictures, sound frames, last picture size).
    fn drain(decoder: &mut Decoder) -> (usize, usize, (u32, u32)) {
        let (mut pictures, mut frames, mut size) = (0, 0, (0, 0));
        while let Some(chunk) = decoder.next().expect("the sample decodes") {
            match chunk {
                Chunk::Video { width, height, .. } => {
                    pictures += 1;
                    size = (width, height);
                }
                Chunk::Audio { samples, .. } => frames += samples.len() / FRAME_BYTES,
            }
        }
        (pictures, frames, size)
    }

    #[test]
    fn every_video_sample_yields_pictures_and_a_second_of_sound() {
        for name in ["tone.mp4", "tone.mkv", "tone.webm", "tone.avi", "tone.mov"] {
            let mut decoder =
                Decoder::open(&corpus(name)).unwrap_or_else(|err| panic!("{name}: {err}"));
            let info = decoder.info();
            assert_eq!(info.video, Some((160, 120)), "{name}");
            assert!(info.audio, "{name}");
            assert!(!info.skipped_video && !info.skipped_audio, "{name}");
            let (pictures, frames, size) = drain(&mut decoder);
            assert!(pictures >= 10, "{name}: {pictures} pictures");
            assert_eq!(size, (160, 120), "{name}");
            // One second of tone at 48 kHz, give or take codec padding.
            assert!(
                (40_000..=56_000).contains(&frames),
                "{name}: {frames} frames"
            );
        }
    }

    #[test]
    fn every_audio_sample_is_sound_only_and_audible() {
        for name in ["tone.wav", "tone.mp3", "tone.flac"] {
            let mut decoder =
                Decoder::open(&corpus(name)).unwrap_or_else(|err| panic!("{name}: {err}"));
            let info = decoder.info();
            assert_eq!(info.video, None, "{name}");
            assert!(info.audio, "{name}");
            let duration = info.duration.expect("the sample states its length");
            assert!(
                (SECOND * 9 / 10..=SECOND * 12 / 10).contains(&duration),
                "{name}"
            );

            let mut loudest = 0_i32;
            let mut frames = 0;
            while let Some(chunk) = decoder.next().unwrap() {
                let Chunk::Audio { samples, .. } = chunk else {
                    panic!("{name}: a picture came out of a sound file");
                };
                frames += samples.len() / FRAME_BYTES;
                for sample in samples.as_chunks::<2>().0 {
                    loudest = loudest.max(i32::from(i16::from_ne_bytes(*sample)).abs());
                }
            }
            assert!(
                (40_000..=56_000).contains(&frames),
                "{name}: {frames} frames"
            );
            if name == "tone.wav" {
                // Exactly 44 100 frames in, so exactly a second out: the
                // resampler's last few samples are flushed, not dropped.
                assert!(
                    frames.abs_diff(super::RATE as usize) <= 1,
                    "{name}: {frames} frames"
                );
            }
            assert!(loudest > 1_000, "{name}: the tone decoded as silence");
        }
    }

    #[test]
    fn hevc_samples_decode_including_a_non_ascii_path() {
        for name in ["hevc.mkv", "hevc.mp4", "hevc 中文.mkv"] {
            let mut decoder =
                Decoder::open(&corpus(name)).unwrap_or_else(|err| panic!("{name}: {err}"));
            assert!(decoder.info().video.is_some(), "{name}");
            let (pictures, _, _) = drain(&mut decoder);
            assert!(pictures > 0, "{name}");
        }
    }

    #[test]
    fn pictures_come_out_at_the_size_asked_for() {
        let mut decoder = Decoder::open(&corpus("tone.mp4")).unwrap();
        decoder.set_size(320, 240);
        let (_, _, size) = drain(&mut decoder);
        assert_eq!(size, (320, 240));
    }

    #[test]
    fn a_disabled_stream_produces_nothing() {
        let mut decoder = Decoder::open(&corpus("tone.mp4")).unwrap();
        decoder.disable_video();
        let (pictures, frames, _) = drain(&mut decoder);
        assert_eq!(pictures, 0);
        assert!(frames > 40_000);

        let mut decoder = Decoder::open(&corpus("tone.mp4")).unwrap();
        decoder.disable_audio();
        let (pictures, frames, _) = drain(&mut decoder);
        assert!(pictures >= 10);
        assert_eq!(frames, 0);
    }

    #[test]
    fn seeking_back_to_the_start_plays_the_file_again() {
        let mut decoder = Decoder::open(&corpus("tone.mkv")).unwrap();
        let first = drain(&mut decoder);
        decoder.seek(0).unwrap();
        let again = drain(&mut decoder);
        assert_eq!(first.0, again.0);
        assert!(again.1 > 40_000);
    }

    #[test]
    fn seeking_lands_at_or_before_the_position() {
        let mut decoder = Decoder::open(&corpus("tone.flac")).unwrap();
        decoder.seek(SECOND / 2).unwrap();
        let Some(Chunk::Audio { pts, .. }) = decoder.next().unwrap() else {
            panic!("no sound after the seek");
        };
        assert!(pts <= SECOND / 2, "landed at {pts}");
    }

    #[test]
    fn a_cut_off_container_is_corrupt_and_other_files_are_not_media() {
        assert!(matches!(
            Decoder::open(&corpus("truncated.mp4")),
            Err(TossError::CorruptInput { .. })
        ));
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(matches!(
            Decoder::open(&manifest),
            Err(TossError::UnsupportedFormat(_))
        ));
        assert!(matches!(
            Decoder::open(&corpus("no-such-file.mp4")),
            Err(TossError::InputNotFound(_))
        ));
    }
}
