//! Hayro page rasterization. No window, temporary image files or external tools.
//!
//! Bounds cover input bytes and output pixels, not Hayro's internal allocations
//! or execution time. A fresh render cache per page avoids retained font/scene
//! caches growing with every visited page.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::core::error::TossError;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};

const MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const MAX_DIMENSION: u32 = 8192;
const MAX_PIXELS: u64 = 16_777_216;
const PAGE_SCALE: f32 = 2.0; // 144 dpi; window zoom scales this raster.

// Linux currently exercises the rasterizer in tests but has no window backend.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Document {
    pdf: Pdf,
    path: PathBuf,
}

#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug)]
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    /// Opaque BGRA, ready for the existing native viewer.
    pub pixels: Vec<u8>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Document {
    pub fn open(path: &Path) -> Result<Self, TossError> {
        let file = File::open(path).map_err(|err| TossError::from_io(path, err))?;
        if file
            .metadata()
            .map_err(|err| TossError::from_io(path, err))?
            .len()
            > MAX_INPUT_BYTES
        {
            return Err(TossError::other("PDF exceeds the 128 MiB input limit"));
        }
        let mut bytes = Vec::new();
        file.take(MAX_INPUT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| TossError::from_io(path, err))?;
        if bytes.len() as u64 > MAX_INPUT_BYTES {
            return Err(TossError::other("PDF exceeds the 128 MiB input limit"));
        }
        Self::from_bytes(bytes, path)
    }

    pub(crate) fn from_bytes(bytes: Vec<u8>, path: &Path) -> Result<Self, TossError> {
        if bytes.len() as u64 > MAX_INPUT_BYTES {
            return Err(TossError::other("PDF exceeds the 128 MiB input limit"));
        }
        let pdf = Pdf::new(Arc::new(bytes)).map_err(|err| match err {
            LoadPdfError::Decryption(_) => TossError::other(
                "password-protected PDF could not be opened; password entry is not supported",
            ),
            LoadPdfError::Invalid => TossError::CorruptInput {
                path: path.to_path_buf(),
                context: "invalid PDF".into(),
            },
        })?;
        if pdf.pages().is_empty() {
            return Err(TossError::CorruptInput {
                path: path.to_path_buf(),
                context: "PDF has no readable pages".into(),
            });
        }
        Ok(Self {
            pdf,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn len(&self) -> usize {
        self.pdf.pages().len()
    }

    pub fn render(&self, index: usize) -> Result<PageImage, TossError> {
        let page = self
            .pdf
            .pages()
            .get(index)
            .ok_or_else(|| TossError::other("PDF page index is out of range"))?;
        // Use Hayro's actual crop/rotation-aware dimensions, not MediaBox.
        let (width, height) = page.render_dimensions();
        let (expected_width, expected_height) = raster_dimensions(width, height)?;
        let pixmap = hayro::render(
            page,
            &RenderCache::new(),
            &InterpreterSettings::default(),
            &RenderSettings::default(),
            &PixmapSettings {
                x_scale: PAGE_SCALE,
                y_scale: PAGE_SCALE,
                bg_color: WHITE,
            },
        );
        let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
        if (width, height) != (expected_width, expected_height) {
            return Err(TossError::other(
                "PDF renderer returned unexpected dimensions",
            ));
        }
        let pixels = into_bgra(pixmap);
        Ok(PageImage {
            width,
            height,
            pixels,
        })
    }
}

/// Transfer Hayro's allocation and convert RGBA to the viewer's BGRA in place.
/// Premultiplication is preserved; no second full-page buffer is allocated.
fn into_bgra(pixmap: hayro::vello_cpu::Pixmap) -> Vec<u8> {
    let mut pixels =
        pixmap.take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::AlphaPremultiplied);
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    pixels
}

#[cfg(test)]
#[path = "../../tests/support/pdf.rs"]
pub(crate) mod fixtures;

fn raster_dimensions(width: f32, height: f32) -> Result<(u32, u32), TossError> {
    let (width, height) = (width * PAGE_SCALE, height * PAGE_SCALE);
    if !width.is_finite()
        || !height.is_finite()
        || width < 1.0
        || height < 1.0
        || width > MAX_DIMENSION as f32
        || height > MAX_DIMENSION as f32
        || (width as u64) * (height as u64) > MAX_PIXELS
    {
        return Err(TossError::other(
            "PDF page exceeds the rendering limit (8192 pixels per side, 16 megapixels at 144 dpi)",
        ));
    }
    // Match the renderer's truncating f32 -> u16 conversion exactly.
    Ok((width as u32, height as u32))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::fixtures::{Page, document};
    use super::*;

    #[test]
    fn bgra_conversion_reuses_the_raster_allocation_and_preserves_alpha() {
        let mut pixmap = hayro::vello_cpu::Pixmap::new(2, 1);
        pixmap
            .data_as_u8_slice_mut()
            .copy_from_slice(&[10, 20, 30, 255, 4, 5, 6, 128]);
        let allocation = pixmap.data_as_u8_slice().as_ptr();
        let pixels = into_bgra(pixmap);
        assert_eq!(pixels.as_ptr(), allocation);
        assert_eq!(pixels, [30, 20, 10, 255, 6, 5, 4, 128]);
    }

    pub(crate) fn sample(pages: &[Page<'_>]) -> Document {
        Document::from_bytes(document(pages), Path::new("synthetic.pdf")).unwrap()
    }

    #[test]
    fn pixels_follow_page_identity_and_repeated_rendering() {
        let document = sample(&[
            Page {
                width: 40,
                height: 20,
                options: "",
                content: "1 0 0 rg 0 0 40 20 re f",
            },
            Page {
                width: 40,
                height: 20,
                options: "",
                content: "0 0 1 rg 0 0 40 20 re f",
            },
        ]);
        assert_eq!(document.len(), 2);
        let first = document.render(0).unwrap();
        assert_eq!((first.width, first.height), (80, 40));
        assert_eq!(&first.pixels[..4], &[0, 0, 255, 255]);
        let second = document.render(1).unwrap();
        assert_eq!(&second.pixels[..4], &[255, 0, 0, 255]);
        assert_eq!(document.render(0).unwrap().pixels, first.pixels);
        assert!(document.render(2).is_err());
    }

    #[test]
    fn crop_and_rotation_use_the_renderers_dimensions() {
        let document = sample(&[Page {
            width: 200,
            height: 100,
            options: "/CropBox [20 10 140 90] /Rotate 90",
            content: "0 1 0 rg 0 0 200 100 re f",
        }]);
        let image = document.render(0).unwrap();
        assert_eq!((image.width, image.height), (160, 240));
        assert_eq!(&image.pixels[..4], &[0, 255, 0, 255]);
    }

    #[test]
    fn standard_font_text_is_visible_on_white_paper() {
        let document = sample(&[Page {
            width: 200,
            height: 100,
            options: "",
            content: "BT /F1 20 Tf 10 50 Td (Toss PDF) Tj ET",
        }]);
        let image = document.render(0).unwrap();
        let dark = image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] < 100 && p[1] < 100 && p[2] < 100)
            .count();
        assert!(dark > 100, "standard-font text must not silently vanish");
        assert_eq!(&image.pixels[..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn transparency_and_inline_image_render_correct_channels() {
        let document = sample(&[Page {
            width: 40,
            height: 20,
            options: "",
            content: "q /GS1 gs 1 0 0 rg 0 0 20 20 re f Q q 20 0 0 20 20 0 cm BI /W 1 /H 1 /CS /RGB /BPC 8 /F /AHx ID 0000FF> EI Q",
        }]);
        let image = document.render(0).unwrap();
        let at = |x: usize| &image.pixels[(10 * image.width as usize + x) * 4..][..4];
        assert!(at(10)[0] > 100 && at(10)[0] < 160 && at(10)[2] == 255);
        assert_eq!(at(60), &[255, 0, 0, 255]);
    }

    #[test]
    fn later_oversized_page_does_not_prevent_opening_the_first_page() {
        let document = sample(&[
            Page {
                width: 40,
                height: 20,
                options: "",
                content: "",
            },
            Page {
                width: 5000,
                height: 5000,
                options: "",
                content: "",
            },
        ]);
        assert!(document.render(0).is_ok());
        assert!(
            document
                .render(1)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
        assert!(document.render(0).is_ok());
    }

    #[test]
    fn malformed_empty_and_resource_boundaries_are_explicit() {
        assert!(matches!(
            Document::from_bytes(b"garbage".to_vec(), Path::new("bad.pdf")),
            Err(TossError::CorruptInput { .. })
        ));
        assert!(Document::from_bytes(document(&[]), Path::new("empty.pdf")).is_err());
        assert_eq!(raster_dimensions(2048.0, 2048.0).unwrap(), (4096, 4096));
        for (w, h) in [
            (f32::NAN, 1.0),
            (1.0, f32::INFINITY),
            (-1.0, 1.0),
            (0.0, 1.0),
            (4097.0, 1.0),
            (2048.5, 2048.5),
        ] {
            assert!(raster_dimensions(w, h).is_err(), "{w}x{h}");
        }
    }
}
