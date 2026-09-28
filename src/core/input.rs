//! Input resolution.
//!
//! Turns a path the user handed us into something Toss can actually operate
//! on, failing with a message that names the path rather than panicking (§23).
//! This is the "Input Resolver" stage of the pipeline (§9).

use std::fs;
use std::path::{Path, PathBuf};

use crate::core::error::TossError;

/// A path that exists, of a kind Toss can branch on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Directory(PathBuf),
    File(PathBuf),
}

impl Input {
    /// The path exactly as the user gave it — never rewritten or renamed.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Directory(path) | Self::File(path) => path,
        }
    }
}

/// Resolve `path`, reporting an unusable input as an error.
///
/// The original path is kept verbatim: no normalisation, no extension
/// guessing, and no renaming (§6.3, §26).
pub fn resolve(path: &Path) -> Result<Input, TossError> {
    if path.as_os_str().is_empty() {
        return Err(TossError::invalid_arguments("empty path"));
    }

    let metadata = fs::metadata(path).map_err(|err| TossError::from_io(path, err))?;

    if metadata.is_dir() {
        Ok(Input::Directory(path.to_path_buf()))
    } else {
        Ok(Input::File(path.to_path_buf()))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Input, resolve};
    use crate::core::exit_code::ExitCode;

    /// A path anchored to the crate, so the tests do not depend on the
    /// working directory the runner happened to pick.
    fn manifest(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    #[test]
    fn a_missing_input_reports_not_found_rather_than_a_crash() {
        let path = manifest("no-such-file-for-toss-tests");
        let err = resolve(&path).expect_err("this path does not exist");

        assert_eq!(err.exit_code(), ExitCode::NotFound);
        assert!(err.to_string().contains("no-such-file-for-toss-tests"));
    }

    #[test]
    fn an_existing_file_resolves_to_a_file() {
        let path = manifest("Cargo.toml");
        assert_eq!(
            resolve(&path).expect("Cargo.toml exists"),
            Input::File(path)
        );
    }

    #[test]
    fn an_existing_directory_resolves_to_a_directory() {
        let path = manifest("src");
        assert_eq!(resolve(&path).expect("src exists"), Input::Directory(path));
    }

    #[test]
    fn an_empty_path_is_a_bad_argument_rather_than_a_missing_file() {
        let err = resolve(Path::new("")).expect_err("an empty path is not an input");

        assert_eq!(err.exit_code(), ExitCode::InvalidArguments);
    }

    #[test]
    fn the_path_comes_back_unchanged() {
        let path = manifest("Cargo.toml");
        let resolved = resolve(&path).expect("exists");

        assert_eq!(resolved.path(), path.as_path());
    }

    #[test]
    fn a_path_longer_than_the_classic_limit_is_handled_without_a_panic() {
        // Well past the 260-character limit older Windows tooling enforced.
        // The path does not exist, so "not found" is the honest answer — but
        // it must never be a panic (§22, §23).
        let path = manifest(&format!("long/{}/file.txt", "segment/".repeat(40)));

        let err = resolve(&path).expect_err("this path does not exist");
        assert_eq!(err.exit_code(), ExitCode::NotFound);
    }
}
