//! The Windows implementation of the image-viewing capability.

mod viewer;

use crate::core::error::TossError;
use crate::platform::image::ViewRequest;

/// Show `request` in a native window.
///
/// The request arrives already decided — which images, which one — so nothing
/// here re-derives it (IMAGE_VIEWER.md §4). What this half owns is the window
/// it is shown in.
pub fn view(request: &ViewRequest) -> Result<(), TossError> {
    viewer::run(request)
}
