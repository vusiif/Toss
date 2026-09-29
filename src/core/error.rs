//! User-facing error model.
//!
//! User-controlled input must never reach `unwrap`, `expect` or `panic!` (§23).
//! It becomes one of these instead: a message a human can act on, paired with
//! the exit code a script will observe (§24).

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::core::exit_code::ExitCode;

/// A failure Toss reports instead of panicking.
#[derive(Debug)]
pub enum TossError {
    InvalidArguments(String),
    InputNotFound(PathBuf),
    PermissionDenied(PathBuf),
    UnsupportedFormat(PathBuf),
    NotImplemented(String),
    /// The input was recognised and found broken, with libarchive's own words
    /// kept as context (§23). Distinguishing this from "unsupported" is what
    /// lets a script tell a bad file from an unhandled one (§24, code 6).
    CorruptInput {
        path: PathBuf,
        context: String,
    },
    OutputConflict(PathBuf),
    Io(io::Error),
    /// Anything with a message that has no dedicated exit code of its own.
    ///
    /// Archive security rejections and backend diagnostics land here (§23):
    /// they must reach the user as words rather than as a bare status.
    Other(String),
}

impl TossError {
    /// Build an error for a command line Toss cannot honour.
    #[must_use]
    pub fn invalid_arguments(message: impl Into<String>) -> Self {
        Self::InvalidArguments(message.into())
    }

    /// Build an error for a route Toss recognises but has not written yet.
    #[must_use]
    pub fn not_implemented(what: impl Into<String>) -> Self {
        Self::NotImplemented(what.into())
    }

    /// Build an error from a complete sentence, reported as a generic failure.
    #[must_use]
    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    /// Map an IO failure onto the most specific variant for `path`.
    ///
    /// Existence and permission problems get their own exit codes rather than
    /// collapsing into a generic failure, so scripts can branch on them (§24).
    #[must_use]
    pub fn from_io(path: &Path, err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound => Self::InputNotFound(path.to_path_buf()),
            io::ErrorKind::PermissionDenied => Self::PermissionDenied(path.to_path_buf()),
            _ => Self::Io(err),
        }
    }

    /// The exit status this error maps to (§24).
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::InvalidArguments(_) => ExitCode::InvalidArguments,
            Self::InputNotFound(_) => ExitCode::NotFound,
            Self::PermissionDenied(_) => ExitCode::PermissionDenied,
            Self::UnsupportedFormat(_) | Self::NotImplemented(_) => ExitCode::UnsupportedFormat,
            Self::CorruptInput { .. } => ExitCode::CorruptInput,
            Self::OutputConflict(_) | Self::Io(_) | Self::Other(_) => ExitCode::Failure,
        }
    }
}

impl fmt::Display for TossError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `Path::display()` renders a name Toss did not choose without failing
        // on bytes that are not valid Unicode (§22).
        match self {
            Self::InvalidArguments(message) => write!(f, "invalid arguments: {message}"),
            Self::InputNotFound(path) => write!(f, "input not found: {}", path.display()),
            Self::PermissionDenied(path) => write!(f, "permission denied: {}", path.display()),
            Self::UnsupportedFormat(path) => {
                write!(f, "unsupported format: {}", path.display())
            }
            Self::NotImplemented(what) => write!(f, "not implemented yet: {what}"),
            Self::CorruptInput { path, context } => {
                write!(
                    f,
                    "input is corrupted or incomplete: {} ({context})",
                    path.display()
                )
            }
            Self::OutputConflict(path) => write!(f, "output already exists: {}", path.display()),
            Self::Io(err) => write!(f, "{err}"),
            // The caller writes a whole sentence; adding a prefix here would
            // double up on the wording it chose (§23).
            Self::Other(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for TossError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::TossError;
    use crate::core::exit_code::ExitCode;

    #[test]
    fn every_variant_maps_to_the_tabled_exit_code() {
        let path = PathBuf::from("sample.zip");

        let cases = [
            (
                TossError::invalid_arguments("bad flag"),
                ExitCode::InvalidArguments,
            ),
            (
                TossError::from_io(&path, std::io::Error::from(std::io::ErrorKind::NotFound)),
                ExitCode::NotFound,
            ),
            (
                TossError::from_io(
                    &path,
                    std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                ),
                ExitCode::PermissionDenied,
            ),
            (
                TossError::UnsupportedFormat(path.clone()),
                ExitCode::UnsupportedFormat,
            ),
            (
                TossError::not_implemented("extract"),
                ExitCode::UnsupportedFormat,
            ),
            (
                TossError::CorruptInput {
                    path: path.clone(),
                    context: "damaged header".to_owned(),
                },
                ExitCode::CorruptInput,
            ),
            (TossError::OutputConflict(path), ExitCode::Failure),
            (
                TossError::Io(std::io::Error::from(std::io::ErrorKind::Other)),
                ExitCode::Failure,
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(err.exit_code(), expected, "wrong code for {err:?}");
        }
    }

    #[test]
    fn io_failures_keep_their_source_context() {
        // Anything that is not plainly "missing" or "denied" stays an `Io`
        // error, so the underlying reason survives for debugging (§23).
        let err = TossError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "truncated header",
        ));

        let source = std::error::Error::source(&err).expect("Io carries a source");
        assert!(source.to_string().contains("truncated header"));
    }

    #[test]
    fn an_unrecognised_io_failure_is_not_mislabelled_as_a_missing_input() {
        let path = PathBuf::from("broken.zip");
        let err = TossError::from_io(
            &path,
            std::io::Error::new(std::io::ErrorKind::InvalidData, "truncated"),
        );

        assert_eq!(err.exit_code(), ExitCode::Failure);
        assert!(!matches!(err, TossError::InputNotFound(_)));
    }

    #[test]
    fn a_unicode_path_survives_into_the_message() {
        let err = TossError::UnsupportedFormat(PathBuf::from("报告 😊.zip"));
        assert_eq!(err.to_string(), "unsupported format: 报告 😊.zip");
    }
}
