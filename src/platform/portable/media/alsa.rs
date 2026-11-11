//! Sound output through ALSA, found at run time.
//!
//! `libasound` is the one sound interface every Linux desktop answers to:
//! PipeWire and PulseAudio both ship an ALSA front end, and a bare machine
//! has the real thing. It is still loaded with `dlopen` rather than linked,
//! because the single `toss` binary has to *start* on a server or a
//! container that has no sound library at all — there it loses sound, in a
//! sentence, and nothing else.
//!
//! The format is fixed and matches what the decoder produces, so this file
//! never converts anything: interleaved native-endian 16-bit samples,
//! [`CHANNELS`] channels at [`RATE`] Hz, on the `default` device. ALSA
//! resamples for hardware that wants something else.
//!
//! Every `unsafe` of the sound path is in this file.

use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::ptr::NonNull;

use super::decoder::{CHANNELS, FRAME_BYTES, RATE};

/// How much sound the device holds ahead of the speaker. Long enough to
/// ride out a slow decode, short enough that pause and volume feel prompt.
const LATENCY_US: c_uint = 100_000;

const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
#[cfg(target_endian = "little")]
const SND_PCM_FORMAT_S16: c_int = 2; // S16_LE
#[cfg(target_endian = "big")]
const SND_PCM_FORMAT_S16: c_int = 3; // S16_BE

type ErrorHandler =
    unsafe extern "C" fn(*const c_char, c_int, *const c_char, c_int, *const c_char, ...);

unsafe extern "C" {
    /// The do-nothing ALSA error handler, defined in `media_shim.c` because
    /// Rust cannot define a variadic function.
    fn toss_media_quiet(
        file: *const c_char,
        line: c_int,
        function: *const c_char,
        err: c_int,
        fmt: *const c_char,
        ...
    );
}

