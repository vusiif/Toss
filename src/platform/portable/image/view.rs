//! The viewer's interaction state, with no window in it.
//!
//! This is the portable twin of the Windows window procedure's arms: the
//! same answers to the same inputs — wheel zoom around the cursor, drag to
//! pan, arrows and PgUp/PgDn to browse, F to fit, 1 for 100% — all built on
//! the Toss-owned rules in `platform::image` (`zoom`, `pan`, `nav`). Keeping
//! it free of winit types is what lets every rule be tested headless.

use crate::core::log;
use crate::platform::image::{pan, zoom};
use crate::platform::raster::{DecodeFn, Decoded, Source};

use super::paint::{Pyramid, drawn_size, paint};

/// What a key asks the window to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Key {
    Next,
    Previous,
    Fit,
    ActualSize,
    Close,
}

/// What the window has to do after an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Effect {
    Nothing,
    Redraw,
    /// The picture changed: redraw and retitle.
    Retitle,
    Close,
}

#[derive(Clone, Copy)]
struct Drag {
    at: (i32, i32),
    origin: (i32, i32),
}

pub(super) struct View {
    source: Source,
    decode: DecodeFn,
    index: usize,
    image: Decoded,
    pyramid: Pyramid,
    zoom: f64,
    auto_fit: bool,
    origin: (i32, i32),
    drag: Option<Drag>,
    client: (i32, i32),
}

impl View {
    pub(super) fn new(source: Source, decode: DecodeFn, index: usize, image: Decoded) -> Self {
        Self {
            source,
            decode,
            index,
            image,
            pyramid: Pyramid::default(),
            zoom: 1.0,
            auto_fit: true,
            origin: (0, 0),
            drag: None,
            client: (0, 0),
        }
    }

    pub(super) fn title(&self) -> String {
        self.source.title(self.index)
    }

    pub(super) fn image(&self) -> &Decoded {
        &self.image
    }

    pub(super) fn image_size(&self) -> (u32, u32) {
        (self.image.width, self.image.height)
    }

    pub(super) fn preserve_initial_aspect(&self) -> bool {
        self.source.preserve_initial_aspect()
    }

    pub(super) fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Automatic fit follows the viewport; a manual zoom keeps its scale.
    pub(super) fn resize(&mut self, client: (i32, i32)) {
        self.client = client;
        if client.0 <= 0 || client.1 <= 0 {
            return;
        }
        (self.zoom, self.origin) = zoom::resized(
            self.image_size(),
            client,
            self.zoom,
            self.origin,
            self.auto_fit,
        );
        self.drag = None;
    }

    /// `delta` in Windows wheel units (120 per notch), positive away from
    /// the user (zoom in), anchored at `cursor`.
    pub(super) fn wheel(&mut self, delta: i16, cursor: (i32, i32)) -> Effect {
        let fit = zoom::fit_factor(self.image_size(), self.client);
        let next = zoom::wheel_factor(self.zoom, delta, fit);
        if next == self.zoom {
            return Effect::Nothing;
        }
        let target = zoom::anchor_origin(self.origin, cursor, self.zoom, next);
        self.zoom = next;
        self.auto_fit = false;
        self.drag = None;
        self.origin = pan::positioned(target, drawn_size(&self.image, next), self.client);
        Effect::Redraw
    }

    pub(super) fn press(&mut self, cursor: (i32, i32)) {
        self.drag = Some(Drag {
            at: cursor,
            origin: self.origin,
        });
    }

    pub(super) fn release(&mut self) {
        self.drag = None;
    }

    pub(super) fn moved(&mut self, cursor: (i32, i32)) -> Effect {
        let Some(drag) = self.drag else {
            return Effect::Nothing;
        };
        let delta = (cursor.0 - drag.at.0, cursor.1 - drag.at.1);
        let content = drawn_size(&self.image, self.zoom);
        let next = pan::positioned(
            pan::dragged(drag.origin, delta, content, self.client),
            content,
            self.client,
        );
        if next == self.origin {
            return Effect::Nothing;
        }
        self.origin = next;
        Effect::Redraw
    }

