//! The portable implementation of the image-viewing capability.
//!
//! Decode first (an image that cannot be opened needs no window), decide
//! where it can be shown — a window when a display is reachable, the
//! terminal when it is not — print the console summary with the keys *that*
//! viewer answers to, then show it.

pub(crate) mod decode;
mod paint;
mod terminal;
mod view;
mod window;

use std::path::Path;

use crate::core::error::TossError;
use crate::core::log;
use crate::platform::image::ViewRequest;
use crate::platform::raster::{self, Decoded, Source};

use self::view::View;

/// Where the picture will be shown.
#[derive(Clone, Copy)]
enum Surface {
    Window,
    Terminal,
}

/// Show `request`: decode, describe, display.
pub fn view(request: &ViewRequest) -> Result<(), TossError> {
    let Some(path) = request.current() else {
        return Err(TossError::other("image viewer was given no image to open"));
    };
    let image = decode::decode(path)?;
    let surface = surface()?;
    let bytes = std::fs::metadata(path).ok().map(|meta| meta.len());
    log::out(&describe(path, &image, bytes, surface));
    show(
        surface,
        Source::Images(request.images.clone()),
        request.index,
        image,
    )
}

/// Show page one of `document` in the same viewer, with page navigation.
#[cfg(feature = "pdf")]
pub fn view_pdf(document: crate::backend::pdf::Document) -> Result<(), TossError> {
    let page = document.render(0)?;
    let surface = surface()?;
    log::out(&raster::pdf_summary(
        document.path(),
        document.len(),
        matches!(surface, Surface::Window),
    ));
    let image = Decoded {
        width: page.width,
        height: page.height,
        pixels: page.pixels,
    };
    show(surface, Source::Pdf(document), 0, image)
}

/// A window if any display answers; otherwise the terminal, if there is
/// one; otherwise a sentence saying neither exists (exit 3).
fn surface() -> Result<Surface, TossError> {
    match window::available() {
        Ok(()) => Ok(Surface::Window),
        Err(_) if terminal::available() => Ok(Surface::Terminal),
        Err(window::NoDisplay(reason)) => Err(TossError::not_implemented(format!(
            "image viewing here: no display ({reason}) and no terminal to draw in"
        ))),
    }
}

fn describe(path: &Path, image: &Decoded, bytes: Option<u64>, surface: Surface) -> String {
    match surface {
        Surface::Window => raster::summary(path, image, bytes),
        Surface::Terminal => raster::summary_with(path, image, bytes, &terminal::CONTROLS),
    }
}

fn show(surface: Surface, source: Source, index: usize, image: Decoded) -> Result<(), TossError> {
    let view = View::new(source, decode::decode, index, image);
    match surface {
        Surface::Window => window::run(view),
        Surface::Terminal => terminal::run(view),
    }
}
