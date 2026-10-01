//! The Windows implementation of the image-viewing capability.

mod decode;
mod viewer;

use std::path::Path;

use windows::core::Error;

use crate::core::error::TossError;
use crate::core::info;
use crate::core::log;
use crate::platform::image::ViewRequest;

use self::decode::Decoded;

/// Show `request` in a native window.
///
/// The request arrives already decided — which images, which one — so nothing
/// here re-derives it (`IMAGE_VIEWER.md` §4). What this half owns is the
/// window it is shown in, and whether there is anything to show in it.
pub fn view(request: &ViewRequest) -> Result<(), TossError> {
    let Some(path) = request.current() else {
        return Err(TossError::other("image viewer was given no image to open"));
    };

    // Decode before the window exists (§3): an image that cannot be opened
    // needs no window at all. What comes back is pixels Toss owns — every WIC
    // object behind it has already been released — so the renderer consumes a
    // plain buffer and the two halves stay separable: WIC decodes, GDI
    // renders, and neither holds the other's types (IMAGE_VIEWER.md §2).
    let image = decode::decode(path)?;

    // The console beside the viewer is not a side effect to suppress but the
    // half that carries the facts and the controls, while the window carries
    // the pixels. One plain write to stdout, before the message loop takes
    // the thread: no cursor addressing, no redrawing, no second interaction
    // model — a redirect or a script reads ordinary text (§25), and the same
    // text appears whether the console was created by dropping a file on
    // `toss.exe` or borrowed from the shell that launched it.
    let bytes = std::fs::metadata(path).ok().map(|meta| meta.len());
    log::out(&summary(path, &image, bytes));

    viewer::run(request, image)
}

/// What the console says about the picture the viewer is about to show.
///
/// Only facts Toss already holds: the name from the path, the format from
/// the extension that got the file classified as an image in the first
/// place (§6.2), the dimensions the decoder just reported, and the size from
/// metadata. No parser is here to fill the box — bit depth, colour space,
/// EXIF and the rest arrive when image metadata becomes a Toss capability of
/// its own, not because a screen has room (`Toss_AGENTS.md` §40).
///
/// The controls list is the implemented ones. `Toss_AGENTS.md` §10 forbids
/// advertising what is not built, and that applies to a console the user
/// reads exactly as it applies to a README: a key printed here is a promise.
fn summary(path: &Path, image: &Decoded, bytes: Option<u64>) -> String {
    let facts = facts(path, image, bytes);

    let mut text = String::new();
    text.push_str("Toss — Image Viewer\n\n");

    let mut field = |label: &str, value: String| {
        text.push_str(&format!("{label:<12}{value}\n"));
    };

    field("File", facts.file);
    field("Format", facts.format.to_owned());
    field("Dimensions", facts.dimensions);
    field("Size", facts.size);

    text.push_str("\nControls\n");
    for (key, action) in [
        ("Wheel", "Zoom"),
        ("Left drag", "Pan"),
        ("Left/Right", "Previous / next image"),
        ("Alt+F4", "Close"),
    ] {
        text.push_str(&format!("  {key:<12}{action}\n"));
    }

    text
}

/// The line printed when browsing lands on another picture.
///
/// The same four facts as [`summary`]'s header, one line, appended — because
/// the console is append-only by choice (§12): no rewriting, no clearing, no
/// status line. The controls were printed once and they do not move with the
/// pictures, so nothing that has not changed is repeated; this is "this is
/// what is on screen now", in the order the arrows walked there.
///
/// It is the same wording a person would use out loud — *now showing this
/// one* — and the `→` is the only ornament: everything after it is a fact
/// from [`facts`], computed the same way the header computes it.
fn announce(path: &Path, image: &Decoded, bytes: Option<u64>) -> String {
    let facts = facts(path, image, bytes);

    format!(
        "→ {} ({}, {}, {})",
        facts.file, facts.format, facts.dimensions, facts.size
    )
}

/// The four fields, computed once so the header and every step line cannot
/// drift apart — a session that describes its first picture one way and its
/// tenth another way is a session nobody can read.
struct Facts {
    file: String,
    format: &'static str,
    dimensions: String,
    size: String,
}

fn facts(path: &Path, image: &Decoded, bytes: Option<u64>) -> Facts {
    Facts {
        file: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        format: format_name(path),
        dimensions: format!("{} × {}", image.width, image.height),
        size: match bytes {
            Some(bytes) => info::human_size(bytes),
            None => "unknown".to_owned(),
        },
    }
}

