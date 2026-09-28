//! Toss bootstrap.
//!
//! Everything platform-specific stays behind the modules declared below, so
//! this file remains plain portable Rust (§10).

mod backend;
mod cli;
mod core;
mod detect;
mod handlers;
mod platform;

use std::path::PathBuf;

use crate::core::{error::TossError, exit_code::ExitCode, log};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => ExitCode::Success.into(),
        Err(err) => {
            log::error(&err);
            err.exit_code().into()
        }
    }
}

/// Resolve the command line and hand it to the dispatcher.
///
/// Only the exit-code contract is final here: a user-facing error on stderr
/// plus a meaningful status (§24, §25). Phase 1 replaces the body with real
/// argument parsing and dispatch; until then Toss says plainly that no
/// handler has claimed the input instead of pretending it succeeded.
fn run() -> Result<(), TossError> {
    let Some(input) = std::env::args_os().nth(1) else {
        return Err(TossError::invalid_arguments(
            "no input given; try `toss <path>`",
        ));
    };

    // `OsString` -> `PathBuf` keeps every byte of the original path, so
    // spaces, CJK text and emoji survive intact (§22).
    Err(TossError::unsupported_format(PathBuf::from(input)))
}
