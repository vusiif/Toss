//! Dispatch.
//!
//! Sits between the pipeline stages and the handlers (§9): resolve the inputs
//! the caller gave us, classify each one, then hand it to whichever handler
//! claims it.
//!
//! Archives and directories are routed to their handlers; images, media and
//! anything unclassified fall through to the info fallback, and that fallback
//! is the point: `toss <anything>` must stay useful rather than answer
//! "unsupported file" (§7). The report says what the input is; the error line
//! says which default action could not be performed, so nothing is claimed
//! that did not happen (§5, §24).

use std::path::PathBuf;

use crate::cli::{Command, Verb};
use crate::core::error::TossError;
use crate::core::info;
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
/// This is the seam a handler registry would plug into if the handlers ever
/// stopped being plain functions (§42). Automatic mode routes on what the
/// classifier decided; explicit mode routes on the verb the caller typed, and
/// the handler decides whether the input can actually be worked on (§4.2).
fn route(verb: Option<Verb>, input: &Input, kind: Kind) -> Result<(), TossError> {
    match verb {
        // `toss extract archive.7z` — the verb states the action outright
        // (§4.2), so it goes straight to the handler that performs it.
        Some(Verb::Extract) => crate::handlers::archive::extract(input.path()),

        // `toss pack folder/` — same, and the handler is the one that says
        // whether the input is something a directory packer can use (§4.2).
        Some(Verb::Pack) => crate::handlers::directory::compress(input.path()),

        // The remaining verbs have no handler yet, so naming one is still
        // the most useful reply (§4.2).
        Some(verb) => Err(TossError::not_implemented(verb.as_str())),

        None => match kind {
            // `toss archive.7z` — the classifier already decided what this
            // is (§6), and dispatch exists to hand that decision onward (§9).
            Kind::Archive => crate::handlers::archive::extract(input.path()),

            // `toss folder/` — §18.2's default action for a directory.
            Kind::Directory => crate::handlers::directory::compress(input.path()),

            // Everything else still has no handler: describe the input, then
            // say which default action is missing (§7, §5).
            other => {
                log::out(&info::gather(input.path()).render(other.label()));

                match other.default_action() {
                    Some(action) => Err(TossError::not_implemented(action)),
                    // Nothing was ever going to happen to this file, so the
                    // description is the entire answer — and that is a success.
                    None => Ok(()),
                }
            }
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

    /// A scratch directory that cleans itself up, so a failing test does not
    /// leave output behind for the next run to trip over.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("toss-dispatch-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn help_succeeds_and_writes_nothing_to_stderr() {
        run(Command::Help).expect("help is not a failure");
    }

    #[test]
    fn an_existing_input_is_resolved_before_it_is_routed() {
        // A directory has a defined default action (§5), so routing reaches
        // the handler rather than stopping at resolution — and the refusal
        // below is raised *by that handler*, which is what proves both halves
        // ran. The conflict is decided before any backend is consulted, so
        // this holds whether or not this build compiled one in (§27).
        let scratch = Scratch::new("resolved");
        let source = scratch.path().join("box");
        std::fs::create_dir_all(&source).expect("source directory");
        std::fs::write(scratch.path().join("box.7z"), b"already here").expect("placeholder");

        let err = run(Command::Automatic {
            paths: vec![source],
        })
        .expect_err("the output name is taken");

        assert_eq!(err.exit_code(), ExitCode::Failure);
        assert!(
            err.to_string().contains("output already exists"),
            "expected the conflict to be named, got: {err}"
        );
    }

    #[test]
    fn the_pack_verb_reaches_the_directory_handler() {
        // The verb is routed before the classifier's verdict matters, so a
        // file handed to `pack` has to be refused by the handler itself —
        // which is only possible if dispatch actually got that far (§4.2).
        let err = run(Command::Explicit {
            verb: Verb::Pack,
            args: vec![manifest("Cargo.toml")],
        })
        .expect_err("a file cannot be packed");

        assert_eq!(err.exit_code(), ExitCode::InvalidArguments);
        assert!(
            err.to_string().contains("is not a directory"),
            "expected the handler's own complaint, got: {err}"
        );
    }

    #[test]
    fn an_unknown_file_is_described_instead_of_being_rejected() {
        // §7: `toss <anything>` has defined behaviour, and for an input no
        // default action covers, describing it *is* that behaviour.
        let err = run(Command::Automatic {
            paths: vec![manifest("Cargo.toml")],
        });

        assert!(err.is_ok(), "an unknown file must not fail: {err:?}");
    }

    #[test]
    fn a_verb_with_no_handler_still_names_what_is_missing() {
        // `extract` has a handler as of step 7; the verbs that do not still
        // answer with the thing they are missing (§4.2).
        let err = run(Command::Explicit {
            verb: Verb::View,
            args: vec![manifest("Cargo.toml")],
        })
        .expect_err("there is no image viewer yet");

        assert_eq!(err.exit_code(), ExitCode::UnsupportedFormat);
        assert!(err.to_string().contains("view"));
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
    fn a_kind_with_a_default_action_states_the_action_it_could_not_perform() {
        let path = manifest("Cargo.toml");

        // `Kind::Archive` and `Kind::Directory` are absent on purpose: both
        // are wired to handlers now, so neither reports a missing action.
        let cases = [
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
    fn an_input_list_is_validated_before_any_of_it_is_routed() {
        let paths = vec![
            manifest("Cargo.toml"),
            manifest("no-such-file-for-toss-tests"),
        ];
        let err = resolve_inputs(&paths).expect_err("the second path does not exist");

        assert_eq!(err.exit_code(), ExitCode::NotFound);
    }
}
