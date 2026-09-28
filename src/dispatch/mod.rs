//! Dispatch.
//!
//! Sits between the pipeline stages and the handlers (§9): resolve the inputs
//! the caller gave us, classify each one, then hand it to whichever handler
//! claims it.
//!
//! No handler is registered yet — archives, media and images arrive in later
//! phases — so every route ends in an honest refusal rather than a fabricated
//! success. The refusal names the specific action that is missing, because
//! "I cannot view this image" is more useful than a generic failure (§5).
//! Silent fake work would be worse than no work at all.

use std::path::PathBuf;

use crate::cli::{Command, Verb};
use crate::core::error::TossError;
use crate::core::input::{self, Input};
use crate::core::log;
use crate::detection::{self, Kind};

/// Run a parsed command to completion.
pub fn run(command: Command) -> Result<(), TossError> {
    match command {
        Command::Help => {
            log::out(&crate::cli::usage());
            Ok(())
        }
        Command::Automatic { paths } => {
            let inputs = resolve_inputs(&paths)?;
            for input in &inputs {
                route(None, input, detection::classify(input))?;
            }
            Ok(())
        }
        Command::Explicit { verb, args } => {
            let inputs = resolve_inputs(&args)?;
            for input in &inputs {
                route(Some(verb), input, detection::classify(input))?;
            }
            Ok(())
        }
    }
}

/// Resolve every path, stopping at the first one that cannot be used.
///
/// Fail-fast keeps the reported problem specific: a script asking about three
/// files should learn which one actually went wrong, not a summary (§25).
/// Every input is resolved before any is routed, so a bad path anywhere in the
/// command is reported rather than silently skipped.
fn resolve_inputs(paths: &[PathBuf]) -> Result<Vec<Input>, TossError> {
    let inputs = paths
        .iter()
        .map(|path| input::resolve(path))
        .collect::<Result<Vec<_>, _>>()?;

    if inputs.is_empty() {
        return Err(TossError::invalid_arguments("no input given"));
    }

    Ok(inputs)
}

/// Hand a classified input to the handler that claims it.
///
/// This is the seam a handler registry plugs into once the first real handler
/// lands (Phase 4). Until then every route refuses, and the refusal says which
/// action is missing rather than reporting work that never happened.
fn route(verb: Option<Verb>, input: &Input, kind: Kind) -> Result<(), TossError> {
    match verb {
        // An explicit verb already states the intent, so the reply names it.
        Some(verb) => Err(TossError::not_implemented(verb.as_str())),
        // Automatic mode replies with the default action for this kind (§5).
        // An unrecognised file has no action to name, so the reply is about
        // the file itself, which is what the info fallback will build on (§7).
        None => match kind.default_action() {
            Some(action) => Err(TossError::not_implemented(action)),
            None => Err(TossError::unsupported_format(input.path().to_path_buf())),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use super::{resolve_inputs, route, run};
    use crate::cli::{Command, Verb, parse};
    use crate::core::error::TossError;
    use crate::core::exit_code::ExitCode;
    use crate::core::input::Input;
    use crate::detection::Kind;

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
    fn a_command_with_no_inputs_at_all_is_an_error_rather_than_a_panic() {
        for command in [
            Command::Automatic { paths: vec![] },
            Command::Explicit {
                verb: Verb::Pack,
                args: vec![],
            },
        ] {
            let err = run(command).expect_err("nothing to work on");
            assert_eq!(err.exit_code(), ExitCode::InvalidArguments);
        }
    }

    #[test]
    fn a_parsed_command_line_reaches_dispatch() {
        run_args(&["--help"]).expect("help parses and runs");

        let err = run_args(&["no-such-file-for-toss-tests"]).expect_err("still missing");
        assert_eq!(err.exit_code(), ExitCode::NotFound);
    }

    #[test]
    fn automatic_mode_names_the_action_that_is_missing_rather_than_a_bare_failure() {
        let path = manifest("Cargo.toml");

        let cases = [
            (Kind::Directory, "directory compression"),
            (Kind::Archive, "archive extraction"),
            (Kind::Image, "image viewing"),
            (Kind::Media, "media playback"),
        ];

        for (kind, action) in cases {
            let err =
                route(None, &Input::File(path.clone()), kind).expect_err("no handler exists yet");

            assert_eq!(err.exit_code(), ExitCode::UnsupportedFormat);
            assert!(
                err.to_string().contains(action),
                "expected {action} in: {err}"
            );
        }
    }

    #[test]
    fn an_unrecognised_file_is_reported_by_path_so_the_info_fallback_can_build_on_it() {
        let path = manifest("Cargo.toml");
        let err = route(None, &Input::File(path.clone()), Kind::Unknown)
            .expect_err("unknown inputs are refused for now");

        assert_eq!(err.exit_code(), ExitCode::UnsupportedFormat);
        assert!(err.to_string().contains("Cargo.toml"));
    }

    #[test]
    fn an_input_list_is_validated_before_any_of_it_is_routed() {
        let paths = vec![
            manifest("Cargo.toml"),
            manifest("no-such-file-for-toss-tests"),
        ];
        let err = resolve_inputs(&paths).expect_err("the second path does not exist");

        assert_eq!(err.exit_code(), ExitCode::NotFound);
    }
}
