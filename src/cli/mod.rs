//! Command-line surface.
//!
//! Two levels, exactly as the CLI model defines them (§4): `toss <path>` asks
//! Toss to work out the safest useful action, and `toss <verb> ...` states the
//! intent outright. Arguments beginning with `-` are read as options so that
//! flags can be added later without re-deciding what a path looks like (§4.3).
//!
//! Paths are carried as `OsString`/`PathBuf` end to end (§22). The only place
//! a conversion to `str` happens is on verbs and options, which are ASCII words
//! by definition — never on a path.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use crate::core::error::TossError;

/// An explicit verb from the CLI model (§4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Extract,
    Pack,
    Play,
    View,
    Info,
    Hash,
}

impl Verb {
    /// Every verb Toss recognises, in the order `--help` lists them.
    pub const ALL: [Verb; 6] = [
        Verb::Extract,
        Verb::Pack,
        Verb::Play,
        Verb::View,
        Verb::Info,
        Verb::Hash,
    ];

    /// Read a verb out of the leading argument, if that is what it is.
    pub fn from_os_str(value: &OsStr) -> Option<Self> {
        value.to_str().and_then(Self::from_str)
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "extract" => Some(Self::Extract),
            "pack" => Some(Self::Pack),
            "play" => Some(Self::Play),
            "view" => Some(Self::View),
            "info" => Some(Self::Info),
            "hash" => Some(Self::Hash),
            _ => None,
        }
    }

    /// The spelling that appears on the command line and in diagnostics.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Extract => "extract",
            Self::Pack => "pack",
            Self::Play => "play",
            Self::View => "view",
            Self::Info => "info",
            Self::Hash => "hash",
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::Extract => "unpack an archive",
            Self::Pack => "compress a directory",
            Self::Play => "play a video or audio file",
            Self::View => "open an image",
            Self::Info => "inspect a file",
            Self::Hash => "print a checksum",
        }
    }
}

/// What the command line asked Toss to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// `toss --help`
    Help,
    /// `toss <path> ...` — Toss decides.
    Automatic { paths: Vec<PathBuf> },
    /// `toss <verb> <path> ...` — the caller decides.
    Explicit { verb: Verb, args: Vec<PathBuf> },
}

/// Read a command line. `args` must already exclude the program name.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, TossError> {
    let args: Vec<OsString> = args.into_iter().collect();

    if args.is_empty() {
        return Err(TossError::invalid_arguments("no input given"));
    }

    let mut positionals: Vec<OsString> = Vec::new();
    let mut options_ended = false;

    for arg in args {
        if options_ended {
            positionals.push(arg);
            continue;
        }
        if arg == "--" {
            options_ended = true;
            continue;
        }
        if is_option(&arg) {
            if arg == "--help" || arg == "-h" {
                return Ok(Command::Help);
            }
            return Err(TossError::invalid_arguments(format!(
                "unknown option: {}",
                arg.to_string_lossy()
            )));
        }
        positionals.push(arg);
    }

    let Some(leading) = positionals.first() else {
        return Err(TossError::invalid_arguments("no input given"));
    };

    let Some(verb) = Verb::from_os_str(leading) else {
        return Ok(Command::Automatic {
            paths: to_paths(&positionals),
        });
    };

    let inputs = to_paths(&positionals[1..]);
    if inputs.is_empty() {
        return Err(TossError::invalid_arguments(format!(
            "`{}` needs at least one input",
            verb.as_str()
        )));
    }

    Ok(Command::Explicit { verb, args: inputs })
}

/// An argument that starts with `-` but is not simply `-`.
fn is_option(arg: &OsStr) -> bool {
    let Some(text) = arg.to_str() else {
        // Not valid UTF-8, so it cannot be one of our ASCII options; it can
        // only be a path, and must be read as one (§22).
        return false;
    };

    text.starts_with('-') && text.len() > 1
}

fn to_paths(values: &[OsString]) -> Vec<PathBuf> {
    // Deliberately no validation here: an empty or unreadable path is an
    // input problem, and the input resolver reports it with exit code 2 or 4.
    values.iter().map(PathBuf::from).collect()
}

const USAGE_HEAD: &str = "\
Toss - drop a file on it, Toss figures out what to do

Usage:
  toss <path> ...         choose the safest useful default action
  toss <verb> <path> ...  do exactly that

Verbs:
";

const USAGE_TAIL: &str = "
Options:
  -h, --help  show this help
  --          treat every following argument as a path

Not every verb has an implementation yet; Toss reports which ones are missing.
";

