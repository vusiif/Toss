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
    // Two shapes are covered: names carried without the UTF-8 flag
    // (`utf8-paths.zip`) and names carried with it (`utf8-flag-paths.zip`,
    // Phase 9 — CJK, emoji, combining marks, a zero-width joiner and a
    // right-to-left override, all of which must arrive as recorded).
    for corpus in ["unicode/utf8-paths.zip", "unicode/utf8-flag-paths.zip"] {
        let scratch = Scratch::new("unicode");
        let archive = sample(scratch.path(), corpus);

        let output = toss(&[archive.as_path()]);
        assert!(
            output.status.success(),
            "{corpus} extraction failed: {}",
            stderr(&output)
        );

        let stem = archive.file_stem().expect("the sample has a stem");
        let destination = scratch.path().join(stem);
        let any_non_ascii = std::fs::read_dir(&destination)
            .unwrap_or_else(|err| panic!("{}: {err}", destination.display()))
            .flatten()
            .any(|entry| !entry.file_name().to_string_lossy().is_ascii());

        assert!(
            any_non_ascii,
            "{corpus}: expected at least one non-ASCII member on disk in {:?}",
            std::fs::read_dir(&destination)
                .map(|entries| entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect::<Vec<_>>())
                .unwrap_or_default()
        );
    }
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

/// The superscript corner of Microsoft's reserved-name list (see
/// <https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file>):
/// `COM¹.txt` and `LPT²` are device names because Windows treats ISO/IEC
/// 8859-1 superscript digits as digits in COM#/LPT# names. The refusal
/// must happen in the path policy *before* any open — this test proves
/// the member never reaches a syscall by showing only the control member
/// on disk. No real COM/LPT device is ever opened.
#[cfg(windows)]
#[test]
fn superscript_device_names_are_refused_before_any_open() {
    let scratch = Scratch::new("superscript");
    let archive = sample(scratch.path(), "security/windows-superscript-devices.zip");

    let output = toss(&[archive.as_path()]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a refused destination is a generic failure: {:?}: {}",
        output.status.code(),
        stderr(&output)
    );

    let message = stderr(&output);
    assert!(
        message.contains("would escape the output directory"),
        "the refusal must say what it refused, got: {message}"
    );
    assert!(
        message.contains("COM\u{00B9}"),
        "the refused member must be named with its superscript digit, got: {message}"
    );

    let destination = scratch.path().join("windows-superscript-devices");
    let mut names: Vec<String> = std::fs::read_dir(&destination)
        .unwrap_or_else(|err| panic!("{}: {err}", destination.display()))
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["ok.txt"],
        "only the control member may exist; nothing may reach a device"
    );
}

