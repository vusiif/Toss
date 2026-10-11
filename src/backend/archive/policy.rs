//! Where an archive member is allowed to land.
//!
//! §16: using libarchive does not transfer security responsibility away from
//! Toss. The names inside an archive are attacker-controlled input, so this
//! module decides the destination and the backend merely writes to what it is
//! given. Nothing here reads libarchive's headers — it only sees a path.
//!
//! The rules are the ones §28 lists: reject absolute paths, drive and UNC
//! prefixes, `..` that climbs out of the output root, and anything that after
//! normalization no longer sits under it.
//!
//! On Windows there is a second half to "stays under it": the Win32 path
//! parser interprets a name *before* the filesystem ever sees it, so a
//! component that reads as inside the root can still be resolved to
//! somewhere else entirely — a device (`NUL`), an alternate data stream
//! (`foo.txt:stream`), or a different file name after trailing spaces and
//! dots are trimmed (`bar.txt ` → `bar.txt`). Those are refused here for
//! the same reason `..` is: the destination policy checked is not the
//! destination the operating system would write to (§28).

use std::path::{Component, Path, PathBuf};

use super::types::ArchiveError;

/// Resolve an archive member's recorded path against `root`.
///
/// Returns the destination to write to, or [`ArchiveError::UnsafePath`] if
/// the member is trying to leave `root`.
///
/// Lexical only: it walks `Component`s rather than touching the filesystem,
/// so it cannot be fooled by a symlink that already exists — that second
/// half of §17 is enforced by never creating links in the first place.
pub fn destination(root: &Path, entry: &Path) -> Result<PathBuf, ArchiveError> {
    let rejected = || ArchiveError::UnsafePath(entry.to_path_buf());

    if entry.as_os_str().is_empty() {
        return Err(rejected());
    }

    // Checked before walking: `Path::components` reports these as their own
    // variants, but an early return documents the intent and covers the
    // `\\?\`-style prefixes that arrive as `Prefix` on Windows.
    if entry.is_absolute() {
        return Err(rejected());
    }

    let root_depth = root.components().count();
    let mut resolved = root.to_path_buf();

    for component in entry.components() {
        match component {
            // Drive letters, UNC shares and a leading separator all mean the
            // entry named an absolute location (§28).
            Component::Prefix(_) | Component::RootDir => return Err(rejected()),

            // `components()` has already collapsed `.` and duplicate
            // separators, so there is nothing to do.
            Component::CurDir => {}

            Component::ParentDir => {
                // Never pop the root itself: `a/../../b` has already left.
                if resolved.components().count() <= root_depth {
                    return Err(rejected());
                }
                resolved.pop();
            }

            Component::Normal(part) => {
                if unsafe_on_windows(part) {
                    return Err(rejected());
                }
                resolved.push(part);
            }
        }
    }

    // Defense in depth: if the walk somehow produced something outside the
    // root — a rooted path on another volume, a UNC that read as `Normal` —
    // this catches it even though the loop above should not have allowed it.
    if !resolved.starts_with(root) {
        return Err(rejected());
    }

    Ok(resolved)
}

