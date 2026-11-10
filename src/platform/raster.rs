//! What every image backend shares: the decoded-pixel contract, the decode
//! budget, the console text beside the window, and the sources a window can
//! browse (a folder of images, or the pages of a PDF).
//!
//! None of this knows how a platform decodes or draws. The Windows backend
//! (WIC + GDI) and the portable backend (bundled codecs + winit/softbuffer)
//! both hand their pixels over in this one shape and obey these limits, so
//! the two cannot drift apart on what Toss accepts or what it says.

use std::path::{Path, PathBuf};

use crate::core::error::TossError;
use crate::core::{info, log};
use crate::platform::image::nav;

/// An image in memory: 32 bits per pixel, top-down, no row padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decoded {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// `width * height * 4` bytes, ordered B, G, R, A, alpha premultiplied.
    pub(crate) pixels: Vec<u8>,
}

/// The longest side any viewer accepts.
pub(crate) const MAX_DIMENSION: u32 = 32_768;
/// The most pixels any viewer accepts.
pub(crate) const MAX_PIXELS: u64 = 64 * 1024 * 1024;
/// The largest decoded buffer any viewer allocates.
pub(crate) const MAX_PIXEL_BYTES: u64 = 256 * 1024 * 1024;

/// Check `width × height` against the budget before anything is allocated,
/// and answer with the row stride and the buffer length.
pub(crate) fn pixel_layout(
    path: &Path,
    width: u32,
    height: u32,
) -> Result<(u32, usize), TossError> {
    if width == 0 || height == 0 {
        return Err(corrupt(path, "the image reports no pixels"));
    }
    let pixels = u64::from(width) * u64::from(height);
    if width > MAX_DIMENSION || height > MAX_DIMENSION || pixels > MAX_PIXELS {
        return Err(TossError::other(format!(
            "{}: image exceeds the native viewer limit (32768 pixels per side, 64 Mi pixels, 256 MiB decoded)",
            path.display()
        )));
    }
    let bytes = pixels * 4;
    if bytes > MAX_PIXEL_BYTES {
        return Err(TossError::other("image exceeds the decoded byte limit"));
    }
    Ok((width * 4, bytes as usize))
}

/// A zeroed buffer of `length` bytes, or an error instead of an abort when
/// the memory is not there.
pub(crate) fn pixel_buffer(length: usize) -> Result<Vec<u8>, TossError> {
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(length)
        .map_err(|_| TossError::other("not enough memory for the decoded image"))?;
    pixels.resize(length, 0);
    Ok(pixels)
}

