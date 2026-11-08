//! Read-only table viewing: terminal UI, or first-sheet CSV on redirected stdout.
#[cfg(feature = "spreadsheet")]
use crate::cli::SheetFormat;
use crate::cli::SheetOptions;
use crate::core::error::TossError;
use std::path::Path;

pub fn view(path: &Path) -> Result<(), TossError> {
    view_with_options(path, SheetOptions::default())
}
pub fn view_with_options(path: &Path, options: SheetOptions) -> Result<(), TossError> {
    #[cfg(not(feature = "spreadsheet"))]
    {
        let _ = options;
        Err(TossError::UnsupportedFormat(path.to_path_buf()))
    }
    #[cfg(feature = "spreadsheet")]
    {
        use std::io::IsTerminal;
        use toss_sheet::{config::Config, workbook::Workbook};
        if options.delimiter.is_some()
            && !path.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("csv") || ext.eq_ignore_ascii_case("tsv")
            })
        {
            return Err(TossError::invalid_arguments(
                "a delimiter applies only to CSV/TSV",
            ));
        }
        if options.format == SheetFormat::Auto {
            crate::platform::console::ensure();
        }
        let interactive = options.format == SheetFormat::Auto && std::io::stdout().is_terminal();
        let no_header = options.no_header || (interactive && !options.header);
        let mut workbook = Workbook::open_with_options(path, no_header, options.delimiter)
            .map_err(|error| {
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
                {
                    TossError::PermissionDenied(path.to_path_buf())
                } else {
                    TossError::CorruptInput {
                        path: path.to_path_buf(),
                        context: format!("{error:#}"),
                    }
                }
            })?;
        let names = workbook.sheet_names();
        let sheet = if let Some(name) = options.sheet {
            if !names.contains(&name) {
                return Err(TossError::invalid_arguments(format!(
                    "worksheet {name:?} not found; available: {names:?}"
                )));
            }
            name
        } else {
            names
                .into_iter()
                .next()
                .ok_or_else(|| TossError::CorruptInput {
                    path: path.to_path_buf(),
                    context: "workbook has no sheets".into(),
                })?
        };
        if interactive {
            toss_sheet::tui::run_tui(workbook, &sheet, &Config::default(), true)
                .map_err(|error| TossError::other(format!("spreadsheet viewer failed: {error:#}")))
        } else {
            let data = workbook
                .load_sheet(&sheet)
                .map_err(|error| TossError::CorruptInput {
                    path: path.to_path_buf(),
                    context: format!("{error:#}"),
                })?;
            let result = match options.format {
                SheetFormat::Json => {
                    toss_sheet::export::json(std::io::stdout().lock(), &data, &sheet)
                }
                SheetFormat::Auto | SheetFormat::Csv => {
                    toss_sheet::export::csv(std::io::stdout().lock(), &data)
                }
            };
            result.map_err(|error| {
                TossError::other(format!("cannot write spreadsheet output: {error:#}"))
            })
        }
    }
}
