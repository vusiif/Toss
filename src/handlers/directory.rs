//! What Toss does with an input it classified as a directory.
//!
//! §18.2's default is *compress*, and §46 puts a directory handler in front of
//! the same archive abstraction extraction uses: this module decides which
//! file to produce and refuses to replace anything already there, the router
//! decides which compiled backend writes it, and the backend decides how
//! (§9, §11).
//!
//! There is deliberately no separate `CompressionBackend`: creation and
//! extraction are two operations on one domain, so both arrive at
//! `ArchiveBackend` (§46). That is also why the error wording comes from the
//! shared translation in the parent module rather than from a copy here.

use std::path::{Path, PathBuf};

use crate::backend::archive::{self, ArchiveFormat, CreateRequest};
use crate::core::error::TossError;
use crate::core::log;

/// Compress `directory` into an archive beside it: `hello/` becomes
/// `hello.7z` (§26).
pub fn compress(directory: &Path) -> Result<(), TossError> {
    // Checked before anything else so that `toss pack report.pdf` says what is
    // actually wrong with the input instead of reporting a backend that could
    // not be reached (§4.2, §23).
    if !directory.is_dir() {
        return Err(TossError::invalid_arguments(format!(
            "{} is not a directory",
            directory.display()
        )));
    }

    let output = output_archive(directory)?;

    // §27: automatic mode never replaces what the user might already have —
    // including an archive left behind by an earlier run of this same command.
    match std::fs::symlink_metadata(&output) {
        Ok(_) => return Err(TossError::OutputConflict(output)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(TossError::from_io(&output, err)),
    }

    // The router is built here rather than in dispatch so that the handler
    // remains the only thing that knows a directory is being packed, and so
    // the single `cfg(feature = "archive")` stays in `backend/` (§9, §12).
    let router = archive::router();
    let backend = router
        .select(ArchiveFormat::SevenZip, true)
        .ok_or_else(|| TossError::not_implemented("directory compression"))?;

    #[cfg(feature = "archive")]
    let result = create_staged(directory, &output, |request| backend.create(request))?;
    #[cfg(not(feature = "archive"))]
    let result = backend
        .create(&CreateRequest {
            source: directory.to_path_buf(),
            output: output.clone(),
        })
        .map_err(|err| super::translate(err, directory))?;

    log::out(&format!(
        "Compressed {} entries to {} ({} bytes)",
        result.entries,
        output.display(),
        result.bytes_written
    ));

    // §17: reported rather than omitted, so a user who pointed at a tree with
    // a symlink in it learns that Toss chose not to store it.
    if result.skipped > 0 {
        log::out(&format!(
            "Skipped {} entries: links, devices and special files are not packed",
            result.skipped
        ));
    }

    Ok(())
}

/// Publish a completely closed archive without replacing a concurrent output.
/// The backend only sees a fresh path in an operation-owned sibling directory.
#[cfg(feature = "archive")]
fn create_staged(
    directory: &Path,
    output: &Path,
    create: impl FnOnce(&CreateRequest) -> Result<archive::CreateResult, archive::ArchiveError>,
) -> Result<archive::CreateResult, TossError> {
    let source =
        std::fs::canonicalize(directory).map_err(|err| TossError::from_io(directory, err))?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = std::fs::canonicalize(parent).map_err(|err| TossError::from_io(parent, err))?;
    // A selected directory link can resolve to an ancestor of its apparent
    // parent. Never create staging/output inside the tree being compressed.
    if parent.starts_with(&source) {
        return Err(TossError::invalid_arguments(
            "archive output would be inside the source directory",
        ));
    }
    let target = parent.join(
        output
            .file_name()
            .ok_or_else(|| TossError::invalid_arguments("archive has no output name"))?,
    );
    let stage = tempfile::Builder::new()
        .prefix(".toss-pack-")
        .tempdir_in(&parent)
        .map_err(|err| TossError::from_io(&parent, err))?;
    let request = CreateRequest {
        source,
        output: stage.path().join("archive.7z"),
    };
    let result = create(&request)
        .map_err(|err| super::translate(err, directory))
        .and_then(|result| {
            let metadata = std::fs::symlink_metadata(&request.output)
                .map_err(|err| TossError::from_io(&request.output, err))?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() == 0
                || metadata.len() != result.bytes_written
            {
                return Err(TossError::other(
                    "archive backend did not finish a regular output of the reported length",
                ));
            }
            crate::platform::archive_fs::rename_no_replace(&request.output, &target).map_err(
                |err| {
                    if std::fs::symlink_metadata(&target).is_ok() {
                        TossError::OutputConflict(target.clone())
                    } else {
                        TossError::from_io(&target, err)
                    }
                },
            )?;
            Ok(result)
        });
    let residual = stage.path().to_path_buf();
    match result {
        Ok(result) => {
            // The final archive is already committed. Cleanup failure must not
            // turn that successful publication into a misleading failure.
            if let Err(err) = stage.close() {
                eprintln!(
                    "Warning: archive was created, but temporary directory {} could not be removed: {err}",
                    residual.display()
                );
            }
            Ok(result)
        }
        Err(err) => {
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

/// `hello/` beside itself as `hello.7z` (§26).
///
/// The suffix is appended rather than swapped in with `with_extension`:
/// `my.backup/` has to become `my.backup.7z`, and replacing the extension
/// would silently drop part of the name the user chose.
///
/// A path with no final component — a filesystem root — has no name to put a
/// file next to, so it is refused rather than answered with a destination
/// Toss invented (§26: never redirect output somewhere unasked for).
fn output_archive(directory: &Path) -> Result<PathBuf, TossError> {
    let Some(name) = directory.file_name() else {
        return Err(TossError::invalid_arguments(format!(
            "{} has no name an archive could take",
            directory.display()
        )));
    };

    let mut file_name = name.to_os_string();
    file_name.push(".7z");

    Ok(directory.with_file_name(file_name))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{compress, output_archive};
    use crate::core::error::TossError;

    /// A scratch directory that cleans itself up, so a failing test does not
    /// leave output behind for the next run to trip over.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("toss-pack-{}-{name}", std::process::id()));
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

    #[test]
    fn the_archive_sits_beside_the_directory_with_its_name() {
        // §26's rule, stated as a test because §26 asks for exactly that.
        //
        // Forward slashes only: a backslash is a separator on Windows and an
        // ordinary character on Unix, so a Windows-shaped path here would
        // assert one platform's rules on the other (§22) — the mistake the
        // extraction naming test already made once.
        let cases = [
            ("/home/x/folder", "folder.7z"),
            ("./relative/folder/", "folder.7z"),
        ];

        for (input, expected) in cases {
            let output = output_archive(Path::new(input)).expect("the name is derivable");
            assert_eq!(
                output.file_name().and_then(|name| name.to_str()),
                Some(expected),
                "wrong output for {input}"
            );
        }
    }

    #[test]
    fn an_existing_extension_is_appended_to_not_replaced() {
        // `with_extension("7z")` would answer `my.7z` here and quietly lose
        // half of the name the user gave the directory.
        let output = output_archive(Path::new("./data/my.backup")).expect("the name is derivable");

        assert_eq!(
            output.file_name().and_then(|name| name.to_str()),
            Some("my.backup.7z")
        );
    }

    #[test]
    fn a_path_with_no_name_is_refused_rather_than_guessed() {
        // A filesystem root has no file name to borrow, so there is nothing
        // to put beside it. Guessing would mean sending output somewhere the
        // user never named (§26).
        let err = output_archive(Path::new("/")).expect_err("a root cannot be named");

        assert!(matches!(err, TossError::InvalidArguments(_)), "got {err:?}");
    }

    #[test]
    fn packing_a_file_says_so_instead_of_blaming_the_backend() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");

        let err = compress(&source).expect_err("a file is not a directory");

        assert!(matches!(err, TossError::InvalidArguments(_)), "got {err:?}");
        assert!(
            err.to_string().contains("is not a directory"),
            "the message must name the problem, got: {err}"
        );
    }

    #[test]
    fn an_existing_archive_is_never_replaced() {
        // §27 in the one place it is cheapest to get wrong: the conflict is
        // decided before any backend is consulted, so this holds whether or
        // not this build compiled an archive backend in.
        let scratch = Scratch::new("conflict");
        let source = scratch.path().join("box");
        std::fs::create_dir_all(&source).expect("source directory");
        let already_there = scratch.path().join("box.7z");
        std::fs::write(&already_there, b"not really an archive").expect("placeholder written");

        let err = compress(&source).expect_err("the output name is taken");

        assert!(
            matches!(err, TossError::OutputConflict(_)),
            "expected a refusal to overwrite, got {err:?}"
        );
        assert_eq!(
            std::fs::read(&already_there).expect("still readable"),
            b"not really an archive",
            "the existing file was touched"
        );
    }
}
#[cfg(all(test, feature = "archive"))]
mod staging_tests {
    use super::*;
    use archive::{ArchiveError, CreateResult};
    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let parent =
            Path::new(env!("CARGO_MANIFEST_DIR")).join(".sandbox-test/pack-staging-20261102/unit");
        std::fs::create_dir_all(&parent).unwrap();
        let root = tempfile::tempdir_in(parent).unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("original"), b"keep").unwrap();
        let output = root.path().join("source.7z");
        (root, source, output)
    }
    fn no_stage(root: &Path) {
        assert!(!std::fs::read_dir(root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".toss-pack-")
        }));
    }
    #[test]
    fn partial_backend_failure_cleans_owned_output_without_publishing() {
        let (root, source, output) = fixture();
        let error = create_staged(&source, &output, |request| {
            std::fs::write(&request.output, b"partial").unwrap();
            assert!(!output.exists());
            Err(ArchiveError::BackendFailure(
                "injected mid-pack failure".into(),
            ))
        })
        .unwrap_err();
        assert!(error.to_string().contains("injected mid-pack failure"));
        assert!(!output.exists());
        assert_eq!(std::fs::read(source.join("original")).unwrap(), b"keep");
        no_stage(root.path());
    }
    #[test]
    fn late_conflict_keeps_the_concurrent_output_and_cleans_only_our_stage() {
        let (root, source, output) = fixture();
        let error = create_staged(&source, &output, |request| {
            std::fs::write(&request.output, b"complete").unwrap();
            std::fs::write(&output, b"concurrent user output").unwrap();
            Ok(CreateResult {
                entries: 1,
                bytes_written: 8,
                skipped: 0,
            })
        })
        .unwrap_err();
        assert!(matches!(error, TossError::OutputConflict(_)));
        assert_eq!(std::fs::read(&output).unwrap(), b"concurrent user output");
        no_stage(root.path());
    }
    #[test]
    fn missing_empty_or_wrong_length_outputs_are_not_committed() {
        for mode in 0..3 {
            let (root, source, output) = fixture();
            let result = create_staged(&source, &output, |request| {
                if mode == 1 {
                    std::fs::write(&request.output, b"").unwrap();
                }
                if mode == 2 {
                    std::fs::write(&request.output, b"short").unwrap();
                }
                Ok(CreateResult {
                    entries: 0,
                    bytes_written: 0,
                    skipped: 0,
                })
            });
            assert!(result.is_err());
            assert!(!output.exists());
            no_stage(root.path());
        }
    }
    #[test]
    fn output_inside_the_canonical_source_is_refused_before_backend_or_staging() {
        let (root, source, _) = fixture();
        let output = source.join("inside.7z");
        let result = create_staged(&source, &output, |_| panic!("backend must not be called"));
        assert!(matches!(result, Err(TossError::InvalidArguments(_))));
        assert!(!output.exists());
        no_stage(root.path());
    }
}
