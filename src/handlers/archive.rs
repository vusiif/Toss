//! What Toss does with an input it classified as an archive.
//!
//! §9's chain lands here: dispatch decides *this is an archive*, this module
//! decides *where it goes* and *what to ask for*, and the router decides
//! *which compiled backend* does it (§1, §7). Nothing below this file knows
//! how an archive is decoded — that is the backend's business (§11).

use std::path::{Path, PathBuf};

use crate::backend::archive::{self, ArchiveError, ArchiveFormat, ArchiveInput, ExtractRequest};
use crate::core::error::TossError;
use crate::core::log;

/// Extract `archive` beside itself: `foo.7z` becomes `foo/` (§21).
pub fn extract(archive: &Path) -> Result<(), TossError> {
    let destination = output_root(archive);

    // §27: automatic mode never replaces what the user might already have.
    if destination.exists() {
        return Err(TossError::OutputConflict(destination));
    }

    // Created up front so the promised directory exists even for an archive
    // with no members. An empty directory left behind by a later failure is
    // untidy but harmless; refusing to start would be worse.
    std::fs::create_dir_all(&destination).map_err(|err| match err.kind() {
        std::io::ErrorKind::PermissionDenied => TossError::PermissionDenied(destination.clone()),
        _ => TossError::other(format!("cannot create {}: {err}", destination.display())),
    })?;

    let request = ExtractRequest {
        input: ArchiveInput::single(archive.to_path_buf()),
        output_root: destination.clone(),
    };

    // The router is built here rather than in dispatch so that the handler
    // remains the only thing that knows an archive is being extracted (§9),
    // and so the single `cfg(feature = "archive")` stays in `backend/` (§12).
    let router = archive::router();
    let backend = router
        .select(ArchiveFormat::Unknown, false)
        .ok_or_else(|| TossError::not_implemented("archive extraction"))?;

    let result = backend
        .extract(&request)
        .map_err(|err| translate(err, archive))?;

    log::out(&format!(
        "Extracted {} entries to {}",
        result.entries,
        destination.display()
    ));

    // §17: reported rather than omitted, so a user who expected a symlink
    // learns that Toss chose not to create it.
    if result.skipped > 0 {
        log::out(&format!(
            "Skipped {} entries: links, devices and special files are not restored",
            result.skipped
        ));
    }

    Ok(())
}

/// `foo.7z` to `foo`, next to the archive (§21).
///
/// A single strip is enough for v0.1: its three formats are single-extension,
/// and §40 leaves compound names such as `.tar.gz` for later rather than
/// committing to a rule the detector cannot yet honour.
///
/// A file with no extension at all — recognised by its contents rather than
/// by a suffix — would otherwise strip down to its own name and collide with
/// the input, so it gets a deterministic suffix instead of a failure.
fn output_root(archive: &Path) -> PathBuf {
    let Some(file_name) = archive.file_name() else {
        return archive.to_path_buf();
    };

    let stripped = match archive.file_stem() {
        Some(stem) => archive.with_file_name(stem),
        None => archive.to_path_buf(),
    };

    if stripped != archive {
        return stripped;
    }

    archive.with_file_name(format!("{}-extracted", file_name.to_string_lossy()))
}

/// Turn a backend failure into something Toss can report (§23).
///
/// The archive's path is added here because this layer is the one that knows
/// which file was being worked on; a backend deep inside extraction only
/// knows what went wrong.
fn translate(err: ArchiveError, archive: &Path) -> TossError {
    match err {
        ArchiveError::UnsupportedFormat => TossError::UnsupportedFormat(archive.to_path_buf()),
        ArchiveError::CorruptArchive(context) => TossError::CorruptInput {
            path: archive.to_path_buf(),
            context,
        },
        ArchiveError::OutputConflict(path) => TossError::OutputConflict(path),
        ArchiveError::PermissionDenied(path) => TossError::PermissionDenied(path),
        ArchiveError::UnsafePath(entry) => TossError::other(format!(
            "archive entry would escape the output directory: {}",
            entry.display()
        )),
        ArchiveError::BackendFailure(context) => {
            TossError::other(format!("archive backend failed: {context}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::output_root;

    #[test]
    fn the_output_directory_sits_beside_the_archive_with_its_name() {
        // §21's rule, stated as a test because §21 asks for exactly that.
        //
        // Forward slashes only: a backslash is a separator on Windows and an
        // ordinary character on Unix, so a Windows-shaped path here would
        // assert one platform's rules on the other (§22) — which is exactly
        // how this test failed on Linux the first time.
        let cases = [
            ("/home/x/archive.zip", "archive"),
            ("./relative/movie.rar", "movie"),
        ];

        for (input, expected) in cases {
            let destination = output_root(Path::new(input));
            assert_eq!(
                destination.file_name().and_then(|name| name.to_str()),
                Some(expected),
                "wrong destination for {input}"
            );
        }
    }

    /// Split out because the path only means a directory on Windows.
    #[cfg(windows)]
    #[test]
    fn a_windows_path_keeps_its_directory_and_strips_the_suffix() {
        let destination = output_root(Path::new(r"D:\Downloads\foo.7z"));

        assert_eq!(
            destination.file_name().and_then(|name| name.to_str()),
            Some("foo")
        );
        assert_eq!(
            destination.parent().and_then(|parent| parent.to_str()),
            Some(r"D:\Downloads")
        );
    }

    #[test]
    fn a_file_with_no_extension_does_not_collide_with_itself() {
        // Recognised by content rather than suffix, so stripping yields the
        // input's own name — which would make the destination the input.
        let destination = output_root(Path::new("/tmp/magic-archive"));

        assert_ne!(
            destination,
            PathBuf::from("/tmp/magic-archive"),
            "the destination must differ from the input"
        );
        assert_eq!(
            destination.file_name().and_then(|name| name.to_str()),
            Some("magic-archive-extracted")
        );
    }

    #[test]
    fn a_hidden_file_keeps_a_usable_name() {
        // `.7z` is a dotfile to Rust, not an extension, so the strip is a no-op
        // and the suffix branch has to catch it.
        let destination = output_root(Path::new("/tmp/.7z"));

        assert_ne!(destination, PathBuf::from("/tmp/.7z"));
    }
}
