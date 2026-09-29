//! Directory compression, end to end (§33.2, §30).
//!
//! These run the shipped binary rather than the library, because the promise
//! §18.2 makes is about `toss folder/`: the output name, the exit code, the
//! stdout/stderr split and the archive that appears on disk all matter, and
//! only the real command line exercises all four at once.
//!
//! The feature gate keeps the target empty in a build that compiled no
//! archive backend in — such a build answers "directory compression" instead,
//! which `src/dispatch` already covers. Tests live outside `src/`, so gating
//! here does not put a `cfg` in a module §12 wants kept free of them.

#![cfg(feature = "archive")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch directory that cleans itself up, so a failing test does not
/// leave archives behind for the next run to trip over.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("toss-pack-e2e-{}-{name}", std::process::id()));
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

fn assert_succeeded(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed with {}: stdout: {} | stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().expect("a file has a parent")).expect("parent directory");
    std::fs::write(path, bytes).expect("file written");
}

/// Every path under `root`, relative to it, paired with its contents — `None`
/// marks a directory, so an empty one is compared too rather than vanishing
/// from the picture (§30).
fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn collect(root: &Path, directory: &Path, out: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let entries = std::fs::read_dir(directory)
            .unwrap_or_else(|err| panic!("{}: {err}", directory.display()))
            .flatten();

        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap_or_else(|err| panic!("{}: {err}", path.display()))
                .to_path_buf();

            if path.is_dir() {
                out.insert(relative, None);
                collect(root, &path, out);
            } else {
                let contents =
                    std::fs::read(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
                out.insert(relative, Some(contents));
            }
        }
    }

    let mut out = BTreeMap::new();
    collect(root, root, &mut out);
    out
}

/// The tree §30 asks for: nested folders, an empty folder, an empty file,
/// spaces, and characters no ASCII-only path could express (§22).
fn source_tree() -> &'static [(&'static str, &'static [u8])] {
    &[
        ("readme.txt", b"hello world\n"),
        ("space in name.txt", b"spaces are ordinary characters\n"),
        ("资料 😊.txt", b"unicode survives the round trip\n"),
        ("nested/deep/inner.bin", &[0_u8, 1, 2, 3, 255]),
        ("nested/empty file.txt", b""),
    ]
}

fn build_source(root: &Path) {
    for (relative, bytes) in source_tree() {
        write(&root.join(relative), bytes);
    }

    // A folder with nothing in it: a round trip that dropped it would still
    // match on files alone, so it is part of the picture on purpose (§30).
    std::fs::create_dir_all(root.join("empty folder")).expect("empty folder created");
}

/// Extract `archive` back into the name it took and compare the result with
/// `source`, which has to move aside first: the extraction target is the
/// archive's own name, and that is the name the source still holds.
fn verify_round_trip(source: &Path, archive: &Path) {
    assert!(
        archive.is_file(),
        "the archive was not written where §26 says: {}",
        archive.display()
    );

    let parent = source.parent().expect("a directory has a parent");
    let original = parent.join("original");
    std::fs::rename(source, &original).expect("the source is moved aside");

    assert_succeeded(&toss(&[archive]), "extracting the archive");

    let expected = tree(&original);
    let actual = tree(source);

    assert!(!expected.is_empty(), "the fixture produced an empty tree");
    assert_eq!(
        actual,
        expected,
        "the round trip changed the tree:\n--- expected ---\n{}\n--- actual ---\n{}",
        render(&expected),
        render(&actual)
    );
}

fn render(tree: &BTreeMap<PathBuf, Option<Vec<u8>>>) -> String {
    tree.iter()
        .map(|(path, contents)| match contents {
            Some(bytes) => format!("{} ({} bytes)", path.display(), bytes.len()),
            None => format!("{}/", path.display()),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_directory_survives_a_round_trip_through_a_7z() {
    let scratch = Scratch::new("round-trip");
    let source = scratch.path().join("资料 folder");
    build_source(&source);

    assert_succeeded(&toss(&[source.as_path()]), "packing the directory");
    verify_round_trip(&source, &scratch.path().join("资料 folder.7z"));
}

#[test]
fn the_pack_verb_packs_the_same_way_automatic_mode_does() {
    // §4.2: naming the verb states the intent, it does not change the result.
    let scratch = Scratch::new("verb");
    let source = scratch.path().join("folder");
    build_source(&source);

    assert_succeeded(
        &toss(&[Path::new("pack"), source.as_path()]),
        "toss pack folder",
    );
    verify_round_trip(&source, &scratch.path().join("folder.7z"));
}

#[test]
fn packing_twice_leaves_the_first_archive_alone() {
    // §27: the conflict is refused rather than resolved by guessing, and the
    // archive the user already has must come back byte for byte unchanged.
    let scratch = Scratch::new("conflict");
    let source = scratch.path().join("folder");
    std::fs::create_dir_all(&source).expect("source directory");
    write(&source.join("hello.txt"), b"contents\n");

    assert_succeeded(&toss(&[source.as_path()]), "the first pack");

    let archive = scratch.path().join("folder.7z");
    let first = std::fs::read(&archive).expect("the first archive exists");

    let refused = toss(&[source.as_path()]);
    assert!(
        !refused.status.success(),
        "the second pack must not succeed silently"
    );
    assert_eq!(
        refused.status.code(),
        Some(1),
        "a refused overwrite is a generic failure (§24)"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.starts_with("Error: "),
        "diagnostics belong on stderr with a prefix (§25), got: {stderr}"
    );
    assert!(
        stderr.contains("output already exists"),
        "the message must name the conflict, got: {stderr}"
    );

    assert_eq!(
        std::fs::read(&archive).expect("still readable"),
        first,
        "the existing archive was modified"
    );
}