/// The contents of `path` will not decode (§24: exit 6), in the words the
/// reason was found in.
pub(crate) fn corrupt(path: &Path, context: &str) -> TossError {
    TossError::CorruptInput {
        path: path.to_path_buf(),
        context: context.to_owned(),
    }
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
pub(crate) fn summary(path: &Path, image: &Decoded, bytes: Option<u64>) -> String {
    summary_with(path, image, bytes, &WINDOW_CONTROLS)
}

/// The keys every viewer *window* answers to.
pub(crate) const WINDOW_CONTROLS: [(&str, &str); 6] = [
    ("Wheel", "Zoom around cursor"),
    ("Left drag", "Pan"),
    ("Left/Right", "Previous / next image"),
    ("F", "Restore automatic fit"),
    ("1", "Actual size (100%)"),
    ("Esc", "Close"),
];

/// [`summary`] for a viewer that answers to a different set of keys.
pub(crate) fn summary_with(
    path: &Path,
    image: &Decoded,
    bytes: Option<u64>,
    controls: &[(&str, &str)],
) -> String {
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
    for (key, action) in controls {
        text.push_str(&format!("  {key:<12}{action}\n"));
    }

    text
}

/// The line printed when browsing lands on another picture.
///
/// The same four facts as [`summary`]'s header, one line, appended — because
/// the console is append-only by choice (§12): no rewriting, no clearing, no
/// status line.
pub(crate) fn announce(path: &Path, image: &Decoded, bytes: Option<u64>) -> String {
    let facts = facts(path, image, bytes);

    format!(
        "→ {} ({}, {}, {})",
        facts.file, facts.format, facts.dimensions, facts.size
    )
}

/// The PDF viewer's console header: the keys a window answers to, or the
/// smaller set the terminal fallback does (`window == false`).
#[cfg(feature = "pdf")]
pub(crate) fn pdf_summary(path: &Path, pages: usize, window: bool) -> String {
    let controls = if window {
        "←/→ or PgUp/PgDn: page | wheel: zoom | drag: pan | F: auto-fit | 1: 144 dpi | Esc: close"
    } else {
        "←/→ or PgUp/PgDn: page | Esc or q: close"
    };
    format!("{} — PDF, {pages} pages\n{controls}", path.display())
}

/// The four fields, computed once so the header and every step line cannot
/// drift apart.
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
/// format from it is reporting a decision, not repeating one.
pub(crate) fn format_name(path: &Path) -> &'static str {
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

/// What a viewer window is walking through. PDF navigation is bounded and
/// never silently skips a failed page; image-directory browsing keeps its
/// rule of skipping pictures that will not open.
pub(crate) enum Source {
    Images(Vec<PathBuf>),
    #[cfg(feature = "pdf")]
    Pdf(crate::backend::pdf::Document),
}

/// How a backend turns a path into pixels; the source decides *which* path.
pub(crate) type DecodeFn = fn(&Path) -> Result<Decoded, TossError>;

impl Source {
    /// The window title for entry `index`.
    pub(crate) fn title(&self, index: usize) -> String {
        match self {
            Self::Images(images) => images
                .get(index)
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Toss".into()),
            #[cfg(feature = "pdf")]
            Self::Pdf(document) => format!(
                "{} — Page {} / {} — Toss",
                document
                    .path()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                index + 1,
                document.len()
            ),
        }
    }

    /// The entry beside `index` in `forward`'s direction, decoded.
    pub(crate) fn adjacent(
        &self,
        index: usize,
        forward: bool,
        decode: DecodeFn,
    ) -> Result<Option<(usize, Decoded)>, TossError> {
        match self {
            Self::Images(images) => {
                for candidate in nav::attempts(index, images.len(), forward) {
                    if let Ok(image) = decode(&images[candidate]) {
                        return Ok(Some((candidate, image)));
                    }
                }
                Ok(None)
            }
            #[cfg(feature = "pdf")]
            Self::Pdf(document) => {
                let _ = decode;
                let candidate = if forward {
                    index.checked_add(1)
                } else {
                    index.checked_sub(1)
                };
                let Some(candidate) = candidate.filter(|&index| index < document.len()) else {
                    return Ok(None);
                };
                let image = document.render(candidate)?;
                Ok(Some((
                    candidate,
                    Decoded {
                        width: image.width,
                        height: image.height,
                        pixels: image.pixels,
                    },
                )))
            }
        }
    }

    /// Tell the console which entry is on screen now.
    pub(crate) fn announce(&self, index: usize, image: &Decoded) {
        match self {
            Self::Images(images) => {
                let path = &images[index];
                let bytes = std::fs::metadata(path).ok().map(|meta| meta.len());
                log::out(&announce(path, image, bytes));
            }
            #[cfg(feature = "pdf")]
            Self::Pdf(document) => log::out(&format!("→ Page {} / {}", index + 1, document.len())),
        }
    }

    /// Pages open at their own aspect; photos open at the half-screen cap.
    pub(crate) fn preserve_initial_aspect(&self) -> bool {
        !matches!(self, Self::Images(_))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Decoded, announce, format_name, pixel_buffer, pixel_layout, summary};

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
    fn pixel_budget_rejects_before_allocation_and_preserves_common_sizes() {
        let path = Path::new("budget.png");
        for (width, height) in [(3840, 2160), (8000, 6000), (8192, 8192), (32768, 1)] {
            assert_eq!(
                pixel_layout(path, width, height).unwrap(),
                (width * 4, width as usize * height as usize * 4)
            );
        }
        for (width, height) in [
            (0, 1),
            (1, 0),
            (32769, 1),
            (8192, 8193),
            (u32::MAX, u32::MAX),
        ] {
            assert!(pixel_layout(path, width, height).is_err());
        }
        assert!(
            pixel_buffer(usize::MAX).is_err(),
            "capacity overflow must be recoverable"
        );
        assert_eq!(pixel_buffer(4).unwrap(), [0; 4]);
    }

    #[test]
    fn the_summary_reports_what_toss_already_holds_and_nothing_more() {
        let text = summary(Path::new("photos/panel.png"), &picture(320, 200), Some(747));

        assert!(text.contains("Toss — Image Viewer"), "{text}");
        assert!(text.contains("panel.png"), "{text}");
        assert!(text.contains("PNG"), "{text}");
        assert!(text.contains("320 × 200"), "{text}");
        assert!(text.contains("747 bytes"), "{text}");
    }

    #[test]
    fn the_controls_promise_only_keys_that_work() {
        let text = summary(Path::new("photo.png"), &picture(1, 1), None);
        let controls_start = text.find("Controls").expect("has controls section");
        let controls = &text[controls_start..];

        for key in ["Wheel", "Left drag", "Left/Right", "F", "1", "Esc"] {
            let key_pattern = format!("  {key}");
            let at = controls
                .find(&key_pattern)
                .unwrap_or_else(|| panic!("{key_pattern:?} is not printed in controls"));
            let after = &controls[at + key_pattern.len()..];
            assert!(
                after.starts_with("  "),
                "{key:?} is not padded before its action: {after:?}"
            );
        }
    }

    #[test]
    fn a_step_announces_the_picture_it_landed_on() {
        let line = announce(Path::new("photos/odd.bmp"), &picture(9, 4), Some(166));

        assert_eq!(line, "→ odd.bmp (BMP, 9 × 4, 166 bytes)");
    }

    #[test]
    fn a_format_is_named_from_the_extension_that_got_it_here() {
        assert_eq!(format_name(Path::new("photo.png")), "PNG");
        assert_eq!(format_name(Path::new("PHOTO.PNG")), "PNG");
        assert_eq!(format_name(Path::new("scan.jpeg")), "JPEG");
        assert_eq!(format_name(Path::new("scan.jpg")), "JPEG");
        assert_eq!(format_name(Path::new("screenshot")), "(none)");
        assert_eq!(format_name(Path::new("mystery.xyz")), "(none)");
    }
}
