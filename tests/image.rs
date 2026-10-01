//! Image viewing, end to end — the numbers §24 tables, without a window.
//!
//! `src/platform/windows/image/decode.rs` proves WIC turns a sample into
//! pixels; this proves the *whole command line* answers a person's actual
//! input with the right exit code, because only the real command exercises
//! argument handling, detection, dispatch and the error translation
//! together (§33.2).
//!
//! **Nothing here may open a window.** The checks that need one are the
//! manual smoke runs (`IMAGE_VIEWER.md` §8); a test that opened a viewer
//! would hang the suite — which is exactly what happened once before
//! (STATUS.md §3.12). Every case below therefore fails *before* any window
//! exists: a missing path, a broken picture, a directory, or a platform
//! with no viewer to offer.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Run the real `toss` against `args`.
fn toss(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_toss"))
        .args(args)
        .output()
        .expect("toss runs")
}

/// The committed image samples, which are read-only input here: unlike
/// extraction, viewing never writes beside what it was given.
fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
        .join("image")
}

/// What a failure looked like, so an assertion can print the reason rather
/// than just the wrong number.
fn reported(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

#[test]
fn a_missing_image_reports_not_found() {
    // Reaching the viewer at all means the path resolved (§`input`), so the
    // one thing this can be is "not there" — exit 4, taken on every
    // platform, because nothing platform-specific has run yet.
    let output = toss(&[&corpus().join("no-such-picture-for-toss.png")]);

    assert_eq!(output.status.code(), Some(4), "got: {}", reported(&output));
    assert!(
        reported(&output).starts_with("Error: "),
        "errors go to stderr with the prefix §25 tables: {}",
        reported(&output)
    );
}

#[test]
fn a_directory_given_to_view_is_refused_as_not_an_image() {
    // The mistake a person can really make (`toss view Documents/`, §4.2).
    // The complaint names what they typed rather than the vaguer
    // "unsupported format", so §24's 2 and 3 stay distinguishable from a
    // script's side.
    let output = Command::new(env!("CARGO_BIN_EXE_toss"))
        .arg("view")
        .arg(corpus())
        .output()
        .expect("toss runs");

    assert_eq!(output.status.code(), Some(2), "got: {}", reported(&output));
    assert!(
        reported(&output).contains("is not an image file"),
        "expected the handler's own complaint: {}",
        reported(&output)
    );
}

#[test]
fn a_file_that_is_not_an_image_is_refused_before_any_decoder_sees_it() {
    // `toss view Cargo.toml`. Nothing is wrong with the manifest, so this
    // is §24's 3 — and it has to be decided from the classification (§6)
    // rather than from a decode attempt, which would report the same input
    // as a corrupt image (6) and tell a script to expect a broken file.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_toss"))
        .arg("view")
        .arg(&manifest)
        .output()
        .expect("toss runs");

    assert_eq!(output.status.code(), Some(3), "got: {}", reported(&output));
}

#[test]
fn a_broken_picture_reports_a_corrupt_input_rather_than_a_failure() {
    // The case M7 exists for. The extension says `png`, the format is
    // recognised, the bytes are not usable — §24's 6, which a script can
    // act on ("this file is broken") where 1 says only "something went
    // wrong".
    //
    // On a platform with no viewer, the same input reaches the capability
    // facade and stops one step earlier with §24's 3: the refusal that
    // `IMAGE_VIEWER.md` §8 requires to be defined — and never a panic.
    let output = toss(&[&corpus().join("truncated.png")]);

    let expected = if cfg!(all(windows, feature = "image")) {
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

#[cfg(not(all(windows, feature = "image")))]
#[test]
fn an_image_on_a_platform_without_a_viewer_is_defined_behaviour() {
    // `IMAGE_VIEWER.md` §8's portability guard, asserted against the real
    // command: a platform that has no viewer must still answer a picture
    // with exit 3 and a sentence, rather than panicking or pretending to
    // have shown something. Gated so that a build which *does* have a
    // viewer never runs it — that case would open a window and hang here.
    let output = toss(&[&corpus().join("odd.png")]);

    assert_eq!(output.status.code(), Some(3), "got: {}", reported(&output));
    assert!(
        reported(&output).contains("not implemented yet: image viewing"),
        "expected the capability facade's refusal: {}",
        reported(&output)
    );
}
