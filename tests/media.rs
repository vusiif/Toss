//! Media playback, end to end — the numbers §24 tables, exercised as real
//! processes (§33.2).
//!
//! Two halves, because the capability itself has two halves:
//!
//! - the failure paths, which answer on **every** platform and need no
//!   audio device — these run in ordinary `cargo test`, on CI included;
//! - the lifecycle itself — open a corpus file, play it to its end, tear
//!   down — which needs Windows, the `media` feature, *and* a machine with
//!   somewhere for sound to go, so it is marked `#[ignore]` and run on a
//!   real machine with `cargo test -- --ignored`. CI runners have no audio
//!   endpoint; a red build there would say more about the runner than about
//!   Toss.
//!
//! Nothing here may open a window: playback in P7-A is console-only, and a
//! test that waited on interactive input would hang the suite.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Run the real `toss` against `args`.
fn toss(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_toss"))
        .args(args)
        .output()
        .expect("toss runs")
}

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
        .join("media")
}

fn reported(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

#[test]
fn a_missing_media_file_reports_not_found() {
    // Before any platform code: path resolution refuses it (§24's 4), the
    // same on every build.
    let missing = corpus().join("no-such-clip-for-toss.mp4");
    let output = toss(&["play", &missing.to_string_lossy()]);

    assert_eq!(output.status.code(), Some(4), "got: {}", reported(&output));
}

#[test]
fn a_directory_given_to_play_is_refused_as_not_a_media_file() {
    // The handler's own precondition, before Media Foundation is reached —
    // so this holds on Linux and on a Windows build without the feature too.
    let output = toss(&["play", &corpus().to_string_lossy()]);

    assert_eq!(output.status.code(), Some(2), "got: {}", reported(&output));
    assert!(
        reported(&output).contains("is not a media file"),
        "expected the handler's own complaint: {}",
        reported(&output)
    );
}

#[test]
fn a_broken_container_reports_a_corrupt_input_rather_than_a_failure() {
    // The classification P7-A measured against real Media Foundation:
    // a container cut inside its header is §24's 6 where a player exists,
    // and §24's 3 where the capability itself does not — the same shape as
    // the image verdict in `tests/image.rs`.
    let broken = corpus().join("truncated.mp4");
    let output = toss(&[&broken.to_string_lossy()]);

    let expected = if cfg!(all(windows, feature = "media")) {
        6
    } else {
        3
    };
    assert_eq!(
        output.status.code(),
        Some(expected),
        "got: {}",
        reported(&output)
    );
}

#[test]
fn a_file_that_is_not_media_at_all_is_refused_by_the_play_verb() {
    // `toss play Cargo.toml` — the verb routes straight to the handler, and
    // what stops it is Media Foundation finding nothing it recognises in the
    // bytes (measured: exit 3). On a build with no player the capability
    // facade stops it one step earlier, with the same number and the same
    // refusal's intent: not this kind of file.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = toss(&["play", &manifest.to_string_lossy()]);

    // 3 on every build: the player says "not this kind of file", and a build
    // without the player says it one step earlier — same number either way.
    assert_eq!(output.status.code(), Some(3), "got: {}", reported(&output));
}

/// The lifecycle itself: open a real corpus file, play it to the end, tear
/// everything down — exit 0, in about as long as the file lasts.
///
/// Ignored rather than merely gated: it needs an audio endpoint, and the
/// honest way to say that is a test that says *run me where sound exists*.
/// Run with `cargo test -- --ignored` on a real machine; P7-A measured it
/// there at ~1.1 s for a 1.0 s file.
#[test]
#[ignore = "needs Windows, the media feature, and an audio endpoint"]
fn plays_a_tone_all_the_way_to_its_end() {
    #[cfg(not(all(windows, feature = "media")))]
    return;

    #[cfg(all(windows, feature = "media"))]
    {
        let tone = corpus().join("tone.wav");
        let output = toss(&[&tone.to_string_lossy()]);

        assert_eq!(
            output.status.code(),
            Some(0),
            "the lifecycle is supposed to run to MEEndOfPresentation: {}",
            reported(&output)
        );
    }
}
