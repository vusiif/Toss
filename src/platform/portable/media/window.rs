//! The video window of the portable player.
//!
//! The same winit + softbuffer pair as the image viewer, on the same shared
//! event loop. The window owns no decoding and no clock: on every turn of
//! the loop it asks the engine which picture is due, puts it on screen
//! letterboxed, and sleeps until the engine says the next one is.
//!
//! Pictures arrive already at the size they are drawn at — the decoder
//! scales while it converts — so painting is a row copy. Only the moments
//! after a resize (and a paused picture in a resized window) are stretched
//! here, crudely, until a picture of the new size arrives.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key as Logical, NamedKey};
use winit::platform::run_on_demand::EventLoopExtRunOnDemand;
use winit::window::{Fullscreen, Window, WindowId};

use crate::core::error::TossError;
use crate::platform::portable::display::{self, NoDisplay};

use super::SEEK_STEP;
use super::engine::{Engine, Frame};

/// The keys this window answers to, for the console summary.
pub(super) const CONTROLS: [(&str, &str); 5] = [
    ("Space", "Play / pause"),
    ("Left/Right", "Seek ±5s"),
    ("Up/Down", "Volume"),
    ("F", "Full screen"),
    ("Esc", "Stop and close"),
];

/// The smallest client area a film opens in.
const MIN_SIZE: (u32, u32) = (320, 200);

/// Play `engine`'s pictures in a window until the film ends or the user
/// closes it, then report how playback ended.
pub(super) fn run(engine: Engine, title: String, picture: (u32, u32)) -> Result<(), TossError> {
    let mut app = App {
        engine,
        title,
        picture,
        shown: None,
        current: None,
        retired: None,
        error: None,
    };
    let ran = display::with_event_loop(|event_loop| event_loop.run_app_on_demand(&mut app));
    let App { engine, error, .. } = app;
    // The engine is stopped before any verdict is given, so its threads
    // never outlive the window.
    let played = engine.finish();
    ran.map_err(|NoDisplay(reason)| TossError::other(format!("no display: {reason}")))?
        .map_err(|err| TossError::other(format!("video window stopped: {err}")))?;
    match error {
        Some(err) => Err(err),
        None => played,
    }
}

struct Shown {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
}

