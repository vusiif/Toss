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

    // Decode first, so that a picture which cannot be opened never gets a
    // window (§3: the window exists only when the operation needs one). The
    // pixels are dropped here for now — M3 keeps them — but a successful
    // decode is what proves the file is real before anything is drawn.
    let _decoded = decode::decode(path)?;

    viewer::run(request)
}

/// Turn a `windows-rs` failure into something Toss can say (§23).
///
/// The HRESULT never leaves this module: a user reads a sentence, and the
/// platform layer is the only place that knows it came from a platform.
fn failed(step: &str, err: Error) -> TossError {
    TossError::other(format!("image viewer could not {step}: {err}"))
}
