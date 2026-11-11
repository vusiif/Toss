//! The playback engine: who decodes, who plays sound, and what time it is.
//!
//! Three parties share one small piece of state behind a mutex:
//!
//! - the **decode thread** owns the decoder and keeps two short queues
//!   topped up — pictures and sound — and nothing else;
//! - the **sound thread** owns the output device, feeds it from the sound
//!   queue and reports how far the speaker has got;
//! - the **caller** (a window or the terminal) asks what time it is, takes
//!   the picture that is due, and issues the few commands a player has.
//!
//! **Sound is the clock.** When a file has sound and the machine can play
//! it, "now" is the last sample handed to the device minus what the device
//! still holds, so pictures follow what is actually heard and a stalled
//! device stalls the film instead of letting it drift. Without sound — a
//! silent film, or after the sound has run out — the clock is the wall
//! clock from an anchor.
//!
//! A seek is a change of **generation**: queues are emptied, the number is
//! bumped, and anything still in flight that was decoded for the old
//! generation is dropped where it lands. No thread is ever told to stop
//! what it is doing mid-call; each notices the new number the next time it
//! looks.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::core::error::TossError;
use crate::core::log;

use super::alsa::Output;
use super::decoder::{Chunk, Decoder, FRAME_BYTES, RATE, SECOND};

/// Pictures kept decoded ahead of the one on screen.
const VIDEO_TARGET: usize = 3;
/// Sound kept decoded ahead of the device: short, so volume and pause are
/// felt at once, and long enough to cover a slow picture.
const AUDIO_TARGET: i64 = SECOND / 5;
/// Hard ceilings for files whose pictures and sound are stored far apart:
/// decoding waits rather than letting either queue grow without bound.
const AUDIO_LIMIT: i64 = 5 * SECOND;
const VIDEO_LIMIT_BYTES: usize = 192 * 1024 * 1024;
const VIDEO_LIMIT_FRAMES: usize = 32;
/// A hole in the sound track longer than this is played as silence, so the
/// clock keeps time across it.
const GAP: i64 = SECOND / 20;
/// One volume step, as on Windows.
pub(super) const VOLUME_STEP: f32 = 0.1;

/// One picture, ready to show.
pub(super) struct Frame {
    pub(super) pts: i64,
    pub(super) width: u32,
    pub(super) height: u32,
    /// BGRA, rows top to bottom, no padding.
    pub(super) pixels: Vec<u8>,
}

struct Sound {
    pts: i64,
    duration: i64,
    samples: Vec<u8>,
}

enum Clock {
    /// Time runs with the wall clock from `since`; frozen at `position`
    /// while `since` is `None`.
    Wall {
        position: i64,
        since: Option<Instant>,
    },
    /// Time is what the device has played: `written_end` less the `delay`
    /// it reported at `stamp`, advancing until the next report but never
    /// past what was written.
    Sound {
        written_end: i64,
        delay: i64,
        stamp: Option<Instant>,
    },
}

fn elapsed(since: Instant) -> i64 {
    i64::try_from(since.elapsed().as_nanos() / 100).unwrap_or(i64::MAX)
}

struct State {
    video: VecDeque<Frame>,
    /// Picture buffers waiting to be reused, so steady playback allocates
    /// nothing per frame.
    spare: Vec<Vec<u8>>,
    audio: VecDeque<Sound>,
    audio_queued: i64,

    generation: u64,
    seek: Option<i64>,
    /// After a seek, chunks ending at or before this are decoded and
    /// dropped: the decoder restarts at a key frame before the target.
    skip_until: i64,
    size: Option<(u32, u32)>,

    paused: bool,
    /// The position shown while paused.
    frozen: i64,
    stop: bool,
    /// The decoder has reached the end of this generation.
    decoded_all: bool,
    /// Everything decoded has been heard.
    audio_finished: bool,
    /// The device failed; the rest plays silent.
    audio_dead: bool,
    /// When the last picture decoded so far stops being due.
    video_end: i64,
    error: Option<TossError>,