/// The full help text. Written to stdout for `--help` (§25).
pub fn usage() -> String {
    let mut text = String::from(USAGE_HEAD);

    for verb in Verb::ALL {
        text.push_str(&format!("  {:<8} {}\n", verb.as_str(), verb.description()));
    }

    text.push_str(USAGE_TAIL);
    text
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    use super::{Command, Verb, parse, usage};
    use crate::core::error::TossError;
    use crate::core::exit_code::ExitCode;
    use crate::core::input::resolve;

    fn parse_args(list: &[&str]) -> Result<Command, TossError> {
        parse(list.iter().map(OsString::from))
    }

    fn automatic(list: &[&str]) -> Vec<PathBuf> {
        match parse_args(list).expect("parses as a command") {
            Command::Automatic { paths } => paths,
            other => panic!("expected automatic mode, got {other:?}"),
        }
    }

    #[test]
    fn no_arguments_is_an_invalid_command_line() {
        let err = parse(std::iter::empty::<OsString>()).expect_err("nothing to do");

        assert_eq!(err.exit_code(), ExitCode::InvalidArguments);
    }

    #[test]
    fn help_is_recognised_in_both_spellings() {
        for flag in ["-h", "--help"] {
            assert_eq!(parse_args(&[flag]).expect("help parses"), Command::Help);
        }
    }

    #[test]
    fn an_unknown_option_is_rejected_rather_than_read_as_a_path() {
        let err = parse_args(&["--json"]).expect_err("not implemented");

        assert_eq!(err.exit_code(), ExitCode::InvalidArguments);
        assert!(err.to_string().contains("--json"));
    }

    #[test]
    fn a_single_path_enters_automatic_mode() {
        assert_eq!(automatic(&["photo.jpg"]), vec![PathBuf::from("photo.jpg")]);
    }

    #[test]
    fn a_leading_verb_enters_explicit_mode() {
        assert_eq!(
            parse_args(&["extract", "archive.7z"]).expect("parses"),
            Command::Explicit {
                verb: Verb::Extract,
                args: vec![PathBuf::from("archive.7z")],
            }
        );
    }

    #[test]
    fn a_verb_alone_is_not_a_complete_command() {
        let err = parse_args(&["pack"]).expect_err("pack needs an input");

        assert_eq!(err.exit_code(), ExitCode::InvalidArguments);
        assert!(err.to_string().contains("pack"));
    }

    #[test]
    fn spaces_unicode_emoji_long_absolute_and_unc_paths_all_survive() {
        let long = format!("nested/{}/file.txt", "deep/".repeat(60));

        let cases = [
            "a file with spaces.jpg".to_owned(),
            "中文目录/测试 图片 😊.png".to_owned(),
            "/home/张三/文档/report.pdf".to_owned(),
            r"C:\Users\张三\Desktop\照片 001.bmp".to_owned(),
            r"\\server\share\目录\file.zip".to_owned(),
            long,
            "./relative/../toward/file.mkv".to_owned(),
        ];

        for case in cases {
            assert_eq!(
                automatic(&[case.as_str()]),
                vec![PathBuf::from(case.as_str())]
            );
        }
    }

    #[test]
    fn double_dash_protects_paths_that_look_like_options() {
        assert_eq!(automatic(&["--", "--help"]), vec![PathBuf::from("--help")]);
        assert_eq!(automatic(&["--", "-h"]), vec![PathBuf::from("-h")]);
    }

    #[test]
    fn a_lone_dash_is_a_path_not_an_option() {
        assert_eq!(automatic(&["-"]), vec![PathBuf::from("-")]);
    }

    #[test]
    fn several_paths_are_all_kept() {
        assert_eq!(
            automatic(&["a.jpg", "b.jpg", "c.jpg"]),
            vec![
                PathBuf::from("a.jpg"),
                PathBuf::from("b.jpg"),
                PathBuf::from("c.jpg"),
            ]
        );
    }

    #[test]
    fn help_lists_every_verb() {
        let text = usage();

        for verb in Verb::ALL {
            assert!(text.contains(verb.as_str()), "help omits {}", verb.as_str());
        }
        assert!(text.contains("-h, --help"));
        assert!(text.contains("toss <path>"));
    }

    /// Build an argument that starts with `-` but is not valid UTF-8.
    ///
    /// Each platform needs its own constructor, so the `cfg` sits here in
    /// test code: everything under test stays portable, and a binary-only
    /// crate cannot be reached from an integration test, so this is the only
    /// door the check fits through (§12, §22).
    #[cfg(unix)]
    fn dash_but_not_utf8() -> OsString {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(vec![b'-', 0xff])
    }

    #[cfg(windows)]
    fn dash_but_not_utf8() -> OsString {
        use std::os::windows::ffi::OsStringExt;
        OsString::from_wide(&[0x2d, 0xd800])
    }

    #[test]
    fn a_path_that_is_not_valid_utf8_is_never_read_as_an_option() {
        let arg = dash_but_not_utf8();
        assert!(
            arg.to_str().is_none(),
            "the fixture must not be valid UTF-8"
        );

        let command = parse([arg.clone()]).expect("a non-UTF-8 argument is still a command");
        let paths = match command {
            Command::Automatic { paths } => paths,
            other => panic!("expected automatic mode, got {other:?}"),
        };
        assert_eq!(paths, vec![PathBuf::from(&arg)]);

        let err = resolve(Path::new(&arg)).expect_err("this path does not exist");
        assert_eq!(err.exit_code(), ExitCode::NotFound);
    }
}
