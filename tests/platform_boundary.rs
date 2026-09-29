//! The platform boundary, enforced by reading the sources.
//!
//! `IMAGE_VIEWER.md` §3 says a Windows type must not appear in the portable
//! layers. The compiler cannot enforce that: on Linux the platform module is
//! never compiled, so a leak would sit there uncompiled and unnoticed, and on
//! Windows it would compile perfectly. This test reads the files instead.
//!
//! Doc comments are stripped before scanning so that a rule can name the very
//! thing it forbids — `//! Handlers must not import WIC or Direct2D` is a
//! sentence about the rule, not a violation of it.

use std::path::{Path, PathBuf};

/// Layers that must stay free of platform specifics (`Toss_AGENTS.md` §12,
/// and §6 of the Phase 6 guide, plus `detection` for the same reason).
const PORTABLE_DIRS: [&str; 4] = ["src/core", "src/handlers", "src/dispatch", "src/detection"];

/// The capability facade. It may name the platform module — that is what a
/// facade is for — but it may not carry a platform *type*.
const PORTABLE_FILES: [&str; 1] = ["src/platform/image.rs"];

/// Anything from this list in code means a Windows type escaped.
const WINDOWS_TYPES: &[&str] = &[
    "HWND",
    "WPARAM",
    "LPARAM",
    "HRESULT",
    "HBITMAP",
    "HDC",
    "IWIC",
    "ID2D",
    "D2D1",
    "WICBitmap",
    "CreateWindowExW",
    "StretchDIBits",
];

/// References to the platform implementation itself, which the facade needs
/// and nothing below it does.
const WINDOWS_PATHS: &[&str] = &["windows::Win32", "platform::windows", "super::windows"];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Drop line comments and doc comments, which are allowed to quote forbidden
/// names because quoting them is how a rule says what it rules out.
///
/// Only `//` at the start of a line or preceded by a space is treated as a
/// comment, so a `//` inside a string literal — a URL, say — is left alone.
fn without_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            if line.trim_start().starts_with("//") {
                return "";
            }
            match line.find(" //") {
                Some(at) => &line[..at],
                None => line,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// First forbidden token found in `path`, if any.
fn violation(path: &Path, forbidden: &[&'static str]) -> Option<&'static str> {
    let source = std::fs::read_to_string(path).ok()?;
    let code = without_comments(&source);

    forbidden.iter().copied().find(|token| code.contains(token))
}

fn relative(path: &Path) -> String {
    path.strip_prefix(root())
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

#[test]
fn no_windows_type_or_path_reaches_the_portable_layers() {
    let mut files = Vec::new();
    for dir in PORTABLE_DIRS {
        rust_files(&root().join(dir), &mut files);
    }

    // Guards the sweep itself: if the directories were renamed and the walk
    // found nothing, every assertion below would pass by asserting nothing.
    assert!(
        files.len() >= 10,
        "expected the portable layers to be scanned, found {} files",
        files.len()
    );

    let forbidden: Vec<&'static str> = WINDOWS_TYPES
        .iter()
        .copied()
        .chain(WINDOWS_PATHS.iter().copied())
        .collect();

    let mut leaks: Vec<String> = files
        .iter()
        .filter_map(|path| {
            violation(path, &forbidden).map(|token| format!("{token} in {}", relative(path)))
        })
        .collect();
    leaks.sort();

    assert!(
        leaks.is_empty(),
        "a Windows type or path escaped into the portable layers:\n  {}",
        leaks.join("\n  ")
    );
}

#[test]
fn the_capability_facade_carries_no_platform_type() {
    for relative_path in PORTABLE_FILES {
        let path = root().join(relative_path);
        assert!(path.is_file(), "{} is missing", path.display());

        let found = violation(&path, WINDOWS_TYPES);

        assert!(
            found.is_none(),
            "{relative_path} must stay free of platform types; found {found:?}"
        );
    }
}

/// A rule that has never fired has never been tested either. The whole point
/// of stripping comments is that the documentation of a rule may quote what it
/// forbids — so both halves of that claim get checked here, on purpose.
#[test]
fn the_scanner_ignores_comments_and_still_finds_code() {
    let probe = std::env::temp_dir().join(format!("toss-boundary-{}.rs", std::process::id()));

    let write = |source: &str| {
        std::fs::write(&probe, source).expect("probe written");
    };

    write("//! Handlers must not use HWND or IWIC.\nfn fine() {}\n");
    assert_eq!(
        violation(&probe, WINDOWS_TYPES),
        None,
        "a doc comment naming the forbidden type must not trip the rule"
    );

    write("fn fine() {} // mentions HRESULT in passing\n");
    assert_eq!(
        violation(&probe, WINDOWS_TYPES),
        None,
        "a trailing comment must not trip the rule"
    );

    write("fn bad(handle: HWND) {}\n");
    assert_eq!(
        violation(&probe, WINDOWS_TYPES),
        Some("HWND"),
        "the same type in code has to be caught"
    );

    let _ = std::fs::remove_file(&probe);
}