/// Windows destination rules for one path component (see the module docs).
///
/// Three classes, each observed to misbehave on a real Windows machine
/// before this check existed:
///
/// 1. **`:` — alternate data streams and device qualifiers.** `foo.txt:stream`
///    does not create a member named `foo.txt:stream`: Win32 writes a hidden
///    stream inside `foo.txt` (creating that file if needed), so the bytes
///    land somewhere the user cannot see in a directory listing.
/// 2. **Trailing spaces and dots — silent renaming.** The Win32 path parser
///    trims them before the file is created, so the member arrives as
///    `bar.txt` when the archive said `bar.txt `. That is a different file
///    than the policy validated, and it collides with any real `bar.txt`.
/// 3. **Reserved device names.** `NUL`, `CON`, `NUL.txt` and friends open
///    devices instead of files: the write "succeeds", nothing appears in the
///    output directory, and the extraction reports an entry that does not
///    exist. The match follows the rule the Windows parser uses — the part
///    before the first dot, trailing spaces removed, case-insensitive.
///
/// Every check is about the *component*, so an ordinary middle directory
/// (`sub./file`) is caught at `sub.` rather than after it has already been
/// rewritten.
///
/// Non-Windows targets answer `false` unconditionally: none of these
/// readings exist there — `NUL` and `foo:bar` are ordinary file names and
/// a trailing dot is part of the name (§22, and the tests in this module
/// pin both platforms).
fn unsafe_on_windows(part: &std::ffi::OsStr) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        // Wide units, so an interior NUL or an unpaired surrogate cannot
        // hide a `:` from a `to_str()` that would fail on the whole name.
        let wide: Vec<u16> = part.encode_wide().collect();

        // Class 1: any colon is the stream/device separator.
        if wide.iter().any(|&unit| unit == u16::from(b':')) {
            return true;
        }

        // Class 2: the parser trims exactly these before resolving.
        if matches!(
            wide.last(),
            Some(&last) if last == u16::from(b' ') || last == u16::from(b'.')
        ) {
            return true;
        }

        // Class 3: device name = the part before the first dot, with
        // trailing spaces removed (that is how `NUL .txt` still resolves
        // to the NUL device), matched case-insensitively — including the
        // official superscript forms `COM¹–³` / `LPT¹–³`, per
        // `is_reserved_device_name` below (Microsoft's published list).
        let base_end = wide
            .iter()
            .position(|&unit| unit == u16::from(b'.'))
            .unwrap_or(wide.len());
        let mut base = &wide[..base_end];
        while matches!(base.last(), Some(&last) if last == u16::from(b' ')) {
            base = &base[..base.len() - 1];
        }

        is_reserved_device_name(base)
    }

    #[cfg(not(windows))]
    {
        let _ = part;
        false
    }
}

