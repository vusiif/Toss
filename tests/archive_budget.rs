#![cfg(feature = "archive")]
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
fn scratch() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(".sandbox-test/reliability-20261102/cli");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/archive")
        .join(name)
}
fn run(path: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_toss"))
        .arg("extract")
        .args(args)
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn no_stage(parent: &Path) {
    assert!(!std::fs::read_dir(parent).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".toss-extract-")
    }));
}
#[test]
fn normal_multiformat_extraction_commits_and_conflicts_never_replace_output() {
    for name in [
        "simple.zip",
        "simple.7z",
        "simple-rar4.rar",
        "simple-rar5.rar",
    ] {
        let root = scratch();
        let path = root.path().join(name);
        std::fs::copy(fixture(&format!("valid/{name}")), &path).unwrap();
        let result = run(&path, &[]);
        assert!(
            result.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let destination = path.with_extension("");
        assert!(destination.is_dir());
        std::fs::write(destination.join("user-marker"), b"keep").unwrap();
        assert!(!run(&path, &[]).status.success());
        assert_eq!(
            std::fs::read(destination.join("user-marker")).unwrap(),
            b"keep"
        );
        no_stage(root.path());
    }
}
#[test]
fn declared_caps_and_noninteractive_space_warning_create_no_output() {
    let root = scratch();
    let path = root.path().join("example.zip");
    std::fs::copy(fixture("valid/simple.zip"), &path).unwrap();
    for args in [
        vec!["--max-total-bytes", "1"],
        vec!["--max-file-bytes", "1"],
        vec!["--max-entries", "1"],
        vec!["--reserve-bytes", "18446744073709551615"],
    ] {
        let output = run(&path, &args);
        assert!(!output.status.success(), "{args:?}");
        assert!(!path.with_extension("").exists());
        no_stage(root.path());
    }
    let output = run(
        &path,
        &[
            "--reserve-bytes",
            "18446744073709551615",
            "--accept-space-risk",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    no_stage(root.path());
}
#[test]
fn simultaneous_extractors_commit_one_complete_directory() {
    let root = scratch();
    let path = root.path().join("race.zip");
    std::fs::copy(fixture("valid/simple.zip"), &path).unwrap();
    let start = || {
        Command::new(env!("CARGO_BIN_EXE_toss"))
            .arg("extract")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let one = start();
    let two = start();
    let results = [
        one.wait_with_output().unwrap(),
        two.wait_with_output().unwrap(),
    ];
    assert_eq!(results.iter().filter(|r| r.status.success()).count(), 1);
    assert!(path.with_extension("").is_dir());
    no_stage(root.path());
}
#[test]
fn corrupted_archives_never_expose_final_or_partial_directories() {
    let root = scratch();
    let path = root.path().join("broken.zip");
    let mut bytes = std::fs::read(fixture("valid/simple.zip")).unwrap();
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&path, bytes).unwrap();
    let output = run(&path, &[]);
    assert!(!output.status.success());
    assert!(!path.with_extension("").exists());
    no_stage(root.path());
}