    pub(super) fn key(&mut self, key: Key) -> Effect {
        match key {
            Key::Next => self.browse(true),
            Key::Previous => self.browse(false),
            Key::Close => Effect::Close,
            Key::Fit => {
                self.auto_fit = true;
                self.resize(self.client);
                Effect::Redraw
            }
            Key::ActualSize => {
                self.auto_fit = false;
                if self.zoom == 1.0 && self.origin == (0, 0) && self.drag.is_none() {
                    return Effect::Nothing;
                }
                self.zoom = 1.0;
                self.origin = pan::positioned((0, 0), drawn_size(&self.image, 1.0), self.client);
                self.drag = None;
                Effect::Redraw
            }
        }
    }

    /// Pixels, index, zoom and title change together, and only on success.
    fn browse(&mut self, forward: bool) -> Effect {
        let (index, image) = match self.source.adjacent(self.index, forward, self.decode) {
            Ok(Some(next)) => next,
            Ok(None) => return Effect::Nothing,
            Err(err) => {
                log::error(format!("could not open the adjacent page: {err}"));
                return Effect::Nothing;
            }
        };
        self.index = index;
        self.image = image;
        self.pyramid.clear();
        self.zoom = 1.0;
        self.origin = (0, 0);
        self.drag = None;
        self.auto_fit = true;
        self.resize(self.client);
        self.source.announce(self.index, &self.image);
        Effect::Retitle
    }

    /// Draw the current state into a `size` framebuffer.
    pub(super) fn paint(&mut self, frame: &mut [u32], size: (u32, u32)) {
        paint(
            frame,
            size,
            &self.image,
            &mut self.pyramid,
            self.zoom,
            self.origin,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Effect, Key, View};
    use crate::platform::portable::image::decode::decode;
    use crate::platform::raster::Source;

    fn corpus(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/image")
            .join(name)
    }

    fn view(names: &[&str]) -> View {
        let images: Vec<_> = names.iter().map(|name| corpus(name)).collect();
        let first = decode(&images[0]).unwrap();
        let mut view = View::new(Source::Images(images), decode, 0, first);
        view.resize((160, 100));
        view
    }

    #[test]
    fn opens_fitted_and_wheel_zoom_switches_to_manual() {
        let mut view = view(&["panel.png"]);
        assert_eq!(view.zoom, 0.5, "320×200 fits 160×100 at half size");
        assert_eq!(view.wheel(120, (80, 50)), Effect::Redraw);
        assert!(view.zoom > 0.5 && !view.auto_fit);
        view.resize((320, 200));
        assert!(view.zoom < 1.0, "a manual zoom survives a resize");
        assert_eq!(view.key(Key::Fit), Effect::Redraw);
        assert_eq!(view.zoom, 1.0);
    }

    #[test]
    fn dragging_pans_only_while_the_button_is_held() {
        let mut view = view(&["panel.png"]);
        view.key(Key::ActualSize);
        assert_eq!(view.moved((10, 10)), Effect::Nothing);
        view.press((50, 50));
        assert_eq!(view.moved((20, 30)), Effect::Redraw);
        assert_eq!(view.origin, (-30, -20));
        view.release();
        assert_eq!(view.moved((0, 0)), Effect::Nothing);
    }

    #[test]
    fn browsing_skips_unreadable_entries_wraps_and_resets_the_view() {
        let mut view = view(&["odd.png", "truncated.png", "panel.png"]);
        view.wheel(240, (0, 0));
        assert_eq!(view.key(Key::Next), Effect::Retitle);
        assert_eq!(view.index, 2, "the broken picture is skipped");
        assert!(view.auto_fit);
        assert_eq!(view.title(), "panel.png");
        view.key(Key::Next);
        assert_eq!(view.index, 0, "browsing wraps");
        assert_eq!(view.key(Key::Close), Effect::Close);
    }

    #[test]
    fn a_framebuffer_of_any_size_paints_without_panicking() {
        let mut view = view(&["panel.png"]);
        for size in [(0, 0), (1, 1), (7, 3), (640, 480)] {
            view.resize((size.0 as i32, size.1 as i32));
            let mut frame = vec![0; (size.0 * size.1) as usize];
            view.paint(&mut frame, size);
        }
    }
}
