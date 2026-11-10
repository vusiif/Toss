//! CLI failures must finish without opening a window or modifying the input.
//! Every command runs with the display environment removed, so a developer
//! machine with X11 or Wayland never turns a failing case into a window.

use std::path::PathBuf;
use std::process::Command;

#[cfg(feature = "pdf")]
#[path = "support/pdf.rs"]
mod fixtures;

fn headless() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_toss"));
    for name in [
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "WAYLAND_SOCKET",
        "XDG_RUNTIME_DIR",
    ] {
        command.env_remove(name);
    }
    command
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("toss-pdf-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn missing_pdf_has_the_normal_missing_input_exit() {
    let scratch = Scratch::new("missing");
    let output = headless()
        .arg(scratch.0.join("missing.pdf"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
}

#[cfg(feature = "pdf")]
#[test]
fn corrupt_and_oversized_first_pages_fail_before_the_window_exists() {
    let scratch = Scratch::new("bad");
    let large = fixtures::document(&[fixtures::Page {
        width: 5000,
        height: 5000,
        options: "",
        content: "",
    }]);
    for (name, data, code) in [
        ("bad.pdf", b"not a pdf".to_vec(), 6),
        ("large.pdf", large.clone(), 1),
        ("large.dat", large, 1),
    ] {
        let path = scratch.0.join(name);
        std::fs::write(&path, &data).unwrap();
        for explicit in [false, true] {
            let mut command = headless();
            if explicit {
                command.arg("view");
            }
            let output = command.arg(&path).output().unwrap();
            assert_eq!(output.status.code(), Some(code), "{name}: {:?}", output);
            assert!(output.stdout.is_empty());
            assert!(!output.stderr.is_empty());
            assert_eq!(std::fs::read(&path).unwrap(), data);
        }
    }
}

#[cfg(feature = "pdf")]
#[test]
fn input_size_limit_is_checked_before_loading_the_file() {
    let scratch = Scratch::new("input-limit");
    let path = scratch.0.join("large.pdf");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(128 * 1024 * 1024 + 1).unwrap();
    drop(file);
    let output = headless().arg(&path).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("128 MiB input limit"));
    assert_eq!(
        std::fs::metadata(path).unwrap().len(),
        128 * 1024 * 1024 + 1
    );
}

#[cfg(not(feature = "pdf"))]
#[test]
fn unavailable_pdf_viewing_is_an_explicit_refusal() {
    let scratch = Scratch::new("unavailable");
    let path = scratch.0.join("file.PDF");
    std::fs::write(&path, b"%PDF-1.7").unwrap();
    let output = headless().arg(path).output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("PDF viewing"));
}
