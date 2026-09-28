//! Toss bootstrap.
//!
//! Everything platform-specific stays behind the modules declared below, so
//! this file remains plain portable Rust (§10).

mod backend;
mod cli;
mod core;
mod detect;
mod dispatch;
mod handlers;
mod platform;

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

/// Parse the command line, resolve its inputs, and dispatch it (§9).
///
/// Anything that goes wrong comes back as an error carrying both a message
/// for the human and an exit code for the script; nothing here panics on
/// input the user chose (§23, §24).
fn run() -> Result<(), TossError> {
    dispatch::run(cli::parse(std::env::args_os().skip(1))?)
}