    clock: Clock,
    volume: f32,
}

impl State {
    fn position(&self) -> i64 {
        if self.paused {
            return self.frozen;
        }
        match self.clock {
            Clock::Wall { position, since } => {
                position.saturating_add(since.map(elapsed).unwrap_or(0))
            }
            Clock::Sound {
                written_end,
                delay,
                stamp,
            } => match stamp {
                Some(stamp) => (written_end - delay)
                    .saturating_add(elapsed(stamp))
                    .min(written_end),
                None => written_end,
            },
        }
    }

    fn recycle(&mut self, pixels: Vec<u8>) {
        if self.spare.len() < VIDEO_TARGET + 2 {
            self.spare.push(pixels);
        }
    }

    /// Empty both queues and start a new generation at `position`.
    fn restart(&mut self, position: i64, sound: bool) {
        while let Some(frame) = self.video.pop_front() {
            self.recycle(frame.pixels);
        }
        self.audio.clear();
        self.audio_queued = 0;
        self.generation += 1;
        self.skip_until = position;
        self.frozen = position;
        self.video_end = position;
        self.decoded_all = false;
        self.audio_finished = false;
        self.clock = if sound && !self.audio_dead {
            Clock::Sound {
                written_end: position,
                delay: 0,
                stamp: None,
            }
        } else {
            Clock::Wall {
                position,
                since: None,
            }
        };
    }
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    has_video: bool,
    has_audio: bool,
    duration: Option<i64>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panicked thread must not take the player's teardown with it.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn wait<'a>(&self, guard: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.wake
            .wait(guard)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn sound_plays(&self, state: &State) -> bool {
        self.has_audio && !state.audio_dead
    }

    /// Whether the decode thread should produce another chunk.
    fn wants_more(&self, state: &State) -> bool {
        let sound = self.sound_plays(state);
        let video_need = self.has_video && state.video.len() < VIDEO_TARGET;
        let audio_need = sound && state.audio_queued < AUDIO_TARGET;
        let frame_bytes = state
            .video
            .back()
            .map_or(1, |frame| frame.pixels.len().max(1));
        let frame_limit =
            (VIDEO_LIMIT_BYTES / frame_bytes).clamp(VIDEO_TARGET + 1, VIDEO_LIMIT_FRAMES);
        let video_full = state.video.len() >= frame_limit;
        let audio_full = state.audio_queued >= AUDIO_LIMIT;
        (video_need || audio_need) && !video_full && !audio_full
    }

    fn finished(&self, state: &State) -> bool {
        if state.error.is_some() {
            return true;
        }
        if !state.decoded_all || state.seek.is_some() {
            return false;
        }
        let sound_done = !self.sound_plays(state) || state.audio_finished;
        let video_done =
            !self.has_video || (state.video.is_empty() && state.position() >= state.video_end);
        sound_done && video_done
    }
}

/// What the caller learns each time it asks for a picture.
pub(super) struct Poll {
    /// The picture that has become due, if a new one has.
    pub(super) frame: Option<Frame>,
    /// How long until it is worth asking again.
    pub(super) wait: Duration,
    /// The presentation is over (or has failed: see [`Engine::finish`]).
    pub(super) finished: bool,
}

/// A running playback session. Dropping it stops and joins both threads.
pub(super) struct Engine {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Engine {
    /// Start playing. `decoder` must already have had the streams nobody
    /// will receive disabled; `output` is the sound device when the file
    /// has sound and the machine can play it.
    pub(super) fn start(decoder: Decoder, output: Option<Output>) -> Self {
        let info = decoder.info();
        let has_audio = info.audio && output.is_some();
        let mut state = State {
            video: VecDeque::new(),
            spare: Vec::new(),
            audio: VecDeque::new(),
            audio_queued: 0,
            generation: 0,
            seek: None,
            skip_until: 0,
            size: None,
            paused: false,
            frozen: 0,
            stop: false,
            decoded_all: false,
            audio_finished: false,
            audio_dead: false,
            video_end: 0,
            error: None,
            clock: Clock::Wall {
                position: 0,
                since: None,
            },
            volume: 1.0,
        };
        state.restart(0, has_audio);
        let shared = Arc::new(Shared {
            state: Mutex::new(state),
            wake: Condvar::new(),
            has_video: info.video.is_some(),
            has_audio,
            duration: info.duration,
        });

        let mut threads = Vec::new();
        let for_decode = shared.clone();
        threads.push(std::thread::spawn(move || {
            decode_loop(&for_decode, decoder)
        }));
        if let Some(output) = output.filter(|_| has_audio) {
            let for_sound = shared.clone();
            threads.push(std::thread::spawn(move || sound_loop(&for_sound, output)));
        }
        Self { shared, threads }
    }