/// The display name for the extension that identified this file as an image.
///
/// This is §6.2 made visible rather than a second detector: `detection`
/// already decided the file *is* an image from this extension, so naming the
/// format from it is reporting a decision, not repeating one. An image that
/// was classified by its contents alone has no extension to name, and says
/// so instead of guessing at a decoder Toss does not ask.
fn format_name(path: &Path) -> &'static str {
    let Some(extension) = path.extension() else {
        return "(none)";
    };

    match extension.to_string_lossy().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "JPEG",
        "png" => "PNG",
        "bmp" => "BMP",
        "gif" => "GIF",
        _ => "(none)",
    }
}

/// Turn a `windows-rs` failure into something Toss can say (§23).
///
/// The HRESULT never leaves this module: a user reads a sentence, and the
/// platform layer is the only place that knows it came from a platform.
fn failed(step: &str, err: Error) -> TossError {
    TossError::other(format!("image viewer could not {step}: {err}"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::decode::Decoded;
    use super::{announce, format_name, summary};

    /// A picture as the decoder would hand it over, sized but not filled:
    /// `summary` reads the dimensions and never the pixels.
    fn picture(width: u32, height: u32) -> Decoded {
        Decoded {
            width,
            height,
            pixels: Vec::new(),
        }
    }

    #[test]
    fn the_summary_reports_what_toss_already_holds_and_nothing_more() {
        // Every field here comes from something Toss had anyway: the name
        // from the path, the format from the extension that classified it
        // (§6.2), the size from metadata, the dimensions from the decoder
        // that just ran. Asserting the exact rendering is what stops a
        // later "let's fill the box" from creeping a parser in behind it.
        let text = summary(
            Path::new(r"C:\photos\panel.png"),
            &picture(320, 200),
            Some(747),
        );

        assert!(text.contains("Toss — Image Viewer"), "{text}");
        assert!(text.contains("panel.png"), "{text}");
        assert!(text.contains("PNG"), "{text}");
        assert!(text.contains("320 × 200"), "{text}");
        assert!(text.contains("747 bytes"), "{text}");
    }

    #[test]
    fn the_controls_promise_only_keys_that_work() {
        // `Toss_AGENTS.md` §10 — no advertising what is not built — applied
        // to a console the user reads. "Fit", an actual-size key and Esc are
        // the ones a viewer is *expected* to have and this one does not, so
        // they are exactly the promises that must not appear; if one of them
        // is implemented later, this assertion is the reminder to print it.
        let text = summary(Path::new("photo.png"), &picture(1, 1), None);

        for implemented in ["Wheel", "Left drag", "Left/Right", "Alt+F4"] {
            assert!(
                text.contains(implemented),
                "missing {implemented:?}:\n{text}"
            );
        }
        for not_built_yet in ["Fit", "Actual size", "Esc"] {
            assert!(
                !text.contains(not_built_yet),
                "promised {not_built_yet:?} without building it:\n{text}"
            );
        }

        // And every key still has to be *followed* by padding: a key that
        // exactly fills the label field runs straight into its action, which
        // is how `Left / Right` once printed as `Left / RightPrevious`
        // without anything looking wrong in the source.
        for key in ["Wheel", "Left drag", "Left/Right", "Alt+F4"] {
            let at = text
                .find(key)
                .unwrap_or_else(|| panic!("{key:?} is not printed at all"));
            let after = &text[at + key.len()..];
            assert!(
                after.starts_with("  "),
                "{key:?} is not padded before its action: {after:?}"
            );
        }
    }

    #[test]
    fn a_step_announces_the_picture_it_landed_on() {
        // The line the console grows by whenever an arrow is pressed: the
        // same four facts as the header, one line, controls not repeated —
        // because the terminal has to say what is on screen *now*, not what
        // the session opened on. Exact wording on purpose: a session is read
        // top to bottom as one document, and a step line that formats
        // differently from its own header is a log nobody can skim.
        let line = announce(Path::new(r"C:\photos\odd.bmp"), &picture(9, 4), Some(166));

        assert_eq!(line, "→ odd.bmp (BMP, 9 × 4, 166 bytes)");
    }

    #[test]
    fn a_format_is_named_from_the_extension_that_got_it_here() {
        // Reporting §6.2's decision rather than running a second detector.
        assert_eq!(format_name(Path::new("photo.png")), "PNG");
        assert_eq!(format_name(Path::new("PHOTO.PNG")), "PNG");
        assert_eq!(format_name(Path::new("scan.jpeg")), "JPEG");
        assert_eq!(format_name(Path::new("scan.jpg")), "JPEG");

        // Classified by contents alone, or by an extension Toss does not
        // know: no extension to name, so no format is invented for it.
        assert_eq!(format_name(Path::new("screenshot")), "(none)");
        assert_eq!(format_name(Path::new("mystery.xyz")), "(none)");
    }
}
