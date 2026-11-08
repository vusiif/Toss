//! Table-specific options live behind the existing view verb.
use super::{Command, Verb, is_option};
use crate::core::error::TossError;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SheetFormat {
    #[default]
    Auto,
    Csv,
    Json,
}
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SheetOptions {
    pub sheet: Option<String>,
    pub no_header: bool,
    pub delimiter: Option<u8>,
    pub format: SheetFormat,
}
pub(super) fn parse_view(args: &[OsString]) -> Result<Command, TossError> {
    let mut paths = Vec::new();
    let mut options = SheetOptions::default();
    let mut configured = false;
    let mut ended = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if ended {
            paths.push(PathBuf::from(arg));
            i += 1;
            continue;
        }
        if arg == "--" {
            ended = true;
            i += 1;
            continue;
        }
        if arg == "--help" || arg == "-h" {
            return Ok(Command::Help);
        }
        let option = arg.to_str().unwrap_or("");
        match option {
            "--no-header" => {
                if options.no_header {
                    return Err(TossError::invalid_arguments("duplicate --no-header"));
                }
                options.no_header = true;
                configured = true;
            }
            "--sheet" | "--delimiter" | "--format" => {
                i += 1;
                let value = args.get(i).and_then(|v| v.to_str()).ok_or_else(|| {
                    TossError::invalid_arguments(format!("{option} needs a UTF-8 value"))
                })?;
                match option {
                    "--sheet" => {
                        if options.sheet.is_some() {
                            return Err(TossError::invalid_arguments("duplicate --sheet"));
                        }
                        if value.is_empty() {
                            return Err(TossError::invalid_arguments(
                                "worksheet name must not be empty",
                            ));
                        }
                        options.sheet = Some(value.to_owned());
                    }
                    "--delimiter" => {
                        if options.delimiter.is_some() {
                            return Err(TossError::invalid_arguments("duplicate --delimiter"));
                        }
                        if value.len() != 1
                            || !value.is_ascii()
                            || matches!(value.as_bytes()[0], 0 | b'\n' | b'\r' | b'"')
                        {
                            return Err(TossError::invalid_arguments(
                                "delimiter must be one ASCII byte other than NUL, CR, LF or quote",
                            ));
                        }
                        options.delimiter = Some(value.as_bytes()[0]);
                    }
                    _ => {
                        if options.format != SheetFormat::Auto {
                            return Err(TossError::invalid_arguments("duplicate --format"));
                        }
                        options.format = match value {
                            "csv" => SheetFormat::Csv,
                            "json" => SheetFormat::Json,
                            _ => {
                                return Err(TossError::invalid_arguments(
                                    "format must be csv or json",
                                ));
                            }
                        };
                    }
                }
                configured = true;
            }
            _ if is_option(arg) => {
                return Err(TossError::invalid_arguments(format!(
                    "unknown option: {}",
                    arg.to_string_lossy()
                )));
            }
            _ => paths.push(PathBuf::from(arg)),
        }
        i += 1;
    }
    if paths.is_empty() {
        return Err(TossError::invalid_arguments(
            "`view` needs at least one input",
        ));
    }
    if configured {
        if paths.len() != 1 {
            return Err(TossError::invalid_arguments(
                "spreadsheet options require exactly one input file",
            ));
        }
        Ok(Command::Spreadsheet {
            path: paths.remove(0),
            options,
        })
    } else {
        Ok(Command::Explicit {
            verb: Verb::View,
            args: paths,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Command, TossError> {
        parse_view(&args.iter().map(OsString::from).collect::<Vec<_>>())
    }
    #[test]
    fn ordinary_view_and_terminated_option_paths_still_work() {
        assert_eq!(
            parse(&["a.png", "b.docx"]).unwrap(),
            Command::Explicit {
                verb: Verb::View,
                args: vec!["a.png".into(), "b.docx".into()]
            }
        );
        assert_eq!(
            parse(&["--", "--sheet"]).unwrap(),
            Command::Explicit {
                verb: Verb::View,
                args: vec!["--sheet".into()]
            }
        );
    }
    #[test]
    fn options_before_and_after_path_have_one_meaning() {
        let a = parse(&[
            "x.xlsx",
            "--sheet",
            "中文",
            "--no-header",
            "--format",
            "json",
        ])
        .unwrap();
        let b = parse(&[
            "--format",
            "json",
            "--sheet",
            "中文",
            "--no-header",
            "x.xlsx",
        ])
        .unwrap();
        assert_eq!(a, b);
        let Command::Spreadsheet { options, .. } = a else {
            panic!("wrong command")
        };
        assert_eq!(options.sheet.as_deref(), Some("中文"));
        assert!(options.no_header);
        assert_eq!(options.format, SheetFormat::Json);
    }
    #[test]
    fn incomplete_duplicate_invalid_and_multi_input_options_fail() {
        for args in [
            vec!["--sheet"],
            vec!["x.xlsx", "--format", "yaml"],
            vec!["a.csv", "b.csv", "--format", "json"],
            vec!["x.csv", "--delimiter", ";;"],
            vec!["x.csv", "--delimiter", "中"],
            vec!["x.csv", "--delimiter", "\n"],
            vec!["x.csv", "--no-header", "--no-header"],
            vec!["x.csv", "--sheet", "A", "--sheet", "B"],
            vec!["x.csv", "--format", "csv", "--format", "json"],
            vec!["x.csv", "--unknown"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }
}
