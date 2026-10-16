//! Archive extraction, end to end (§33.2).
//!
//! `src/backend/archive/libarchive/reader.rs` proves the backend round-trips a
//! member; this proves the whole promise §45 step 7 makes — that the shipped
//! command takes a path, chooses a destination, writes it, and reports the
//! result with the exit code §24 tables — because only the real command line
//! exercises argument handling, dispatch, naming and the status together.
//!
//! Samples are copied into a scratch directory first: extraction writes
//! beside the archive (§21), and `tests/corpus/` is not a place Toss may
//! leave output behind.
//!
//! The feature gate keeps the target empty in a build that compiled no
//! archive backend in. Tests live outside `src/`, so gating here does not put
//! a `cfg` in a module §12 wants kept free of them.

#![cfg(feature = "archive")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch directory that cleans itself up, so a failing test does not
/// leave extraction output behind for the next run to trip over.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("toss-extract-e2e-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run the real `toss` against `args`.
fn toss(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_toss"))
        .args(args)
        .output()
        .expect("toss runs")
}

/// Copy a committed sample into `scratch` and hand back where it landed, so
/// extraction has somewhere of its own to write beside.
fn sample(scratch: &Path, corpus: &str) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
        .join("archive")
        .join(corpus);

    let destination = scratch.join(source.file_name().expect("the sample has a name"));
    std::fs::copy(&source, &destination)
        .unwrap_or_else(|err| panic!("copying {}: {err}", source.display()));

    destination
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn an_archive_extracts_beside_itself_under_its_own_name() {
    // §21: `simple.zip` becomes `simple/`, next to the archive, not somewhere
    // Toss chose for the user.
    let scratch = Scratch::new("automatic");
    let archive = sample(scratch.path(), "valid/simple.zip");

    let output = toss(&[archive.as_path()]);
    assert!(
        output.status.success(),
        "extraction failed: {}",
        stderr(&output)
    );

    let destination = scratch.path().join("simple");
    assert!(
        destination.join("META-INF").join("MANIFEST.MF").is_file(),
        "a member of the archive was not written"
    );
    assert!(
        destination.join("tmp.class").is_file(),
        "a member is missing"
    );

    // §25: results belong on stdout and diagnostics stay off it, so a script
    // can read the two streams separately.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Extracted"),
        "expected a result line on stdout, got: {stdout}"
    );
    assert!(
        output.stderr.is_empty(),
        "stderr was not empty: {}",
        stderr(&output)
    );
}

#[test]
fn the_extract_verb_does_exactly_what_automatic_mode_does() {
    // §4.2: naming the verb states the intent, it does not change the result.
    let scratch = Scratch::new("verb");
    let archive = sample(scratch.path(), "valid/simple.zip");

    let output = toss(&[Path::new("extract"), archive.as_path()]);
    assert!(
        output.status.success(),
        "`toss extract` failed: {}",
        stderr(&output)
    );

    assert!(
        scratch.path().join("simple").join("tmp.class").is_file(),
        "the verb produced a different destination"
    );
}

#[test]
fn extracting_twice_leaves_the_first_result_untouched() {
    // §27: automatic mode never replaces what the user might already have,
    // including a folder an earlier run produced — and the refusal is a
    // failure, not a quiet success (§24).
    let scratch = Scratch::new("conflict");
    let archive = sample(scratch.path(), "valid/simple.zip");

    assert!(
        toss(&[archive.as_path()]).status.success(),
        "the first extraction must succeed"
    );
    let destination = scratch.path().join("simple");
    let before = std::fs::read(destination.join("tmp.class")).expect("the member exists");

    let refused = toss(&[archive.as_path()]);
    assert_eq!(
        refused.status.code(),
        Some(1),
        "a refused overwrite is a generic failure: {}",
        stderr(&refused)
    );
    let message = stderr(&refused);
    assert!(
        message.starts_with("Error: "),
        "diagnostics belong on stderr with a prefix (§25), got: {message}"
    );
    assert!(
        message.contains("output already exists"),
        "the message must name the conflict, got: {message}"
    );

    assert_eq!(
        std::fs::read(destination.join("tmp.class")).expect("still readable"),
        before,
        "the existing output was modified"
    );
}

#[test]
fn member_names_survive_unicode_end_to_end() {
    // §22 read the other way round: the unit tests prove `list` keeps a name
    // intact, this proves the bytes reach the filesystem the same way.
    let scratch = Scratch::new("unicode");
    let archive = sample(scratch.path(), "unicode/utf8-paths.zip");

    let output = toss(&[archive.as_path()]);
    assert!(
        output.status.success(),
        "extraction failed: {}",
        stderr(&output)
    );

    let destination = scratch.path().join("utf8-paths");
    let any_non_ascii = std::fs::read_dir(&destination)
        .unwrap_or_else(|err| panic!("{}: {err}", destination.display()))
        .flatten()
        .any(|entry| !entry.file_name().to_string_lossy().is_ascii());

    assert!(
        any_non_ascii,
        "expected at least one non-ASCII member on disk in {:?}",
        std::fs::read_dir(&destination)
            .map(|entries| entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>())
            .unwrap_or_default()
    );
}

