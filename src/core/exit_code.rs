//! Process exit codes.
//!
//! Scripts depend on these numbers, so they are a contract rather than an
//! implementation detail (§24).

/// The status Toss exits with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExitCode {
    Success,
    Failure,
    InvalidArguments,
    UnsupportedFormat,
    NotFound,
    PermissionDenied,
    CorruptInput,
}

impl ExitCode {
    /// The numeric value a shell or CI job observes.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Failure => 1,
            Self::InvalidArguments => 2,
            Self::UnsupportedFormat => 3,
            Self::NotFound => 4,
            Self::PermissionDenied => 5,
            Self::CorruptInput => 6,
        }
    }
}

impl From<ExitCode> for std::process::ExitCode {
    fn from(value: ExitCode) -> Self {
        Self::from(value.code())
    }
}

#[cfg(test)]
mod tests {
    use super::ExitCode;

    const ALL: [ExitCode; 7] = [
        ExitCode::Success,
        ExitCode::Failure,
        ExitCode::InvalidArguments,
        ExitCode::UnsupportedFormat,
        ExitCode::NotFound,
        ExitCode::PermissionDenied,
        ExitCode::CorruptInput,
    ];

    #[test]
    fn codes_match_the_documented_table() {
        assert_eq!(ExitCode::Success.code(), 0);
        assert_eq!(ExitCode::Failure.code(), 1);
        assert_eq!(ExitCode::InvalidArguments.code(), 2);
        assert_eq!(ExitCode::UnsupportedFormat.code(), 3);
        assert_eq!(ExitCode::NotFound.code(), 4);
        assert_eq!(ExitCode::PermissionDenied.code(), 5);
        assert_eq!(ExitCode::CorruptInput.code(), 6);
    }

    #[test]
    fn no_two_codes_collide() {
        for (index, first) in ALL.iter().enumerate() {
            for second in &ALL[index + 1..] {
                assert_ne!(
                    first.code(),
                    second.code(),
                    "{first:?} collides with {second:?}"
                );
            }
        }
    }
}
