//! What Toss does with an input it classified as an archive.
//!
//! §9's chain lands here: dispatch decides *this is an archive*, this module
//! decides *where it goes* and *what to ask for*, and the router decides
//! *which compiled backend* does it (§1, §7). Nothing below this file knows
//! how an archive is decoded — that is the backend's business (§11).

use std::path::{Path, PathBuf};

#[cfg(feature = "archive")]
use crate::backend::archive::{self, ArchiveFormat, ArchiveInput, ExtractRequest};
use crate::core::error::TossError;
#[cfg(feature = "archive")]
use crate::core::log;

/// Extract `archive` beside itself: `foo.7z` becomes `foo/` (§21).
pub fn extract(archive: &Path) -> Result<(), TossError> {
    extract_with_options(archive, crate::cli::ExtractOptions::default())
}

pub fn extract_with_options(
    archive: &Path,
    options: crate::cli::ExtractOptions,
) -> Result<(), TossError> {
    #[cfg(not(feature = "archive"))]
    {
        let _ = (archive, options);
        Err(TossError::not_implemented("archive extraction"))
    }
    #[cfg(feature = "archive")]
    {
        use crate::backend::archive::budget::space_required;
        let destination = output_root(archive);
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent =
            std::fs::canonicalize(parent).map_err(|err| TossError::from_io(parent, err))?;
        let destination = parent.join(
            destination
                .file_name()
                .ok_or_else(|| TossError::invalid_arguments("archive has no output name"))?,
        );
        match std::fs::symlink_metadata(&destination) {
            Ok(_) => return Err(TossError::OutputConflict(destination)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(TossError::from_io(&destination, err)),
        }
        let router = archive::router();
        let backend = router
            .select(ArchiveFormat::Unknown, false)
            .ok_or_else(|| TossError::not_implemented("archive extraction"))?;
        let input = ArchiveInput::single(archive.to_path_buf());
        let summary = backend
            .summary(&input, options.limits)
            .map_err(|err| super::translate(err, archive))?;
        let disk = crate::platform::archive_fs::disk_space(&parent);
        let required = disk
            .as_ref()
            .ok()
            .and_then(|disk| space_required(summary, disk.allocation_unit, options.reserve_bytes));
        let risk = match (required, disk.as_ref().ok()) {
            (Some(required), Some(disk)) if required <= disk.available => None,
            (Some(required), Some(disk)) => Some(format!(
                "Estimated output plus reserve: {required} bytes; target volume available: {} bytes.",
                disk.available
            )),
            _ => Some(format!(
                "Cannot reliably estimate disk requirements: {} declared bytes, {} regular files with unknown sizes. {}",
                summary.declared_bytes,
                summary.unknown_sizes,
                disk.as_ref()
                    .err()
                    .map(|err| format!("Space query failed: {err}."))
                    .unwrap_or_default()
            )),
        };
        if let Some(risk) = risk {
            eprintln!(
                "Warning: {risk} Extraction may exhaust disk space. Hard output limits remain active."
            );
            if !options.accept_space_risk {
                confirm_space_risk()?;
            }
        }
        // Same-volume staging: no second full copy at commit. TempDir owns only
        // this newly created unique directory; never clean the final target.
        let stage = tempfile::Builder::new()
            .prefix(".toss-extract-")
            .tempdir_in(&parent)
            .map_err(|err| TossError::from_io(&parent, err))?;
        let request = ExtractRequest {
            input,
            output_root: stage.path().to_path_buf(),
            limits: options.limits,
        };
        let result = backend
            .extract(&request)
            .map_err(|err| super::translate(err, archive))
            .and_then(|result| {
                crate::platform::archive_fs::rename_no_replace(stage.path(), &destination)
                    .map_err(|err| {
                        if std::fs::symlink_metadata(&destination).is_ok() {
                            TossError::OutputConflict(destination.clone())
                        } else {
                            TossError::from_io(&destination, err)
                        }
                    })?;
                Ok(result)
            });
        match result {
            Ok(result) => {
                let _ = stage.keep();
                log::out(&format!(
                    "Extracted {} entries to {}",
                    result.entries,
                    destination.display()
                ));
                if result.skipped > 0 {
                    log::out(&format!(
                        "Skipped {} entries: links, devices and special files are not restored",
                        result.skipped
                    ));
                }
                Ok(())
            }
            Err(err) => {
                let residual = stage.path().to_path_buf();
                if let Err(cleanup) = stage.close() {
                    return Err(TossError::other(format!(
                        "{err}; could not remove this operation's temporary output {}: {cleanup}",
                        residual.display()
                    )));
                }
                Err(err)
            }
        }
    }
}

#[cfg(feature = "archive")]
fn confirm_space_risk() -> Result<(), TossError> {
    use std::io::{BufRead, IsTerminal, Read, Write};
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return Err(TossError::other(
            "space risk requires an interactive confirmation; use extract --accept-space-risk to explicitly accept it (hard limits still apply)",
        ));
    }
    eprint!("Continue? [y/N] ");
    std::io::stderr().flush().map_err(TossError::Io)?;
    let mut answer = Vec::new();
    stdin
        .lock()
        .take(16)
        .read_until(b'\n', &mut answer)
        .map_err(TossError::Io)?;
    if answer.ends_with(b"\n") && answer.trim_ascii().eq_ignore_ascii_case(b"y") {
        Ok(())
    } else {
        Err(TossError::other(
            "extraction cancelled before output was created",
        ))
    }
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
#[cfg_attr(not(feature = "archive"), allow(dead_code))]
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
