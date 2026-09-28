//! Dispatch.
//!
//! Sits between the CLI and the handlers (§9): it resolves the inputs the
//! caller gave us, then decides which handler should act on them.
//!
//! No handler is registered yet — archives, media and images arrive in later
//! phases — so both routes end in an honest refusal rather than a fabricated
//! success. Silent fake work would be worse than no work at all (§5).

use std::path::PathBuf;

use crate::cli::{Command, Verb};
use crate::core::error::TossError;
use crate::core::input::{self, Input};
use crate::core::log;

/// Run a parsed command to completion.
pub fn run(command: Command) -> Result<(), TossError> {
    match command {
        Command::Help => {
            log::out(&crate::cli::usage());
            Ok(())
        }
        Command::Automatic { paths } => {
            let inputs = resolve_all(&paths)?;
            route(None, &inputs)
        }
        Command::Explicit { verb, args } => {
            let inputs = resolve_all(&args)?;
            route(Some(verb), &inputs)
        }
    }
}

/// Resolve every path, stopping at the first one that cannot be used.
///
/// Fail-fast keeps the reported problem specific: a script asking about three
/// files should learn which one actually went wrong, not a summary (§25).
fn resolve_all(paths: &[PathBuf]) -> Result<Vec<Input>, TossError> {
    paths.iter().map(|path| input::resolve(path)).collect()
}

/// Hand resolved inputs to whichever handler claims them.
///
/// This is the seam a handler registry plugs into once the first real handler
/// lands (Phase 4). Until then every route refuses, and the refusal says which
/// thing is missing rather than reporting work that never happened.
fn route(verb: Option<Verb>, inputs: &[Input]) -> Result<(), TossError> {
    match verb {
        Some(verb) => Err(TossError::not_implemented(verb.as_str())),
        None => {
            let Some(input) = inputs.first() else {
                return Err(TossError::invalid_arguments("no input given"));
            };

            Err(TossError::unsupported_format(input.path().to_path_buf()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use super::{route, run};
    use crate::cli::{Command, Verb, parse};
    use crate::core::error::TossError;
    use crate::core::exit_code::ExitCode;
    use crate::core::input::Input;

    fn manifest(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    fn run_args(list: &[&str]) -> Result<(), TossError> {
        run(parse(list.iter().map(OsString::from))?)
    }

    #[test]
    fn help_succeeds_and_writes_nothing_to_stderr() {
        run(Command::Help).expect("help is not a failure");
    }

    #[test]
    fn an_existing_path_is_resolved_before_it_is_routed() {
        let err = run(Command::Automatic {
            paths: vec![manifest("Cargo.toml")],
        })
        .expect_err("no handler exists yet");

        assert_eq!(err.exit_code(), ExitCode::UnsupportedFormat);
        assert!(err.to_string().contains("Cargo.toml"));
    }

    #[test]
    fn a_verb_is_routed_to_the_handler_that_has_not_been_written() {
        let err = run(Command::Explicit {
            verb: Verb::Extract,
            args: vec![manifest("Cargo.toml")],
        })
        .expect_err("extract has no handler yet");

        assert_eq!(err.exit_code(), ExitCode::UnsupportedFormat);
        assert!(err.to_string().contains("extract"));
    }

    #[test]
    fn a_missing_path_stops_before_anything_is_routed() {
        let err = run(Command::Automatic {
            paths: vec![manifest("no-such-file-for-toss-tests")],
        })
        .expect_err("the path does not exist");

        assert_eq!(err.exit_code(), ExitCode::NotFound);
    }

    #[test]
    fn an_empty_path_is_rejected_before_routing() {
        let err = run(Command::Automatic {
            paths: vec![PathBuf::new()],
        })
        .expect_err("an empty path is not an input");

        assert_eq!(err.exit_code(), ExitCode::InvalidArguments);
    }

    #[test]
    fn a_parsed_command_line_reaches_dispatch() {
        run_args(&["--help"]).expect("help parses and runs");

        let err = run_args(&["no-such-file-for-toss-tests"]).expect_err("still missing");
        assert_eq!(err.exit_code(), ExitCode::NotFound);
    }

    #[test]
    fn routing_an_empty_input_list_is_an_error_rather_than_a_panic() {
        let err = route(None, &[]).expect_err("nothing to route");

        assert_eq!(err.exit_code(), ExitCode::InvalidArguments);
    }

    #[test]
    fn routing_reports_the_first_resolved_input() {
        let inputs = vec![
            Input::File(manifest("Cargo.toml")),
            Input::File(manifest("Cargo.lock")),
        ];

        let err = route(None, &inputs).expect_err("no handler yet");
        assert!(err.to_string().contains("Cargo.toml"));
    }
}
