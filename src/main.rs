//! Toss bootstrap.
//!
//! Everything platform-specific stays behind the modules declared below, so
//! this file remains plain portable Rust (§10).

mod backend;
mod cli;
mod core;
mod detection;
mod dispatch;
mod handlers;
mod platform;

use crate::core::{error::TossError, exit_code::ExitCode, log};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => ExitCode::Success.into(),
        Err(err) => {
            log::error(&err);
            // Explorer launches close their console on exit; keep it readable.
            platform::console::hold_owned_window_on_failure();
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
    #[cfg(all(windows, feature = "word-pdf"))]
    {
        let mut args = std::env::args_os().skip(1);
        if args.next().as_deref() == Some(std::ffi::OsStr::new("--internal-docx-pdf")) {
            let path = args
                .next()
                .ok_or_else(|| TossError::invalid_arguments("missing DOCX path"))?;
            if args.next().is_some() {
                return Err(TossError::invalid_arguments(
                    "unexpected converter arguments",
                ));
            }
            return backend::word_pdf::worker(std::path::Path::new(&path));
        }
    }
    dispatch::run(cli::parse(std::env::args_os().skip(1))?)
}
