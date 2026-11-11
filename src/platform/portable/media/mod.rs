//! The portable media player: what `toss film.mkv` does where there is no
//! system media framework to hand the file to.
//!
//! Linux has no decoder stack that every distribution ships, so the one
//! binary carries its own: FFmpeg, built from the vendored tree and linked
//! in (`decoder.rs`, behind `media_shim.c`). What it cannot carry is the
//! machine's screen and speaker, and those are found at run time:
//!
//! - **pictures** go to the same winit/softbuffer window as the image
//!   viewer (`window.rs`);
//! - **sound** goes to ALSA, loaded with `dlopen` (`alsa.rs`);
//! - a file with no pictures — or a film on a machine with no display —
//!   plays from the terminal with keyboard controls (`console.rs`).
//!
//! This file decides which of those a given file on a given machine gets,
//! says so in words when something is missing, and owns which number a
//! failure becomes. The file is opened and its streams checked *before*
//! anything is shown, so a damaged or foreign file is exit 6 or 3 on a
//! server exactly as on a desktop.

mod alsa;
mod console;
mod decoder;
mod engine;
mod window;

use std::path::Path;

use crate::core::error::TossError;
use crate::core::log;
use crate::platform::portable::display::{self, NoDisplay};

use decoder::{Decoder, Info, SECOND};
use engine::Engine;

/// One press of Left or Right, as on Windows.
const SEEK_STEP: i64 = 5 * SECOND;

/// Play `path` until it ends or the user stops it.
pub fn play(path: &Path) -> Result<(), TossError> {
    let mut decoder = Decoder::open(path)?;
    let info = decoder.info();

    // What this machine can offer the file, asked only for what the file
    // actually holds: a sound file never touches the display.
    let screen = match info.video {
        Some(_) => display::available().map_err(|NoDisplay(reason)| reason),
        None => Err(String::new()),
    };
    let speaker = if info.audio {
        alsa::Output::open()
    } else {
        Err(String::new())
    };

    match (info.video, screen, speaker) {
        // A film with somewhere to show it. Sound comes along if it can.
        (Some(picture), Ok(()), speaker) => {
            let sound = match speaker {
                Ok(output) => Sound::Playing(output),
                Err(reason) if info.audio => {
                    decoder.disable_audio();
                    Sound::Unavailable(reason)
                }
                Err(_) => Sound::Absent,
            };
            log::out(&summary(
                path,
                &info,
                Some(picture),
                &sound,
                &window::CONTROLS,
            ));
            let engine = Engine::start(decoder, sound.into_output());
            window::run(engine, title_of(path), picture)
        }
        // A film with no display but a speaker: its sound track.
        (Some(_), Err(reason), Ok(output)) => {
            decoder.disable_video();
            log::diagnostic(format_args!(
                "Note: no display ({reason}); playing the sound only"
            ));
            listen(decoder, output)
        }
        (Some(_), Err(screen), Err(speaker)) => Err(TossError::not_implemented(format!(
            "media playback here: no display ({screen}) and {}",
            if info.audio {
                speaker
            } else {
                "the file has no sound".to_owned()
            }
        ))),
        // A sound file.
        (None, _, Ok(output)) => {
            if info.skipped_video {
                log::diagnostic(
                    "Note: the video is in a format this build does not decode; playing the sound only",
                );
            }
            listen(decoder, output)
        }
        (None, _, Err(reason)) => Err(TossError::not_implemented(format!(
            "audio playback here: {reason}"
        ))),
    }
}

/// Play sound with keyboard controls in the terminal.
fn listen(decoder: Decoder, output: alsa::Output) -> Result<(), TossError> {
    let engine = Engine::start(decoder, Some(output));
    console::run(&engine);
    engine.finish()
}

/// What became of the file's sound.
enum Sound {
    Playing(alsa::Output),
    /// The file has sound and this machine cannot play it: why.
    Unavailable(String),
    /// The file has no sound this build decodes.
    Absent,
}

