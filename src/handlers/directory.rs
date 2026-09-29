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
    if output.exists() {
        return Err(TossError::OutputConflict(output));
    }

    let request = CreateRequest {
        source: directory.to_path_buf(),
        output: output.clone(),
    };

    // The router is built here rather than in dispatch so that the handler
    // remains the only thing that knows a directory is being packed, and so
    // the single `cfg(feature = "archive")` stays in `backend/` (§9, §12).
    let router = archive::router();
    let backend = router
        .select(ArchiveFormat::SevenZip, true)
        .ok_or_else(|| TossError::not_implemented("directory compression"))?;

    let result = backend
        .create(&request)
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
