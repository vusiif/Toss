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

            Component::Normal(part) => resolved.push(part),
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
}
