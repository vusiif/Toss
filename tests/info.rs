//! Explicit inspection must never run the input's automatic action.

use std::path::Path;
use std::process::Command;

fn inspect(path: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_toss"))
        .arg("info")
        .arg(path)
        .output()
        .expect("toss runs")
}

#[test]
fn info_inspects_images_media_and_archives_without_opening_them() {
    // Invalid payloads would fail if routed to a decoder or extractor. Info
    // only needs metadata, including in a build with those handlers enabled.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    for path in [
        root.join("image/truncated.png"),
        root.join("media/truncated.mp4"),
        root.join("archive/corrupt/truncated.zip"),
    ] {
        let result = inspect(&path);
        assert!(result.status.success(), "{path:?}: {result:?}");
        assert!(result.stderr.is_empty(), "{result:?}");
        assert!(String::from_utf8_lossy(&result.stdout).contains("Size"));
    }
}

#[test]
fn info_inspects_directory_without_creating_an_archive() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let result = inspect(&root.join("src"));
    assert!(result.status.success(), "{result:?}");
    assert!(result.stderr.is_empty(), "{result:?}");
    assert!(String::from_utf8_lossy(&result.stdout).contains("directory"));
}

#[test]
fn info_preserves_missing_path_exit_code() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("no-such-info-input-for-toss");
    let result = inspect(&path);
    assert_eq!(result.status.code(), Some(4), "{result:?}");
}