#[test]
fn a_traversal_member_is_refused_and_nothing_escapes() {
    // §16/§28: Toss owns this policy rather than trusting the library, and
    // the promise is only worth what happens on the real command line.
    let scratch = Scratch::new("traversal");
    let archive = sample(scratch.path(), "security/traversal.zip");

    let output = toss(&[archive.as_path()]);
    assert!(
        !output.status.success(),
        "a member climbing out of the output root must not succeed"
    );

    let message = stderr(&output);
    assert!(
        message.contains("would escape the output directory"),
        "the refusal must say what it refused, got: {message}"
    );

    assert!(
        !scratch
            .path()
            .join("toss-corpus-traversal-should-not-exist")
            .exists(),
        "a member escaped the output directory"
    );
}

#[test]
fn a_corrupt_archive_reports_six_rather_than_one() {
    // §24's table: a recognised-but-broken input is exit 6, so a script can
    // tell a bad file from a generic failure. An unknown input is exit 3, and
    // the two must not collapse into each other (§42).
    let scratch = Scratch::new("corrupt");
    let archive = sample(scratch.path(), "corrupt/malformed.zip");

    let output = toss(&[archive.as_path()]);
    assert_eq!(
        output.status.code(),
        Some(6),
        "expected exit 6 for corrupt input, got {:?}: {}",
        output.status.code(),
        stderr(&output)
    );
    assert!(
        stderr(&output).starts_with("Error: "),
        "the failure must be a diagnostic on stderr"
    );
}

#[test]
fn an_archive_with_unicode_and_spaces_in_its_own_path_extracts_correctly() {
    let scratch = Scratch::new("unicode-archive-name");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
        .join("archive")
        .join("valid")
        .join("simple.zip");
    let unicode_archive_name = "包含 空格 与 中文 😊.zip";
    let destination = scratch.path().join(unicode_archive_name);
    std::fs::copy(&source, &destination).expect("archive copied");

    let output = toss(&[destination.as_path()]);
    assert!(
        output.status.success(),
        "extraction failed: {}",
        stderr(&output)
    );

    let output_dir = scratch.path().join("包含 空格 与 中文 😊");
    assert!(
        output_dir.is_dir(),
        "expected output directory beside archive with matching unicode name"
    );
    assert!(
        output_dir.join("tmp.class").is_file(),
        "extracted member missing from unicode destination folder"
    );
}

#[test]
fn unicode_rar5_members_survive_end_to_end() {
    let scratch = Scratch::new("unicode-rar5");
    let archive = sample(scratch.path(), "unicode/unicode-rar5.rar");

    let output = toss(&[archive.as_path()]);
    assert!(
        output.status.success(),
        "RAR5 extraction failed: {}",
        stderr(&output)
    );

    let destination = scratch.path().join("unicode-rar5");
    let any_non_ascii = std::fs::read_dir(&destination)
        .unwrap_or_else(|err| panic!("{}: {err}", destination.display()))
        .flatten()
        .any(|entry| !entry.file_name().to_string_lossy().is_ascii());

    assert!(
        any_non_ascii,
        "expected non-ASCII member in RAR5 extraction"
    );
}

#[test]
fn archive_members_with_spaces_survive_end_to_end() {
    let scratch = Scratch::new("spaces-in-members");
    let archive = sample(scratch.path(), "edge/spaces-in-names.zip");

    let output = toss(&[archive.as_path()]);
    assert!(
        output.status.success(),
        "extraction with spaces in member names failed: {}",
        stderr(&output)
    );

    let destination = scratch.path().join("spaces-in-names");
    let has_spaces = std::fs::read_dir(&destination)
        .unwrap_or_else(|err| panic!("{}: {err}", destination.display()))
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().contains(' '));

    assert!(
        has_spaces,
        "expected at least one member with spaces in its name"
    );
}

#[test]
fn empty_archive_extracts_to_an_empty_directory() {
    let scratch = Scratch::new("empty-archive");
    let archive = sample(scratch.path(), "edge/empty.zip");

    let output = toss(&[archive.as_path()]);
    assert!(
        output.status.success(),
        "empty archive extraction failed: {}",
        stderr(&output)
    );

    let destination = scratch.path().join("empty");
    assert!(
        destination.is_dir(),
        "expected extraction directory to exist even for an empty archive"
    );
}

#[test]
fn zero_byte_member_survives_end_to_end() {
    let scratch = Scratch::new("zero-byte-member");
    let archive = sample(scratch.path(), "edge/zero-byte-member.zip");

    let output = toss(&[archive.as_path()]);
    assert!(
        output.status.success(),
        "extraction with zero byte member failed: {}",
        stderr(&output)
    );

    let destination = scratch.path().join("zero-byte-member");
    let zero_byte = std::fs::read_dir(&destination)
        .unwrap_or_else(|err| panic!("{}: {err}", destination.display()))
        .flatten()
        .any(|entry| entry.metadata().map(|m| m.len() == 0).unwrap_or(false));

    assert!(zero_byte, "expected at least one zero-byte member on disk");
}
