//! The Windows implementation of the image-viewing capability.

pub(crate) mod decode;
mod viewer;

use windows::core::Error;

use crate::core::error::TossError;
use crate::core::log;
use crate::platform::image::ViewRequest;
use crate::platform::raster;

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
    log::out(&raster::summary(path, &image, bytes));

    viewer::run(request, image)
}

#[cfg(feature = "pdf")]
pub fn view_pdf(document: crate::backend::pdf::Document) -> Result<(), TossError> {
    let image = document.render(0)?;
    log::out(&raster::pdf_summary(document.path(), document.len(), true));
    viewer::run_source(
        raster::Source::Pdf(document),
        0,
        raster::Decoded {
            width: image.width,
            height: image.height,
            pixels: image.pixels,
        },
    )
}

/// Turn a `windows-rs` failure into something Toss can say (§23).
///
/// The HRESULT never leaves this module: a user reads a sentence, and the
/// platform layer is the only place that knows it came from a platform.
fn failed(step: &str, err: Error) -> TossError {
    TossError::other(format!("image viewer could not {step}: {err}"))
}
