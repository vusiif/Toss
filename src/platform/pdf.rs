//! The PDF window capability, with a portable refusal when unavailable.

use crate::core::error::TossError;

/// Fit the page uniformly inside the available client area. Window decorations
/// are deducted first so an outer-height cap cannot crop a portrait page.
#[cfg_attr(not(feature = "pdf"), allow(dead_code))]
pub(crate) fn initial_client_size(
    page: (u32, u32),
    outer_limit: (i32, i32),
    decorations: (i32, i32),
) -> (i32, i32) {
    let available = (
        (outer_limit.0 - decorations.0).max(1),
        (outer_limit.1 - decorations.1).max(1),
    );
    let width = page.0.max(1) as f64;
    let height = page.1.max(1) as f64;
    let scale = (available.0 as f64 / width)
        .min(available.1 as f64 / height)
        .min(1.0);
    (
        (width * scale).floor().max(1.0) as i32,
        (height * scale).floor().max(1.0) as i32,
    )
}

use std::path::Path;

#[cfg(all(windows, feature = "word-pdf"))]
pub const AVAILABLE: bool = cfg!(feature = "pdf");

#[cfg(all(windows, feature = "pdf"))]
pub fn view(path: &Path) -> Result<(), TossError> {
    let document = crate::backend::pdf::Document::open(path)?;
    super::windows::image::view_pdf(document)
}

#[cfg(all(not(windows), feature = "pdf"))]
pub fn view(path: &Path) -> Result<(), TossError> {
    let document = crate::backend::pdf::Document::open(path)?;
    super::portable::image::view_pdf(document)
}

#[cfg(not(feature = "pdf"))]
pub fn view(_path: &Path) -> Result<(), TossError> {
    Err(TossError::not_implemented("PDF viewing on this build"))
}

#[cfg(all(windows, feature = "word-pdf"))]
pub fn view_bytes(bytes: Vec<u8>, source: &Path) -> Result<(), TossError> {
    let document = crate::backend::pdf::Document::from_bytes(bytes, source)?;
    super::windows::image::view_pdf(document)
}

#[cfg(test)]
mod tests {
    use super::initial_client_size;

    #[test]
    fn portrait_landscape_and_square_pages_keep_their_client_aspect() {
        for page in [(1224, 1584), (1584, 1224), (2000, 2000)] {
            let size = initial_client_size(page, (1280, 720), (16, 39));
            assert!(size.0 + 16 <= 1280 && size.1 + 39 <= 720);
            let scale = size.1 as f64 / page.1 as f64;
            assert!((size.0 as f64 - page.0 as f64 * scale).abs() < 2.0);
            assert_eq!(size.0 < size.1, page.0 < page.1);
        }
    }

    #[test]
    fn small_pages_and_small_screens_have_nonzero_bounded_sizes() {
        assert_eq!(
            initial_client_size((100, 200), (1280, 720), (16, 39)),
            (100, 200)
        );
        let size = initial_client_size((1224, 1584), (400, 300), (16, 39));
        assert!(size.0 > 0 && size.1 > 0 && size.0 + 16 <= 400 && size.1 + 39 <= 300);
        assert_eq!(initial_client_size((0, 0), (0, 0), (16, 39)), (1, 1));
    }
}
