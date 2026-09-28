//! Output routing.
//!
//! Results belong on stdout and diagnostics on stderr (§25), so a script can
//! read one stream while a human reads the other.

use std::fmt;
use std::io::Write;

/// Write a normal result line to stdout.
pub fn out(message: &str) {
    println!("{message}");
}

/// Write an error line to stderr as `Error: ...`.
///
/// Stdout is flushed first so a report printed just before a failure is not
/// reordered underneath the reader when the two streams are interleaved.
pub fn error(err: impl fmt::Display) {
    let _ = std::io::stdout().flush();
    eprintln!("Error: {err}");
}
