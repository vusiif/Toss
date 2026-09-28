//! User-facing error model.
//!
//! User-controlled input must never reach `unwrap`, `expect` or `panic!` (§23).
//! It becomes one of these instead: a message a human can act on, paired with
//! the exit code a script will observe (§24).
//!
//! The variants mirror the §24 exit-code table one for one. Most have no
//! constructor yet, because nothing in this phase can produce that failure —
//! input resolution, conflict policy and archive validation arrive later and
//! each adds its own constructor as it lands.

use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::core::exit_code::ExitCode;

/// A failure Toss reports instead of panicking.
#[derive(Debug)]
#[allow(dead_code)] // Reserved failure modes; each gains a constructor when its phase lands.
pub enum TossError {
    InvalidArguments(String),
    InputNotFound(PathBuf),
    PermissionDenied(PathBuf),
    UnsupportedFormat(PathBuf),
    CorruptInput(PathBuf),
    OutputConflict(PathBuf),
    Io(io::Error),
    Other(String),
}

impl TossError {
    /// Build an error for a command line Toss cannot honour.
    #[must_use]
    pub fn invalid_arguments(message: impl Into<String>) -> Self {
        Self::InvalidArguments(message.into())
    }

    /// Build an error for a path no handler claims.
    #[must_use]
    pub fn unsupported_format(path: impl Into<PathBuf>) -> Self {
        Self::UnsupportedFormat(path.into())
    }

    /// The exit status this error maps to (§24).
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::InvalidArguments(_) => ExitCode::InvalidArguments,
            Self::InputNotFound(_) => ExitCode::NotFound,
            Self::PermissionDenied(_) => ExitCode::PermissionDenied,
            Self::UnsupportedFormat(_) => ExitCode::UnsupportedFormat,
            Self::CorruptInput(_) => ExitCode::CorruptInput,
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
                write!(f, "no handler for this input yet: {}", path.display())
            }
            Self::CorruptInput(path) => {
                write!(f, "input is corrupted or incomplete: {}", path.display())
            }
            Self::OutputConflict(path) => write!(f, "output already exists: {}", path.display()),
            Self::Io(err) => write!(f, "{err}"),
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
            (TossError::InputNotFound(path.clone()), ExitCode::NotFound),
            (
                TossError::PermissionDenied(path.clone()),
                ExitCode::PermissionDenied,
            ),
            (
                TossError::unsupported_format(path.clone()),
                ExitCode::UnsupportedFormat,
            ),
            (
                TossError::CorruptInput(path.clone()),
                ExitCode::CorruptInput,
            ),
            (TossError::OutputConflict(path.clone()), ExitCode::Failure),
            (
                TossError::Io(std::io::Error::from(std::io::ErrorKind::Other)),
                ExitCode::Failure,
            ),
            (TossError::Other("boom".to_owned()), ExitCode::Failure),
        ];

        for (err, expected) in cases {
            assert_eq!(err.exit_code(), expected, "wrong code for {err:?}");
        }
    }

    #[test]
    fn io_failures_keep_their_source_context() {
        let err = TossError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no such entry",
        ));

        let source = std::error::Error::source(&err).expect("Io carries a source");
        assert!(source.to_string().contains("no such entry"));
    }

    #[test]
    fn a_unicode_path_survives_into_the_message() {
        let err = TossError::unsupported_format(PathBuf::from("报告 😊.zip"));
        assert_eq!(
            err.to_string(),
            "no handler for this input yet: 报告 😊.zip"
        );
    }
}
