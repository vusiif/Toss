//! Word document viewer handler.
//!
//! Handles `.docx` document viewing using vendored doxx (WORD_VIEWER.md §4.C).
//!
//! Automatic mode and `view` verb both route here (§9).
//! TTY: interactive terminal viewer with TerminalGuard.
//! Redirected stdout: clean plain text output without ANSI codes, exits immediately.
//! Missing console on Windows (e.g. explorer drag-and-drop): attaches or allocates console.
//! Zero disk configuration files created, zero history files, no clipboard touches.

use std::path::Path;

use crate::core::error::TossError;

#[cfg(feature = "word")]
#[path = "word_legacy.rs"]
mod legacy;

/// View `path` as a Word document, or report why it cannot be viewed.
pub fn view(path: &Path) -> Result<(), TossError> {
    if !path.exists() {
        return Err(TossError::InputNotFound(path.to_path_buf()));
    }

    if !path.is_file() {
        return Err(TossError::invalid_arguments(format!(
            "{} is not a document file",
            path.display()
        )));
    }

    #[cfg(not(feature = "word"))]
    {
        Err(TossError::UnsupportedFormat(path.to_path_buf()))
    }

    #[cfg(feature = "word")]
    {
        ensure_console();

        use std::io::IsTerminal;
        let interactive = std::io::stdout().is_terminal();
        let image_options = doxx::document::ImageOptions {
            enabled: true,
            ..doxx::document::ImageOptions::default()
        };

        let document = if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("doc"))
        {
            if interactive {
                legacy::load(path, image_options)?
            } else {
                legacy::load_for_text(path, image_options)?
            }
        } else if !interactive {
            doxx::document::load_document_for_text(path)
                .map_err(|err| translate_doxx_error(err, path))?
        } else {
            doxx::document::load_document(path, image_options)
                .map_err(|err| translate_doxx_error(err, path))?
        };

        let app_options = doxx::AppOptions {
            color: true,
            images: true,
            ..doxx::AppOptions::default()
        };

        let open_image = |request: &doxx::ui::DocumentImageRequest| {
            crate::platform::image::view(&crate::platform::image::ViewRequest {
                images: request.images.clone(),
                index: request.index,
            })
            .map_err(std::io::Error::other)
        };
        #[cfg(feature = "word-pdf")]
        let mut cached_pdf: Option<Vec<u8>> = None;
        #[cfg(feature = "word-pdf")]
        let source_metadata =
            std::fs::metadata(path).map_err(|err| TossError::from_io(path, err))?;
        #[cfg(feature = "word-pdf")]
        let mut open_pdf = || {
            let current = std::fs::metadata(path)?;
            if current.len() != source_metadata.len()
                || current.modified().ok() != source_metadata.modified().ok()
            {
                return Err(std::io::Error::other(
                    "Document changed; reopen it before previewing PDF",
                ));
            }
            if cached_pdf.is_none() {
                cached_pdf =
                    Some(crate::backend::word_pdf::convert(path).map_err(std::io::Error::other)?);
            }
            crate::platform::pdf::view_bytes(cached_pdf.as_ref().unwrap().clone(), path)
                .map_err(std::io::Error::other)
        };
        #[cfg(feature = "word-pdf")]
        let pdf_callback: Option<&mut dyn FnMut() -> std::io::Result<()>> =
            if crate::platform::pdf::AVAILABLE
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("docx"))
            {
                Some(&mut open_pdf)
            } else {
                None
            };
        #[cfg(not(feature = "word-pdf"))]
        let pdf_callback = None;
        doxx::ui::run_toss_viewer_with_previews(document, &app_options, open_image, pdf_callback)
            .map_err(|err| TossError::from_io(path, err))
    }
}

#[cfg(feature = "word")]
fn translate_doxx_error(err: doxx::document::DocumentError, path: &Path) -> TossError {
    use doxx::document::package::PackageError;

    if let Some(pkg_err) = err.downcast_ref::<PackageError>() {
        return match pkg_err {
            PackageError::ZipError(msg) => TossError::CorruptInput {
                path: path.to_path_buf(),
                context: msg.clone(),
            },
            PackageError::EncryptedPackage => TossError::CorruptInput {
                path: path.to_path_buf(),
                context: "encrypted or password-protected DOCX packages are not supported"
                    .to_string(),
            },
            PackageError::PathTraversal(entry) => TossError::CorruptInput {
                path: path.to_path_buf(),
                context: format!("security violation: path traversal entry '{entry}'"),
            },
            PackageError::MissingMainDocument => TossError::CorruptInput {
                path: path.to_path_buf(),
                context: "missing main document part".to_string(),
            },
            PackageError::DuplicateMainDocument(entry) => TossError::CorruptInput {
                path: path.to_path_buf(),
                context: format!("duplicate main document part '{entry}'"),
            },
            PackageError::BudgetExceeded(msg) => TossError::CorruptInput {
                path: path.to_path_buf(),
                context: format!("resource budget exceeded: {msg}"),
            },
            PackageError::IoError(io_err) => match io_err.kind() {
                std::io::ErrorKind::NotFound => TossError::InputNotFound(path.to_path_buf()),
                std::io::ErrorKind::PermissionDenied => {
                    TossError::PermissionDenied(path.to_path_buf())
                }
                _ => TossError::other(format!("IO error reading package: {io_err}")),
            },
        };
    }

    if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
        return match io_err.kind() {
            std::io::ErrorKind::NotFound => TossError::InputNotFound(path.to_path_buf()),
            std::io::ErrorKind::PermissionDenied => TossError::PermissionDenied(path.to_path_buf()),
            _ => TossError::other(format!("IO error: {io_err}")),
        };
    }

    TossError::CorruptInput {
        path: path.to_path_buf(),
        context: err.to_string(),
    }
}

#[cfg(all(feature = "word", windows))]
fn ensure_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleWindow() -> *mut std::ffi::c_void;
        fn AllocConsole() -> i32;
        fn AttachConsole(dw_process_id: u32) -> i32;
    }

    // SAFETY: Attaching to or allocating a Windows console window via Win32 kernel32
    // does not mutate arbitrary memory or violate Rust invariants.
    unsafe {
        if GetConsoleWindow().is_null() {
            const ATTACH_PARENT_PROCESS: u32 = 0xFFFFFFFF;
            if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
                AllocConsole();
            }
        }
    }
}

#[cfg(all(feature = "word", not(windows)))]
fn ensure_console() {}
