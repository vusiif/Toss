//! Exercise the public binary, not a duplicate batch coordinator.
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("third_party/toss-sheet/tests/fixtures")
        .join(name)
}
fn run(paths: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_toss"))
        .args(paths)
        .output()
        .unwrap()
}
#[cfg(feature = "spreadsheet")]
#[test]
fn automatic_batch_continues_after_bad_data_and_missing_paths() {
    let first = fixture("simple.csv");
    let bad = fixture("invalid-utf8.csv");
    let missing = fixture("not-a-real-batch-input.csv");
    let last = fixture("edge.tsv");
    let first_output = run(&[&first]);
    let last_output = run(&[&last]);
    let output = run(&[&first, &bad, &missing, &last]);
    assert_eq!(output.status.code(), Some(6));
    assert_eq!(
        output.stdout,
        [first_output.stdout, last_output.stdout].concat()
    );
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(
        errors.contains("2 succeeded, 2 failed, 0 skipped"),
        "{errors}"
    );
    assert!(errors.contains("invalid-utf8.csv") && errors.contains("not-a-real-batch-input.csv"));
    let output = run(&[&missing, &bad, &last]);
    assert_eq!(output.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&output.stdout).contains("中文"));
}
#[cfg(feature = "spreadsheet")]
#[test]
fn successful_batch_keeps_stdout_data_and_single_input_is_unchanged() {
    let file = fixture("simple.csv");
    let single = run(&[&file]);
    assert!(single.status.success());
    assert!(single.stderr.is_empty());
    let batch = run(&[&file, &file]);
    assert!(batch.status.success());
    assert_eq!(
        batch.stdout,
        [single.stdout.clone(), single.stdout].concat()
    );
    assert!(String::from_utf8_lossy(&batch.stderr).contains("2 succeeded, 0 failed, 0 skipped"));
}
#[cfg(feature = "spreadsheet")]
#[test]
fn explicit_modes_still_stop_at_resolution_or_handler_errors() {
    let good = fixture("simple.csv");
    let bad = fixture("invalid-utf8.csv");
    let last = fixture("edge.tsv");
    let missing = fixture("not-a-real-batch-input.csv");
    let explicit = Command::new(env!("CARGO_BIN_EXE_toss"))
        .arg("view")
        .args([&good, &bad, &last])
        .output()
        .unwrap();
    assert_eq!(explicit.status.code(), Some(6));
    assert_eq!(explicit.stdout, run(&[&good]).stdout);
    assert!(!String::from_utf8_lossy(&explicit.stderr).contains("batch complete"));
    let info = Command::new(env!("CARGO_BIN_EXE_toss"))
        .arg("info")
        .args([&good, &missing, &last])
        .output()
        .unwrap();
    assert_eq!(info.status.code(), Some(4));
    assert!(info.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&info.stderr).contains("batch complete"));
}
#[cfg(not(feature = "spreadsheet"))]
#[test]
fn unsupported_items_are_reported_as_skipped_and_do_not_stop_following_inputs() {
    let image = fixture("basic.xlsx");
    let other = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = run(&[&image, &other]);
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("1 succeeded, 0 failed, 1 skipped"));
}