    /// Where playback is, in 100-ns units from the start.
    #[cfg(test)]
    pub(super) fn position(&self) -> i64 {
        self.shared.lock().position()
    }

    /// The length of the presentation, when the file states one.
    pub(super) fn duration(&self) -> Option<i64> {
        self.shared.duration
    }

    pub(super) fn finished(&self) -> bool {
        let state = self.shared.lock();
        self.shared.finished(&state)
    }

    /// Pause or resume; returns whether playback is now paused.
    pub(super) fn toggle_pause(&self) -> bool {
        let mut state = self.shared.lock();
        if state.paused {
            let position = state.frozen;
            state.paused = false;
            // Re-anchor so that "now" is exactly where the pause held it.
            match &mut state.clock {
                Clock::Wall {
                    position: anchor,
                    since,
                } => {
                    *anchor = position;
                    // A clock that had not started yet still starts with
                    // the first picture, as it would have.
                    if since.is_some() {
                        *since = Some(Instant::now());
                    }
                }
                Clock::Sound {
                    written_end,
                    delay,
                    stamp,
                } => {
                    if stamp.is_some() {
                        *delay = (*written_end - position).max(0);
                        *stamp = Some(Instant::now());
                    }
                }
            }
        } else {
            state.frozen = state.position();
            state.paused = true;
        }
        let paused = state.paused;
        drop(state);
        self.shared.wake.notify_all();
        paused
    }

    /// Jump by `delta` from the current position; returns where to.
    pub(super) fn seek_by(&self, delta: i64) -> i64 {
        let mut state = self.shared.lock();
        let mut target = state.position().saturating_add(delta).max(0);
        if let Some(duration) = self.shared.duration {
            target = target.min(duration);
        }
        let sound = self.shared.has_audio;
        state.restart(target, sound);
        state.seek = Some(target);
        drop(state);
        self.shared.wake.notify_all();
        target
    }

    /// Change the volume by `steps` steps; returns the new level, 0 to 1.
    pub(super) fn change_volume(&self, steps: i32) -> f32 {
        let mut state = self.shared.lock();
        let level = (state.volume + steps as f32 * VOLUME_STEP).clamp(0.0, 1.0);
        // Snap to the step grid so ten steps down is exactly silence.
        state.volume = (level / VOLUME_STEP).round() * VOLUME_STEP;
        state.volume
    }

    /// Whether sound is being played (the file has it and a device took it).
    #[cfg(test)]
    pub(super) fn has_sound(&self) -> bool {
        let state = self.shared.lock();
        self.shared.sound_plays(&state)
    }

    /// Ask for pictures at this size from now on.
    pub(super) fn set_size(&self, width: u32, height: u32) {
        self.shared.lock().size = Some((width, height));
        self.shared.wake.notify_all();
    }