/// The same fixture where those names are ordinary files: extraction
/// succeeds and every member lands on disk as recorded — pinning that the
/// superscript gate never leaks onto Unix.
#[cfg(not(windows))]
#[test]
fn superscript_device_names_are_ordinary_files_on_unix() {
    let scratch = Scratch::new("superscript-unix");
    let archive = sample(scratch.path(), "security/windows-superscript-devices.zip");

    let output = toss(&[archive.as_path()]);
    assert!(
        output.status.success(),
        "extraction failed: {}",
        stderr(&output)
    );

    let destination = scratch.path().join("windows-superscript-devices");
    let mut names: Vec<String> = std::fs::read_dir(&destination)
        .unwrap_or_else(|err| panic!("{}: {err}", destination.display()))
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["COM\u{00B9}.txt", "LPT\u{00B2}", "ok.txt"],
        "every member must land as recorded on Unix"
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
fn a_declared_size_that_lies_is_corrupt_not_a_partial_success() {
    // The sample records 16 bytes where the stream really expands to 1088.
    // libarchive refuses the difference, and §24 keeps that as 6 — the file
    // is damaged, not merely awkward. (An *honest* huge size is a different
    // question — resource limits are §29 and are deliberately not enforced
    // by this check.)
    let scratch = Scratch::new("size-lie");
    let archive = sample(scratch.path(), "corrupt/size-lie.zip");

    let output = toss(&[archive.as_path()]);
    assert_eq!(
        output.status.code(),
        Some(6),
        "expected exit 6 for the lying size, got {:?}: {}",
        output.status.code(),
        stderr(&output)
    );
}

/// The regression for the Windows destination-semantics gate in
/// `backend::archive::policy`: before it, `NUL`/`NUL.txt`/`CON` wrote to
/// devices and reported success with no file on disk, `foo.txt:stream`
/// created a hidden alternate data stream, `bar.txt ` and `baz.txt.` were
/// silently renamed by the Win32 path parser, and only the trailing
/// backslash-traversal member ever produced an error.
#[cfg(windows)]
#[test]
fn windows_destination_tricks_are_refused_at_the_command_line() {
    let scratch = Scratch::new("windows-hostile");
    let archive = sample(scratch.path(), "security/hostile-windows-names.zip");

    let output = toss(&[archive.as_path()]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a refused destination is a generic failure: {:?}: {}",
        output.status.code(),
        stderr(&output)
    );

    let message = stderr(&output);
    assert!(
        message.contains("would escape the output directory"),
        "the refusal must say what it refused, got: {message}"
    );
    // The refused member must be named, so the message is actionable.
    assert!(
        message.contains("NUL"),
        "the first hostile member must be named, got: {message}"
    );

    // Only the control member landed: nothing was written to a device,
    // nothing was renamed, no stream was created.
    let destination = scratch.path().join("hostile-windows-names");
    let mut names: Vec<String> = std::fs::read_dir(&destination)
        .unwrap_or_else(|err| panic!("{}: {err}", destination.display()))
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["ok.txt"], "unexpected members: {names:?}");

    // And the scratch directory holds nothing but the sample and the output
    // root — the refusal happened before any syscall touched the disk.
    let mut beside: Vec<String> = std::fs::read_dir(scratch.path())
        .expect("scratch readable")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    beside.sort();
    assert_eq!(
        beside,
        ["hostile-windows-names", "hostile-windows-names.zip"],
        "something else was written beside the output root: {beside:?}"
    );
}

/// The same sample on Unix. Two things are platform facts, both observed:
/// `NUL`, `CON` and `foo.txt:stream` are ordinary file names there (no
/// devices, no streams — they must land on disk as recorded), and
/// libarchive hands a zip's backslash separators over as `/` on every
/// platform, so `..\..\evil.txt` arrives as `../../evil.txt` and the
/// shared `..` walk refuses it here just as on Windows. The members before
/// it were already written; nothing may appear outside the output root.
#[cfg(not(windows))]
#[test]
fn windows_only_hostile_names_stay_contained_on_unix() {
    let scratch = Scratch::new("unix-hostile");
    let archive = sample(scratch.path(), "security/hostile-windows-names.zip");

    let output = toss(&[archive.as_path()]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "the backslash traversal must be refused here too: {:?}: {}",
        output.status.code(),
        stderr(&output)
    );
    let message = stderr(&output);
    assert!(
        message.contains("would escape the output directory"),
        "the refusal must say what it refused, got: {message}"
    );

    // The seven members written before the refused one, under their exact
    // recorded names — device names included, because Unix has no devices
    // to write them to.
    let destination = scratch.path().join("hostile-windows-names");
    let mut names: Vec<String> = std::fs::read_dir(&destination)
        .unwrap_or_else(|err| panic!("{}: {err}", destination.display()))
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "CON",
            "NUL",
            "NUL.txt",
            "bar.txt ",
            "baz.txt.",
            "foo.txt:stream",
            "ok.txt",
        ],
        "unexpected members on disk"
    );

    // Containment: the archive and the output root, nothing else.
    let mut beside: Vec<String> = std::fs::read_dir(scratch.path())
        .expect("scratch readable")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    beside.sort();
    assert_eq!(
        beside,
        ["hostile-windows-names", "hostile-windows-names.zip"],
        "something else was written beside the output root: {beside:?}"
    );
}