type Pcm = *mut c_void;
type OpenFn = unsafe extern "C" fn(*mut Pcm, *const c_char, c_int, c_int) -> c_int;
type SetParamsFn = unsafe extern "C" fn(Pcm, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
type WriteFn = unsafe extern "C" fn(Pcm, *const c_void, c_ulong) -> c_long;
type RecoverFn = unsafe extern "C" fn(Pcm, c_int, c_int) -> c_int;
type DelayFn = unsafe extern "C" fn(Pcm, *mut c_long) -> c_int;
type PauseFn = unsafe extern "C" fn(Pcm, c_int) -> c_int;
type PlainFn = unsafe extern "C" fn(Pcm) -> c_int;
type StrerrorFn = unsafe extern "C" fn(c_int) -> *const c_char;
type SetHandlerFn = unsafe extern "C" fn(Option<ErrorHandler>) -> c_int;

/// The handful of `libasound` entry points playback needs.
struct Api {
    open: OpenFn,
    set_params: SetParamsFn,
    writei: WriteFn,
    recover: RecoverFn,
    delay: DelayFn,
    pause: PauseFn,
    drop: PlainFn,
    prepare: PlainFn,
    drain: PlainFn,
    close: PlainFn,
    strerror: StrerrorFn,
}

impl Api {
    /// Load `libasound` and resolve every entry point, or say what is
    /// missing. The library is never unloaded: it is needed for as long as
    /// the process plays anything, and unloading a sound library that has
    /// started threads is not safe anyway.
    fn load() -> Result<Self, String> {
        // SAFETY: a constant, NUL-terminated library name.
        let library = unsafe { libc::dlopen(c"libasound.so.2".as_ptr(), libc::RTLD_NOW) };
        if library.is_null() {
            return Err("the ALSA sound library (libasound.so.2) is not installed".to_owned());
        }

        /// Resolve one symbol as a function pointer of the named type.
        macro_rules! symbol {
            ($name:literal as $kind:ty) => {{
                // SAFETY: `library` is a live handle and the name is a
                // constant, NUL-terminated string.
                let address = unsafe { libc::dlsym(library, $name.as_ptr()) };
                if address.is_null() {
                    return Err(format!(
                        "the ALSA sound library lacks {}",
                        $name.to_string_lossy()
                    ));
                }
                // SAFETY: the symbol is libasound's function of this name,
                // whose C signature is the one `$kind` spells out;
                // a data pointer and a function pointer are the same size
                // on every platform `dlsym` exists on.
                unsafe { std::mem::transmute::<*mut c_void, $kind>(address) }
            }};
        }

        // ALSA writes its own diagnostics to stderr unless told otherwise;
        // stderr is Toss's to word.
        let set_handler = symbol!(c"snd_lib_error_set_handler" as SetHandlerFn);
        // SAFETY: the handler is a valid variadic C function that ignores
        // its arguments and lives for the whole process.
        unsafe { set_handler(Some(toss_media_quiet)) };

        Ok(Self {
            open: symbol!(c"snd_pcm_open" as OpenFn),
            set_params: symbol!(c"snd_pcm_set_params" as SetParamsFn),
            writei: symbol!(c"snd_pcm_writei" as WriteFn),
            recover: symbol!(c"snd_pcm_recover" as RecoverFn),
            delay: symbol!(c"snd_pcm_delay" as DelayFn),
            pause: symbol!(c"snd_pcm_pause" as PauseFn),
            drop: symbol!(c"snd_pcm_drop" as PlainFn),
            prepare: symbol!(c"snd_pcm_prepare" as PlainFn),
            drain: symbol!(c"snd_pcm_drain" as PlainFn),
            close: symbol!(c"snd_pcm_close" as PlainFn),
            strerror: symbol!(c"snd_strerror" as StrerrorFn),
        })
    }

    fn describe(&self, code: c_int) -> String {
        // SAFETY: `snd_strerror` returns a static, NUL-terminated string
        // for any code.
        let text = unsafe { (self.strerror)(code) };
        if text.is_null() {
            return format!("ALSA error {code}");
        }
        // SAFETY: non-null and NUL-terminated, as above.
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    }
}

/// An open playback device.
pub(super) struct Output {
    api: Api,
    pcm: NonNull<c_void>,
}

// SAFETY: a PCM handle has no thread affinity; it is used by one thread at
// a time, which `&mut self` on every call enforces.
unsafe impl Send for Output {}

impl Output {
    /// Open the default playback device, or say in a sentence why this
    /// machine cannot play sound.
    pub(super) fn open() -> Result<Self, String> {
        let api = Api::load()?;
        let mut pcm: *mut c_void = std::ptr::null_mut();
        // SAFETY: `pcm` is a valid out-parameter and the device name is a
        // constant, NUL-terminated string.
        let code = unsafe { (api.open)(&mut pcm, c"default".as_ptr(), SND_PCM_STREAM_PLAYBACK, 0) };
        let Some(pcm) = NonNull::new(pcm).filter(|_| code >= 0) else {
            return Err(format!("no sound device ({})", api.describe(code)));
        };
        let output = Self { api, pcm };
        // SAFETY: the handle is open. `1` lets ALSA resample for hardware
        // that does not run at this rate.
        let code = unsafe {
            (output.api.set_params)(
                output.pcm.as_ptr(),
                SND_PCM_FORMAT_S16,
                SND_PCM_ACCESS_RW_INTERLEAVED,
                CHANNELS,
                RATE,
                1,
                LATENCY_US,
            )
        };
        if code < 0 {
            return Err(format!(
                "the sound device refused 16-bit stereo ({})",
                output.api.describe(code)
            ));
        }
        Ok(output)
    }

    /// Play `samples` (whole frames), blocking until the device has taken
    /// them all. An underrun is recovered from silently — a gap in sound is
    /// a moment, not a reason to stop the film.
    pub(super) fn write(&mut self, samples: &[u8]) -> Result<(), String> {
        let mut rest = &samples[..samples.len() - samples.len() % FRAME_BYTES];
        while !rest.is_empty() {
            let frames = (rest.len() / FRAME_BYTES) as c_ulong;
            // SAFETY: `rest` holds `frames` whole frames of readable bytes
            // and the handle is open.
            let written =
                unsafe { (self.api.writei)(self.pcm.as_ptr(), rest.as_ptr().cast(), frames) };
            if written < 0 {
                // SAFETY: the handle is open; `1` keeps ALSA quiet.
                let code = unsafe { (self.api.recover)(self.pcm.as_ptr(), written as c_int, 1) };
                if code < 0 {
                    return Err(format!("sound output failed ({})", self.api.describe(code)));
                }
                continue;
            }
            let taken = (written as usize).min(frames as usize) * FRAME_BYTES;
            if taken == 0 {
                // A device that accepts nothing and reports no error would
                // spin this loop forever.
                return Err("sound output stalled".to_owned());
            }
            rest = &rest[taken..];
        }
        Ok(())
    }

    /// Sample frames written but not yet heard. Zero when the device cannot
    /// say (it has underrun, or is a sink that keeps no time).
    pub(super) fn delay_frames(&mut self) -> i64 {
        let mut frames: c_long = 0;
        // SAFETY: the handle is open and `frames` is a valid out-parameter.
        let code = unsafe { (self.api.delay)(self.pcm.as_ptr(), &mut frames) };
        if code < 0 {
            0
        } else {
            // `c_long` is narrower than `i64` on 32-bit targets.
            #[allow(clippy::useless_conversion)]
            let frames = i64::from(frames);
            frames.max(0)
        }
    }

    /// Hold or release the device. Hardware that cannot pause simply plays
    /// out the little it holds, which the clock accounts for.
    pub(super) fn pause(&mut self, paused: bool) {
        // SAFETY: the handle is open.
        unsafe { (self.api.pause)(self.pcm.as_ptr(), c_int::from(paused)) };
    }

    /// Throw away what the device holds and make it ready for new sound —
    /// what a seek needs.
    pub(super) fn reset(&mut self) {
        // SAFETY: the handle is open; both calls are valid in any state.
        unsafe {
            (self.api.drop)(self.pcm.as_ptr());
            (self.api.prepare)(self.pcm.as_ptr());
        }
    }

    /// Block until everything written has been heard, then make the device
    /// ready again (a seek back from the end plays more).
    pub(super) fn drain(&mut self) {
        // SAFETY: as in `reset`.
        unsafe {
            (self.api.drain)(self.pcm.as_ptr());
            (self.api.prepare)(self.pcm.as_ptr());
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        // SAFETY: the handle came from `snd_pcm_open` and is closed exactly
        // once, here. Anything still queued is discarded first so closing
        // never blocks.
        unsafe {
            (self.api.drop)(self.pcm.as_ptr());
            (self.api.close)(self.pcm.as_ptr());
        }
    }
}