    /// Take the picture that is due, handing back the one it replaces.
    pub(super) fn poll_video(&self, replaced: Option<Frame>) -> Poll {
        let mut state = self.shared.lock();
        if let Some(frame) = replaced {
            state.recycle(frame.pixels);
        }
        // A wall clock starts with the first picture it has to show, not
        // while the decoder is still warming up.
        let ready = !state.paused && (!state.video.is_empty() || state.decoded_all);
        if ready
            && let Clock::Wall { since, .. } = &mut state.clock
            && since.is_none()
        {
            *since = Some(Instant::now());
        }

        let position = state.position();
        let mut due: Option<Frame> = None;
        while state
            .video
            .front()
            .is_some_and(|frame| frame.pts <= position)
        {
            let frame = state.video.pop_front().expect("front was just seen");
            if let Some(late) = due.replace(frame) {
                state.recycle(late.pixels);
            }
        }
        let wait = if state.paused {
            Duration::from_millis(100)
        } else {
            match state.video.front() {
                Some(next) => {
                    let ahead = (next.pts - position).clamp(SECOND / 1000, SECOND / 10);
                    Duration::from_nanos(ahead as u64 * 100)
                }
                None => Duration::from_millis(5),
            }
        };
        let finished = self.shared.finished(&state);
        drop(state);
        if due.is_some() {
            self.shared.wake.notify_all();
        }
        Poll {
            frame: due,
            wait,
            finished,
        }
    }