struct App {
    engine: Engine,
    title: String,
    /// The film's display size: the aspect every picture is drawn at.
    picture: (u32, u32),
    shown: Option<Shown>,
    current: Option<Frame>,
    /// The picture `current` replaced, on its way back to the engine.
    retired: Option<Frame>,
    error: Option<TossError>,
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, step: &str, err: impl std::fmt::Display) {
        self.error = Some(TossError::other(format!(
            "video window could not {step}: {err}"
        )));
        self.close(event_loop);
    }

    fn close(&mut self, event_loop: &ActiveEventLoop) {
        self.shown = None;
        event_loop.exit();
    }

    /// The film's own size, grown to a usable minimum and held inside
    /// four fifths of the screen, aspect kept either way.
    fn initial_size(&self, event_loop: &ActiveEventLoop) -> PhysicalSize<u32> {
        let screen = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())
            .map(|monitor| monitor.size())
            .map(|size| (size.width, size.height))
            .unwrap_or((0, 0));
        let (width, height) = opening_size(self.picture, screen);
        PhysicalSize::new(width, height)
    }

    /// Tell the decoder what size the window now shows pictures at.
    fn resized(&mut self, window: (u32, u32)) {
        let (_, _, width, height) = letterbox(self.picture, window);
        self.engine.set_size(width, height);
    }

    fn redraw(&mut self) -> Result<(), String> {
        let Some(shown) = self.shown.as_mut() else {
            return Ok(());
        };
        let size = shown.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return Ok(());
        };
        shown.surface.resize(w, h).map_err(|err| err.to_string())?;
        let mut buffer = shown.surface.buffer_mut().map_err(|err| err.to_string())?;
        paint(
            &mut buffer,
            (size.width, size.height),
            self.picture,
            self.current.as_ref(),
        );
        buffer.present().map_err(|err| err.to_string())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.shown.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.title.clone())
            .with_inner_size(self.initial_size(event_loop));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Rc::new(window),
            Err(err) => return self.fail(event_loop, "open a window", err),
        };
        let context = match softbuffer::Context::new(window.clone()) {
            Ok(context) => context,
            Err(err) => return self.fail(event_loop, "reach the display", err),
        };
        let surface = match softbuffer::Surface::new(&context, window.clone()) {
            Ok(surface) => surface,
            Err(err) => return self.fail(event_loop, "create a drawing surface", err),
        };
        let size = window.inner_size();
        self.resized((size.width, size.height));
        window.focus_window();
        window.request_redraw();
        self.shown = Some(Shown { window, surface });
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.shown.is_none() {
            return;
        }
        let poll = self.engine.poll_video(self.retired.take());
        if let Some(frame) = poll.frame {
            self.retired = self.current.replace(frame);
            if let Some(shown) = &self.shown {
                shown.window.request_redraw();
            }
        }
        if poll.finished {
            return self.close(event_loop);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + poll.wait));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => self.close(event_loop),
            WindowEvent::Resized(size) => {
                self.resized((size.width, size.height));
                if let Some(shown) = &self.shown {
                    shown.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(err) = self.redraw() {
                    self.fail(event_loop, "draw the picture", err);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                match event.logical_key.as_ref() {
                    Logical::Named(NamedKey::Space) | Logical::Character(" ") if !event.repeat => {
                        self.engine.toggle_pause();
                    }
                    Logical::Named(NamedKey::ArrowRight) => {
                        self.engine.seek_by(SEEK_STEP);
                    }
                    Logical::Named(NamedKey::ArrowLeft) => {
                        self.engine.seek_by(-SEEK_STEP);
                    }
                    Logical::Named(NamedKey::ArrowUp) => {
                        self.engine.change_volume(1);
                    }
                    Logical::Named(NamedKey::ArrowDown) => {
                        self.engine.change_volume(-1);
                    }
                    Logical::Character("f" | "F") if !event.repeat => {
                        if let Some(shown) = &self.shown {
                            let next = match shown.window.fullscreen() {
                                Some(_) => None,
                                None => Some(Fullscreen::Borderless(None)),
                            };
                            shown.window.set_fullscreen(next);
                        }
                    }
                    Logical::Named(NamedKey::Escape) => self.close(event_loop),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// The client size a film of `picture` opens at on a `screen` (0×0 when
/// the screen is unknown).
fn opening_size(picture: (u32, u32), screen: (u32, u32)) -> (u32, u32) {
    let (mut width, mut height) = (f64::from(picture.0.max(1)), f64::from(picture.1.max(1)));
    let grow = (f64::from(MIN_SIZE.0) / width).max(f64::from(MIN_SIZE.1) / height);
    if grow > 1.0 {
        width *= grow;
        height *= grow;
    }
    if screen.0 > 0 && screen.1 > 0 {
        let room = (f64::from(screen.0) * 0.8, f64::from(screen.1) * 0.8);
        let shrink = (room.0 / width).min(room.1 / height);
        if shrink < 1.0 {
            width *= shrink;
            height *= shrink;
        }
    }
    (
        (width.round() as u32).max(1),
        (height.round() as u32).max(1),
    )
}

/// Where a picture of aspect `picture` sits in a `window`: the largest
/// rectangle of that aspect, centred. Returns `(x, y, width, height)`.
fn letterbox(picture: (u32, u32), window: (u32, u32)) -> (u32, u32, u32, u32) {
    let (pw, ph) = (u64::from(picture.0.max(1)), u64::from(picture.1.max(1)));
    let (ww, wh) = (u64::from(window.0.max(1)), u64::from(window.1.max(1)));
    // Compare aspects by cross-multiplying: no rounding, no division by 0.
    let (width, height) = if ww * ph <= wh * pw {
        (ww, (ww * ph / pw).max(1))
    } else {
        ((wh * pw / ph).max(1), wh)
    };
    (
        ((ww - width) / 2) as u32,
        ((wh - height) / 2) as u32,
        width as u32,
        height as u32,
    )
}

/// softbuffer's pixel is `0x00RRGGBB`; BGRA bytes are that number in
/// little-endian with the alpha on top, which is masked off.
fn pixel(bgra: [u8; 4]) -> u32 {
    u32::from_le_bytes(bgra) & 0x00FF_FFFF
}

/// Draw `frame` letterboxed on black into a `size` window buffer.
fn paint(buffer: &mut [u32], size: (u32, u32), picture: (u32, u32), frame: Option<&Frame>) {
    buffer.fill(0);
    let Some(frame) = frame else {
        return;
    };
    let (window_w, window_h) = (size.0 as usize, size.1 as usize);
    let (frame_w, frame_h) = (frame.width as usize, frame.height as usize);
    if buffer.len() != window_w * window_h
        || frame_w == 0
        || frame_h == 0
        || frame.pixels.len() != frame_w * frame_h * 4
    {
        return;
    }
    let (x, y, width, height) = letterbox(picture, size);
    let (x, y, width, height) = (x as usize, y as usize, width as usize, height as usize);

    if (width, height) == (frame_w, frame_h) {
        // The normal case: the decoder already produced this exact size.
        for (row, source) in frame.pixels.chunks_exact(frame_w * 4).enumerate() {
            let start = (y + row) * window_w + x;
            let target = &mut buffer[start..start + width];
            for (out, bgra) in target.iter_mut().zip(source.as_chunks::<4>().0) {
                *out = pixel(*bgra);
            }
        }
        return;
    }

    // A picture of another size (the window has just been resized):
    // nearest neighbour until one of the right size arrives.
    let pixels = frame.pixels.as_chunks::<4>().0;
    for row in 0..height {
        let source_row = row * frame_h / height;
        let source = &pixels[source_row * frame_w..(source_row + 1) * frame_w];
        let start = (y + row) * window_w + x;
        for (column, out) in buffer[start..start + width].iter_mut().enumerate() {
            *out = pixel(source[column * frame_w / width]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{letterbox, opening_size, paint};
    use crate::platform::portable::media::engine::Frame;

    fn frame(width: u32, height: u32, bgra: [u8; 4]) -> Frame {
        Frame {
            pts: 0,
            width,
            height,
            pixels: bgra.repeat((width * height) as usize),
        }
    }

    #[test]
    fn a_picture_is_centred_at_its_own_aspect() {
        assert_eq!(letterbox((160, 120), (320, 240)), (0, 0, 320, 240));
        // Wider window: bars left and right.
        assert_eq!(letterbox((160, 120), (400, 240)), (40, 0, 320, 240));
        // Taller window: bars above and below.
        assert_eq!(letterbox((1920, 1080), (800, 800)), (0, 175, 800, 450));
        // Degenerate sizes never divide by zero or vanish.
        assert_eq!(letterbox((0, 0), (0, 0)), (0, 0, 1, 1));
        assert_eq!(letterbox((4000, 1), (10, 10)), (0, 4, 10, 1));
    }

    #[test]
    fn a_film_opens_at_a_usable_size_inside_the_screen() {
        // Its own size when that fits.
        assert_eq!(opening_size((640, 360), (1920, 1080)), (640, 360));
        // Tiny pictures are grown, aspect kept.
        assert_eq!(opening_size((160, 120), (1920, 1080)), (320, 240));
        // Large pictures are held inside four fifths of the screen.
        assert_eq!(opening_size((3840, 2160), (1920, 1080)), (1536, 864));
        // An unknown screen imposes no ceiling.
        assert_eq!(opening_size((3840, 2160), (0, 0)), (3840, 2160));
    }

    #[test]
    fn painting_copies_an_exact_size_picture_and_leaves_bars_black() {
        // 4×2 window, 2×2 picture: one black column each side.
        let mut buffer = vec![0xDEAD_BEEF; 8];
        paint(
            &mut buffer,
            (4, 2),
            (2, 2),
            Some(&frame(2, 2, [1, 2, 3, 255])),
        );
        assert_eq!(
            buffer,
            [
                0,
                0x0003_0201,
                0x0003_0201,
                0,
                0,
                0x0003_0201,
                0x0003_0201,
                0
            ]
        );
    }

    #[test]
    fn painting_stretches_a_picture_of_another_size() {
        let mut buffer = vec![0; 16];
        paint(
            &mut buffer,
            (4, 4),
            (1, 1),
            Some(&frame(2, 2, [9, 9, 9, 255])),
        );
        assert!(buffer.iter().all(|&pixel| pixel == 0x0009_0909));
    }

    #[test]
    fn painting_without_a_picture_or_with_a_malformed_one_is_black() {
        let mut buffer = vec![7; 4];
        paint(&mut buffer, (2, 2), (2, 2), None);
        assert_eq!(buffer, [0; 4]);

        let mut broken = frame(2, 2, [1, 1, 1, 255]);
        broken.pixels.truncate(3);
        let mut buffer = vec![7; 4];
        paint(&mut buffer, (2, 2), (2, 2), Some(&broken));
        assert_eq!(buffer, [0; 4]);
    }
}
