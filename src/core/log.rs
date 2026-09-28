//! Output routing.
//!
//! Results belong on stdout and diagnostics on stderr (§25), so a script can
//! read one stream while a human reads the other.

use std::fmt;

/// Write a normal result line to stdout.
pub fn out(message: &str) {
    println!("{message}");
}

/// Write an error line to stderr as `Error: ...`.
pub fn error(err: impl fmt::Display) {
    eprintln!("Error: {err}");
}
