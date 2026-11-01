//! Raster sources for the shared native window. PDF navigation is bounded and
//! never silently skips a failed page; image-directory browsing keeps its rule.

use super::decode::Decoded;
use crate::core::error::TossError;
use crate::core::log;
use crate::platform::image::nav;
use std::path::PathBuf;
use windows::core::HSTRING;

pub(super) enum Source {
    Images(Vec<PathBuf>),
    #[cfg(feature = "pdf")]
    Pdf(crate::backend::pdf::Document),
}

#[cfg(all(test, feature = "pdf"))]
mod tests {
    use super::*;
    use crate::backend::pdf::{fixtures::Page, tests::sample};

    #[test]
    fn image_browsing_still_skips_failed_entries_and_wraps() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/image");
        let source = Source::Images(vec![
            root.join("odd.png"),
            root.join("missing.png"),
            root.join("panel.png"),
        ]);
        let (index, image) = source.adjacent(0, true).unwrap().unwrap();
        assert_eq!(index, 2);
        assert_eq!((image.width, image.height), (320, 200));
        let (index, image) = source.adjacent(index, true).unwrap().unwrap();
        assert_eq!(index, 0);
        assert_eq!((image.width, image.height), (7, 5));
        assert!(!source.preserve_initial_aspect());
    }

    #[test]
    fn pdf_navigation_stops_at_ends_and_keeps_exact_page_identity() {
        let source = Source::Pdf(sample(&[
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
        ]));
        assert!(source.adjacent(0, false).unwrap().is_none());
        assert!(source.adjacent(1, true).unwrap().is_none());
        let (index, blue) = source.adjacent(0, true).unwrap().unwrap();
        assert_eq!(index, 1);
        assert_eq!(&blue.pixels[..4], &[255, 0, 0, 255]);
        assert!(source.title(index).to_string_lossy().contains("Page 2 / 2"));
        let (index, red) = source.adjacent(index, false).unwrap().unwrap();
        assert_eq!(index, 0);
        assert_eq!(&red.pixels[..4], &[0, 0, 255, 255]);
        assert!(source.preserve_initial_aspect());
    }

    #[test]
    fn failed_pdf_page_is_reported_instead_of_skipped() {
        let source = Source::Pdf(sample(&[
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
            Page {
                width: 40,
                height: 20,
                options: "",
                content: "",
            },
        ]));
        assert!(source.adjacent(0, true).is_err());
        assert!(source.title(0).to_string_lossy().contains("Page 1 / 3"));
    }
}

impl Source {
    pub(super) fn title(&self, index: usize) -> HSTRING {
        match self {
            Self::Images(images) => HSTRING::from(
                images
                    .get(index)
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Toss".into()),
            ),
            #[cfg(feature = "pdf")]
            Self::Pdf(document) => HSTRING::from(format!(
                "{} — Page {} / {} — Toss",
                document
                    .path()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                index + 1,
                document.len()
            )),
        }
    }

    pub(super) fn adjacent(
        &self,
        index: usize,
        forward: bool,
    ) -> Result<Option<(usize, Decoded)>, TossError> {
        match self {
            Self::Images(images) => {
                for candidate in nav::attempts(index, images.len(), forward) {
                    if let Ok(image) = super::decode::decode(&images[candidate]) {
                        return Ok(Some((candidate, image)));
                    }
                }
                Ok(None)
            }
            #[cfg(feature = "pdf")]
            Self::Pdf(document) => {
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

    pub(super) fn announce(&self, index: usize, image: &Decoded) {
        match self {
            Self::Images(images) => {
                let path = &images[index];
                let bytes = std::fs::metadata(path).ok().map(|meta| meta.len());
                log::out(&super::announce(path, image, bytes));
            }
            #[cfg(feature = "pdf")]
            Self::Pdf(document) => log::out(&format!("→ Page {} / {}", index + 1, document.len())),
        }
    }

    pub(super) fn preserve_initial_aspect(&self) -> bool {
        !matches!(self, Self::Images(_))
    }
}