/// Whether `base` (already cut at the first dot and trailing spaces — the
/// caller did both) names a Windows device.
///
/// The reserved list is the one Microsoft publishes in *Naming Files,
/// Paths, and Namespaces*:
/// <https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file>
///
/// ```text
/// CON, PRN, AUX, NUL, COM1–COM9, LPT1–LPT9, and the ISO/IEC 8859-1
/// superscript forms COM¹ COM² COM³ and LPT¹ LPT² LPT³;
/// also these names followed immediately by an extension
/// ("NUL.txt and NUL.tar.gz are both equivalent to NUL").
/// ```
///
/// The superscripts are real: the same document notes that Windows treats
/// `¹` `²` `³` (U+00B9, U+00B2, U+00B3) as digits in COM#/LPT# names, so
/// `COM¹` is reserved in every directory and creating it fails. That is
/// exactly the silent-loss class this gate exists for — an archive member
/// named `COM¹.txt` must be refused, not written to a device.
///
/// Deliberately *not* refused, so ordinary archives keep working: `COM0`
/// and `COM10` (not in the published list), names longer than one digit
/// (`COM¹²`), and ordinary Unicode names (`café`, `中文`) — those are
/// files, not devices.
///
/// Takes wide units because that is what the caller holds; the ASCII
/// prefix comparison is case-insensitive because Windows device names are.
#[cfg(windows)]
fn is_reserved_device_name(base: &[u16]) -> bool {
    if base.is_empty() {
        return false;
    }

    // Superscript forms: three ASCII letters (any case) plus exactly one
    // ISO/IEC 8859-1 superscript digit — `COM¹`, `com³`, `LPT²`, …
    // A non-ASCII letter before the digit (`éOM¹`) is simply not a
    // COM#/LPT# name, so it skips this branch and lands in the ASCII
    // check below, which cannot refuse a non-ASCII name either.
    const SUPERSCRIPT_DIGITS: [u16; 3] = [0x00B9, 0x00B2, 0x00B3];
    if base.len() == 4
        && SUPERSCRIPT_DIGITS.contains(&base[3])
        && base[..3].iter().all(|&unit| unit < 128)
    {
        let mut prefix = [0_u8; 3];
        for (slot, &unit) in prefix.iter_mut().zip(&base[..3]) {
            *slot = (unit as u8).to_ascii_uppercase();
        }
        if matches!(&prefix, b"COM" | b"LPT") {
            return true;
        }
        // `café¹`-shaped names fall through; neither branch refuses them.
    }

    // The ASCII half of the published list: bare names and COM1–COM9 /
    // LPT1–LPT9. A name containing anything else cannot match it.
    if !base.iter().all(|&unit| unit < 128) {
        return false;
    }
    let name: String = base
        .iter()
        .map(|&unit| (unit as u8 as char).to_ascii_uppercase())
        .collect();

    if matches!(
        name.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }

    name.len() == 4
        && (name.starts_with("COM") || name.starts_with("LPT"))
        && matches!(name.as_bytes()[3], b'1'..=b'9')
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{ArchiveError, destination};

    const ROOT: &str = "/output/root";

    fn resolve(entry: &str) -> Result<PathBuf, ArchiveError> {
        destination(Path::new(ROOT), Path::new(entry))
    }

    fn rejected(entry: &str) {
        match resolve(entry) {
            Err(ArchiveError::UnsafePath(path)) => {
                assert_eq!(
                    path,
                    PathBuf::from(entry),
                    "wrong entry reported for {entry:?}"
                );
            }
            other => panic!("{entry:?} should have been rejected, got {other:?}"),
        }
    }

    #[test]
    fn ordinary_members_land_under_the_root() {
        for entry in [
            "a.txt",
            "dir/a.txt",
            "./a.txt",
            "a/./b/c.txt",
            "a/../b.txt",
            // Normalizes to the root itself. It cannot escape, so accepting
            // it is correct; writing it as a file then fails on its own
            // merits because the root is a directory.
            "a/..",
        ] {
            let path = resolve(entry).unwrap_or_else(|err| panic!("{entry:?}: {err}"));
            assert!(
                path.starts_with(ROOT),
                "{entry:?} produced {path:?} outside the root"
            );
        }
    }

    #[test]
    fn climbing_out_of_the_root_is_refused() {
        // The cases §28 names, plus the ones that hide a climb mid-path.
        // `"a/.."` is deliberately absent: it comes back to the root rather
        // than leaving it, and a policy that rejected harmless paths as well
        // as hostile ones would only teach callers to ignore it.
        for entry in [
            "../evil",
            "../../evil",
            "a/../../evil",
            "a/b/../../../evil",
            "..",
        ] {
            rejected(entry);
        }
    }

    #[test]
    fn absolute_paths_are_refused_on_sight() {
        // A leading separator is absolute on both platforms, and a UNC-style
        // `//server/share` reads as rooted everywhere.
        for entry in ["/etc/passwd", "//server/share/x"] {
            rejected(entry);
        }
    }

    /// A drive letter is only a drive letter where drives exist.
    ///
    /// Split rather than asserted unconditionally, because the two platforms
    /// genuinely disagree and both are right: on Windows `C:/Windows/x` is
    /// rooted and must be refused, while on Unix `C:` is an ordinary
    /// filename character and the path is merely relative — rejecting it
    /// there would refuse something that cannot escape (§22, §28).
    #[cfg(windows)]
    #[test]
    fn a_drive_prefixed_path_is_refused() {
        rejected("C:/Windows/x");
    }

    #[cfg(not(windows))]
    #[test]
    fn a_drive_prefixed_path_is_a_relative_name_and_stays_inside() {
        let path = resolve("C:/Windows/x").expect("relative on unix");
        assert!(
            path.starts_with(ROOT),
            "it must still land under the root, got {path:?}"
        );
    }

    #[test]
    fn an_empty_member_is_refused() {
        rejected("");
    }

    #[test]
    fn a_traversal_is_rejected_before_anything_can_be_written() {
        // The order matters: validation happens first, so a hostile archive
        // never reaches a syscall (§16).
        assert!(resolve("../../Windows/System32/evil.dll").is_err());
    }

    /// Windows reads a member name through the Win32 path parser before the
    /// filesystem sees it, and each of these readings wrote somewhere the
    /// policy never validated: a device, a hidden stream, or a different
    /// file name than the archive recorded.
    #[cfg(windows)]
    #[test]
    fn windows_destination_tricks_are_refused_on_sight() {
        // Reserved device names, in the forms the parser really accepts
        // them: bare, with an extension, with a space before the dot, and
        // case-insensitively.
        for entry in [
            "NUL",
            "nul.txt",
            "NUL.tar.gz",
            "NUL .txt",
            "CON",
            "aux",
            "com1.log",
            "LPT9",
            "dir/PRN",
            "CONIN$",
        ] {
            rejected(entry);
        }

        // The superscript forms from the same published list — ISO/IEC
        // 8859-1 ¹ ² ³ (U+00B9/00B2/00B3) are digits in COM#/LPT# names,
        // so `COM¹`, `LPT².tar.gz` and case variants are devices too.
        // (Rejected lexically here: no test ever opens a real device.)
        for entry in ["COM¹", "com¹", "LPT²", "LPT³", "com³.txt", "LPT¹.tar.gz"] {
            rejected(entry);
        }

        // Alternate data streams: the bytes would land inside another file,
        // not in a member of this name.
        for entry in ["foo.txt:stream", "a:b", ":stream"] {
            rejected(entry);
        }

        // Trailing dots and spaces are trimmed by the parser, so the file
        // would be created under a name different from the one validated.
        for entry in ["bar.txt ", "baz.txt.", "sub.", "dir /file.txt"] {
            rejected(entry);
        }

        // Not reserved, so these must stay extractable rather than being
        // rejected by an over-eager rule: only COM1–COM9 / LPT1–LPT9
        // (and their superscripts) are on the list, and ordinary Unicode
        // names are ordinary files.
        for entry in [
            "COM0",
            "COM10",
            "LPT0",
            "NULL",
            "file",
            "café",
            "中文.txt",
            "comm¹.txt",
        ] {
            assert!(
                resolve(entry).is_ok(),
                "{entry:?} is an ordinary name and must be accepted"
            );
        }
    }

    /// The same names on Unix: `NUL`, `foo:bar`, a trailing dot and even
    /// `COM¹` are ordinary file names there, and refusing them would
    /// refuse work that cannot escape (§22 — the two platforms disagree
    /// and both are right; the superscript gate is `cfg(windows)` only).
    #[cfg(not(windows))]
    #[test]
    fn device_streams_and_trailing_dots_stay_ordinary_names_on_unix() {
        for entry in [
            "NUL",
            "nul.txt",
            "foo.txt:stream",
            "bar.txt ",
            "baz.txt.",
            "dir/aux",
            "COM¹",
            "LPT².tar.gz",
        ] {
            let path = resolve(entry).unwrap_or_else(|err| panic!("{entry:?}: {err}"));
            assert!(
                path.starts_with(ROOT),
                "{entry:?} produced {path:?} outside the root"
            );
        }
    }

    /// A Windows-only component must not become a Unix rule by accident:
    /// the shared traversal, absolute and drive checks above already run on
    /// both platforms, and this pins that the platform-specific gate is
    /// exactly one branch wide.
    #[cfg(not(windows))]
    #[test]
    fn unix_rejection_is_still_exactly_the_shared_rules() {
        for entry in ["../evil", "/etc/passwd", "//server/share/x", "", ".."] {
            rejected(entry);
        }
    }
}