impl Sound {
    fn into_output(self) -> Option<alsa::Output> {
        match self {
            Sound::Playing(output) => Some(output),
            Sound::Unavailable(_) | Sound::Absent => None,
        }
    }
}

fn title_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// `1:02:03` or `2:03`, rounded down to the second.
fn clock(position: i64) -> String {
    let seconds = position.max(0) / SECOND;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// The console companion of the video window: what is playing, what is
/// missing, and the keys that exist — no others, the rule every Toss
/// viewer's summary follows.
fn summary(
    path: &Path,
    info: &Info,
    picture: Option<(u32, u32)>,
    sound: &Sound,
    controls: &[(&str, &str)],
) -> String {
    let mut text = String::from("Toss — Media Player\n\n");
    let mut field = |label: &str, value: String| {
        text.push_str(&format!("{label:<12}{value}\n"));
    };
    field("File", title_of(path));
    if let Some((width, height)) = picture {
        field("Video", format!("{width} × {height}"));
    }
    field(
        "Sound",
        match sound {
            Sound::Playing(_) => "yes".to_owned(),
            Sound::Unavailable(reason) => format!("silent — {reason}"),
            Sound::Absent if info.skipped_audio => {
                "silent — in a format this build does not decode".to_owned()
            }
            Sound::Absent => "none".to_owned(),
        },
    );
    if let Some(duration) = info.duration {
        field("Duration", clock(duration));
    }

    text.push_str("\nControls\n");
    let silent = !matches!(sound, Sound::Playing(_));
    for (key, action) in controls {
        // A key printed here is a promise: no volume key without sound.
        if silent && *action == "Volume" {
            continue;
        }
        text.push_str(&format!("  {key:<12}{action}\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::decoder::{Info, SECOND};
    use super::{Sound, clock, summary, window};

    const INFO: Info = Info {
        video: Some((1920, 1080)),
        audio: true,
        skipped_video: false,
        skipped_audio: false,
        duration: Some(3723 * SECOND),
    };

    #[test]
    fn durations_read_like_a_clock() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(SECOND - 1), "0:00");
        assert_eq!(clock(125 * SECOND), "2:05");
        assert_eq!(clock(3723 * SECOND), "1:02:03");
        assert_eq!(clock(-5), "0:00");
    }

    #[test]
    fn the_summary_says_why_a_film_is_silent_and_drops_the_volume_key() {
        let silent = summary(
            Path::new("/films/a film.mkv"),
            &INFO,
            INFO.video,
            &Sound::Unavailable("no sound device (No such file or directory)".to_owned()),
            &window::CONTROLS,
        );
        assert!(silent.starts_with("Toss — Media Player\n\n"));
        assert!(silent.contains("File        a film.mkv\n"));
        assert!(silent.contains("Video       1920 × 1080\n"));
        assert!(
            silent.contains("Sound       silent — no sound device (No such file or directory)\n")
        );
        assert!(silent.contains("Duration    1:02:03\n"));
        assert!(silent.contains("  Space       Play / pause\n"));
        assert!(!silent.contains("Volume"));
    }

    #[test]
    fn a_film_without_a_sound_track_says_none() {
        let info = Info {
            audio: false,
            duration: None,
            ..INFO
        };
        let text = summary(
            Path::new("x.mp4"),
            &info,
            info.video,
            &Sound::Absent,
            &window::CONTROLS,
        );
        assert!(text.contains("Sound       none\n"));
        assert!(!text.contains("Duration"));

        let undecodable = Info {
            audio: false,
            skipped_audio: true,
            ..INFO
        };
        let text = summary(
            Path::new("x.mp4"),
            &undecodable,
            undecodable.video,
            &Sound::Absent,
            &window::CONTROLS,
        );
        assert!(text.contains("silent — in a format this build does not decode"));
    }
}
