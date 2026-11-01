//! On-demand DOCX conversion. The same executable runs the converter in a child
//! process so a timeout or converter panic does not terminate the Word reader.
//! Byte budgets are not a hard bound on the renderer's internal memory usage.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::core::error::TossError;

const MAX_INPUT: u64 = 128 * 1024 * 1024;
const MAX_OUTPUT: u64 = 128 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);

/// Internal child entry point, not an export verb. No document output on disk.
pub fn worker(path: &Path) -> Result<(), TossError> {
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("docx"))
    {
        return Err(TossError::other(
            "PDF conversion currently supports DOCX only",
        ));
    }
    let file = std::fs::File::open(path).map_err(|e| TossError::from_io(path, e))?;
    if file
        .metadata()
        .map_err(|e| TossError::from_io(path, e))?
        .len()
        > MAX_INPUT
    {
        return Err(TossError::other(
            "DOCX exceeds the 128 MiB conversion input limit",
        ));
    }
    let bytes = read_limited(file, MAX_INPUT).map_err(|e| TossError::from_io(path, e))?;
    validate_package(&bytes)?;
    let pdf = docxide_pdf::convert_docx_bytes_to_pdf_bytes(&bytes)
        .map_err(|e| TossError::other(format!("DOCX conversion failed: {e}")))?;
    if pdf.len() as u64 > MAX_OUTPUT {
        return Err(TossError::other("Converted PDF exceeds the 128 MiB limit"));
    }
    std::io::stdout()
        .lock()
        .write_all(&pdf)
        .map_err(|e| TossError::from_io(path, e))
}

// Check every part, including charts/embedded fonts not read by the terminal
// loader. Stream to EOF for real byte/CRC accounting before the converter reads
// the same immutable bytes. Never extract member paths to disk.
fn validate_package(bytes: &[u8]) -> Result<(), TossError> {
    let invalid = |e| TossError::other(format!("Invalid DOCX conversion package: {e}"));
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(invalid)?;
    if archive.len() > 10_000 {
        return Err(TossError::other(
            "DOCX conversion exceeds 10000 package parts",
        ));
    }
    let mut total = 0u64;
    let mut names = std::collections::HashSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(invalid)?;
        let name = doxx::document::package::sanitize_zip_path(entry.name())
            .map_err(|e| TossError::other(e.to_string()))?;
        if !names.insert(name.to_ascii_lowercase()) {
            return Err(TossError::other(
                "DOCX conversion package has duplicate part names",
            ));
        }
        let limit = if name.eq_ignore_ascii_case("word/document.xml") {
            16 * 1024 * 1024
        } else if name.ends_with(".xml") || name.ends_with(".rels") {
            2 * 1024 * 1024
        } else {
            20 * 1024 * 1024
        };
        let remaining = (64 * 1024 * 1024u64).saturating_sub(total);
        let count = std::io::copy(
            &mut Read::by_ref(&mut entry).take(limit.min(remaining) + 1),
            &mut std::io::sink(),
        )
        .map_err(|e| TossError::other(format!("DOCX part could not be read: {e}")))?;
        if count > limit || count > remaining {
            return Err(TossError::other(
                "DOCX conversion package exceeds the part/64 MiB expanded-byte budget",
            ));
        }
        total += count;
    }
    if !names.contains("word/document.xml") {
        return Err(TossError::other(
            "DOCX conversion requires word/document.xml",
        ));
    }
    Ok(())
}

fn read_limited(reader: impl Read, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(std::io::Error::other("conversion byte limit exceeded"));
    }
    Ok(bytes)
}

pub fn convert(path: &Path) -> Result<Vec<u8>, TossError> {
    let executable = std::env::current_exe().map_err(|e| TossError::from_io(path, e))?;
    let mut command = Command::new(executable);
    command
        .arg("--internal-docx-pdf")
        .arg(path)
        .env("DOCXIDE_NO_FONT_CACHE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let child = command.spawn().map_err(|e| TossError::from_io(path, e))?;
    collect(child, path, TIMEOUT)
}

fn collect(
    mut child: std::process::Child,
    path: &Path,
    timeout: Duration,
) -> Result<Vec<u8>, TossError> {
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    // Drain concurrently so a full pipe cannot deadlock the child. The stdout
    // reader stops on overflow, closing the pipe and making the child fail.
    let output = std::thread::spawn(move || read_limited(stdout, MAX_OUTPUT));
    let errors = std::thread::spawn(move || {
        let mut head = Vec::new();
        let mut stream = stderr;
        let _ = Read::by_ref(&mut stream).take(8192).read_to_end(&mut head);
        let _ = std::io::copy(&mut stream, &mut std::io::sink());
        head
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(25)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(match other {
                    Err(e) => TossError::from_io(path, e),
                    _ => TossError::other(format!(
                        "DOCX conversion timed out after {} seconds",
                        timeout.as_secs_f32()
                    )),
                });
            }
        }
    };
    let bytes = output
        .join()
        .map_err(|_| TossError::other("PDF output reader failed"))?;
    let diagnostics = errors.join().unwrap_or_default();
    let status = status?;
    if !status.success() {
        return Err(TossError::other(format!(
            "DOCX conversion failed: {}",
            String::from_utf8_lossy(&diagnostics).trim()
        )));
    }
    let bytes = bytes.map_err(|e| TossError::from_io(path, e))?;
    if !bytes.starts_with(b"%PDF-") {
        return Err(TossError::other("DOCX converter returned no PDF"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converter_child_fixture() {
        if std::env::var_os("TOSS_TEST_CONVERTER_SLEEP").is_some() {
            std::thread::sleep(Duration::from_secs(5));
        }
    }

    #[test]
    fn timeout_kills_and_reaps_the_converter() {
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backend::word_pdf::tests::converter_child_fixture",
            ])
            .env("TOSS_TEST_CONVERTER_SLEEP", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        let error =
            collect(child, Path::new("timeout.docx"), Duration::from_millis(100)).unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    fn package(parts: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = doxx::zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, bytes) in parts {
            writer
                .start_file(
                    *name,
                    doxx::zip::write::FileOptions::default()
                        .compression_method(doxx::zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn auxiliary_parts_cannot_bypass_expansion_or_path_checks() {
        let body = b"<document/>";
        assert!(validate_package(&package(&[("word/document.xml", body)])).is_ok());
        let oversized = vec![b'x'; 2 * 1024 * 1024 + 1];
        let error = validate_package(&package(&[
            ("word/document.xml", body),
            ("word/charts/chart1.xml", &oversized),
        ]))
        .unwrap_err();
        assert!(error.to_string().contains("budget"));
        assert!(
            validate_package(&package(&[
                ("word/document.xml", body),
                ("../../outside.xml", b"bad"),
            ]))
            .is_err()
        );
        assert!(
            validate_package(&package(&[
                ("word/document.xml", body),
                ("word/DOCUMENT.xml", body),
            ]))
            .is_err()
        );
    }

    #[test]
    fn output_limit_and_invalid_package_are_rejected() {
        assert_eq!(read_limited(&b"abc"[..], 3).unwrap(), b"abc");
        assert!(read_limited(&b"abcd"[..], 3).is_err());
        assert!(validate_package(b"not a zip").is_err());
    }
}
