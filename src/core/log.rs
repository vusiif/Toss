//! Output routing.
//!
//! Results belong on stdout and diagnostics on stderr (§25), so a script can
//! read one stream while a human reads the other.

use std::fmt;

/// Write an error line to stderr as `Error: ...`.
pub fn error(err: impl fmt::Display) {
    eprintln!("Error: {err}");
}