    /// Stop playback and report how it ended: `Ok` for a presentation that
    /// played (or was closed), the decode error for one that broke.
    pub(super) fn finish(mut self) -> Result<(), TossError> {
        self.halt();
        match self.shared.lock().error.take() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    fn halt(&mut self) {
        self.shared.lock().stop = true;
        self.shared.wake.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.halt();
    }
}

fn decode_loop(shared: &Shared, mut decoder: Decoder) {
    let mut sound_disabled = false;
    loop {
        let (generation, seek, size, drop_sound) = {
            let mut state = shared.lock();
            loop {
                if state.stop {
                    return;
                }
                if state.seek.is_some() || state.size.is_some() {
                    break;
                }
                if !state.decoded_all && shared.wants_more(&state) {
                    break;
                }
                state = shared.wait(state);
            }
            (
                state.generation,
                state.seek.take(),
                state.size.take(),
                state.audio_dead,
            )
        };

        if drop_sound && !sound_disabled {
            decoder.disable_audio();
            sound_disabled = true;
        }
        if let Some((width, height)) = size {
            decoder.set_size(width, height);
        }
        if let Some(target) = seek {
            // A file that cannot seek keeps decoding from where it is; the
            // skip rule then reaches a later target the slow way.
            let _ = decoder.seek(target);
        }

        let chunk = match decoder.next() {
            Ok(Some(chunk)) => chunk,
            Ok(None) => {
                let mut state = shared.lock();
                if state.generation == generation && state.seek.is_none() {
                    state.decoded_all = true;
                }
                drop(state);
                shared.wake.notify_all();
                continue;
            }
            Err(err) => {
                let mut state = shared.lock();
                state.error = Some(err);
                state.decoded_all = true;
                drop(state);
                shared.wake.notify_all();
                continue;
            }
        };

        match chunk {
            Chunk::Video {
                pts,
                duration,
                width,
                height,
                pixels,
            } => {
                let mut buffer = {
                    let mut state = shared.lock();
                    if state.generation != generation || pts + duration <= state.skip_until {
                        continue;
                    }
                    state.spare.pop().unwrap_or_default()
                };
                buffer.clear();
                buffer.extend_from_slice(pixels);
                let mut state = shared.lock();
                if state.generation != generation {
                    state.recycle(buffer);
                    continue;
                }
                state.video_end = pts + duration;
                state.video.push_back(Frame {
                    pts,
                    width,
                    height,
                    pixels: buffer,
                });
            }
            Chunk::Audio {
                pts,
                duration,
                samples,
            } => {
                let samples = samples.to_vec();
                let mut state = shared.lock();
                if state.generation != generation
                    || state.audio_dead
                    || pts + duration <= state.skip_until
                {
                    continue;
                }
                state.audio_queued += duration;
                state.audio.push_back(Sound {
                    pts,
                    duration,
                    samples,
                });
            }
        }
        shared.wake.notify_all();
    }
}

fn sound_loop(shared: &Shared, mut output: Output) {
    let mut generation = 0;
    let mut held = false;
    let mut expected_end: Option<i64> = None;
    let mut scratch: Vec<u8> = Vec::new();

    loop {
        let (sound, volume) = {
            let mut state = shared.lock();
            loop {
                if state.stop {
                    return;
                }
                if state.generation != generation {
                    generation = state.generation;
                    output.reset();
                    held = false;
                    expected_end = None;
                }
                if state.paused {
                    if !held {
                        output.pause(true);
                        held = true;
                    }
                } else {
                    if held {
                        output.pause(false);
                        held = false;
                    }
                    if let Some(sound) = state.audio.pop_front() {
                        state.audio_queued -= sound.duration;
                        break (sound, state.volume);
                    }
                    if state.decoded_all && state.seek.is_none() && !state.audio_finished {
                        // The last sample is written; wait until it has
                        // been heard, then hand the clock to the wall so a
                        // picture track that runs longer still ends.
                        drop(state);
                        output.drain();
                        state = shared.lock();
                        if state.generation == generation && !state.stop {
                            let position = match state.clock {
                                Clock::Sound { written_end, .. } => written_end,
                                Clock::Wall { position, .. } => position,
                            };
                            state.audio_finished = true;
                            state.clock = Clock::Wall {
                                position,
                                since: (!state.paused).then(Instant::now),
                            };
                            if state.paused {
                                state.frozen = position;
                            }
                            shared.wake.notify_all();
                        }
                        continue;
                    }
                }
                state = shared.wait(state);
            }
        };
        // The decoder may refill what was just taken.
        shared.wake.notify_all();

        let mut result = Ok(());
        if let Some(end) = expected_end
            && sound.pts > end + GAP
        {
            let frames = ((sound.pts - end).min(AUDIO_LIMIT) * i64::from(RATE) / SECOND) as usize;
            scratch.clear();
            scratch.resize(frames * FRAME_BYTES, 0);
            result = output.write(&scratch);
        }
        if result.is_ok() {
            result = if volume >= 1.0 {
                output.write(&sound.samples)
            } else {
                scale(&sound.samples, volume, &mut scratch);
                output.write(&scratch)
            };
        }

        let end = sound.pts + sound.duration;
        expected_end = Some(end);
        let mut state = shared.lock();
        match result {
            Ok(()) => {
                if state.generation == generation {
                    let delay = output.delay_frames() * SECOND / i64::from(RATE);
                    state.clock = Clock::Sound {
                        written_end: end,
                        delay: delay.max(0),
                        stamp: Some(Instant::now()),
                    };
                }
            }
            Err(reason) => {
                // The device went away mid-film. The pictures carry on
                // against the wall clock from where the sound stopped.
                let position = state.position();
                state.audio_dead = true;
                state.audio.clear();
                state.audio_queued = 0;
                state.frozen = position;
                state.clock = Clock::Wall {
                    position,
                    since: (!state.paused).then(Instant::now),
                };
                drop(state);
                shared.wake.notify_all();
                log::diagnostic(format_args!(
                    "Warning: {reason}; continuing without sound\r"
                ));
                return;
            }
        }
    }
}

/// `samples` at `volume` (0 to 1), into `out`.
fn scale(samples: &[u8], volume: f32, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(samples.len());
    // Fixed point: one multiply and a shift per sample, no rounding drift.
    let gain = (volume.clamp(0.0, 1.0) * 32_768.0).round() as i32;
    for sample in samples.as_chunks::<2>().0 {
        let scaled = (i32::from(i16::from_ne_bytes(*sample)) * gain) >> 15;
        out.extend_from_slice(&(scaled as i16).to_ne_bytes());
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use super::{Engine, scale};
    use crate::platform::portable::media::decoder::{Decoder, SECOND};

    fn corpus(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("corpus")
            .join("media")
            .join(name)
    }

    fn samples(values: &[i16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_ne_bytes()).collect()
    }

    #[test]
    fn volume_scales_samples_and_reaches_silence() {
        let input = samples(&[1000, -1000, i16::MAX, i16::MIN]);
        let mut out = Vec::new();
        scale(&input, 0.5, &mut out);
        assert_eq!(out, samples(&[500, -500, 16383, -16384]));
        scale(&input, 0.0, &mut out);
        assert_eq!(out, samples(&[0, 0, 0, 0]));
        scale(&input, 1.0, &mut out);
        assert_eq!(out, input);
    }

    /// Play a silent-clock session to its end, counting pictures.
    fn play_out(engine: &Engine, limit: Duration) -> usize {
        let started = Instant::now();
        let mut shown = 0;
        let mut current = None;
        loop {
            let poll = engine.poll_video(current.take());
            if let Some(frame) = poll.frame {
                shown += 1;
                current = Some(frame);
            }
            if poll.finished {
                return shown;
            }
            assert!(started.elapsed() < limit, "playback did not end");
            std::thread::sleep(poll.wait.min(Duration::from_millis(5)));
        }
    }

    #[test]
    fn a_silent_film_plays_in_real_time_and_ends() {
        let mut decoder = Decoder::open(&corpus("tone.mp4")).unwrap();
        decoder.disable_audio();
        let engine = Engine::start(decoder, None);
        assert!(!engine.has_sound());
        let started = Instant::now();
        let shown = play_out(&engine, Duration::from_secs(10));
        let took = started.elapsed();
        assert!(shown >= 5, "{shown} pictures shown");
        // The sample is one second long: it must take about that, not
        // finish as fast as it decodes and not hang.
        assert!(took >= Duration::from_millis(700), "ended after {took:?}");
        assert!(took <= Duration::from_secs(5), "ended after {took:?}");
        engine.finish().unwrap();
    }

    #[test]
    fn pause_holds_the_position_and_seek_moves_it() {
        let mut decoder = Decoder::open(&corpus("tone.mkv")).unwrap();
        decoder.disable_audio();
        let engine = Engine::start(decoder, None);
        assert!(engine.toggle_pause());
        let held = engine.position();
        std::thread::sleep(Duration::from_millis(150));
        let _ = engine.poll_video(None);
        assert_eq!(engine.position(), held);
        assert!(!engine.finished());

        // Seeking is clamped to the presentation.
        assert_eq!(engine.seek_by(-5 * SECOND), 0);
        let duration = engine.duration().expect("the sample states its length");
        assert_eq!(engine.seek_by(60 * SECOND), duration);
        assert_eq!(engine.position(), duration);

        // Back to the start and resumed, it plays to the end again.
        assert_eq!(engine.seek_by(-60 * SECOND), 0);
        assert!(!engine.toggle_pause());
        assert!(play_out(&engine, Duration::from_secs(10)) >= 5);
        engine.finish().unwrap();
    }

    #[test]
    fn volume_moves_in_steps_and_stays_in_range() {
        let mut decoder = Decoder::open(&corpus("tone.mp4")).unwrap();
        decoder.disable_audio();
        let engine = Engine::start(decoder, None);
        assert_eq!(engine.change_volume(1), 1.0);
        assert!((engine.change_volume(-3) - 0.7).abs() < 1e-6);
        assert_eq!(engine.change_volume(-20), 0.0);
        engine.finish().unwrap();
    }

    #[test]
    fn closing_mid_playback_stops_cleanly() {
        let mut decoder = Decoder::open(&corpus("tone.webm")).unwrap();
        decoder.disable_audio();
        let engine = Engine::start(decoder, None);
        let _ = engine.poll_video(None);
        engine.finish().unwrap();
    }
}
