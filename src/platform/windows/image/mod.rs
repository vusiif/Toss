//! The Windows implementation of the image-viewing capability.

mod decode;
mod viewer;

use windows::core::Error;

use crate::core::error::TossError;
use crate::platform::image::ViewRequest;

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

    viewer::run(request, image)
}

/// Turn a `windows-rs` failure into something Toss can say (§23).
///
/// The HRESULT never leaves this module: a user reads a sentence, and the
/// platform layer is the only place that knows it came from a platform.
fn failed(step: &str, err: Error) -> TossError {
    TossError::other(format!("image viewer could not {step}: {err}"))
}
