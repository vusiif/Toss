//! The portable viewer window: winit for the window and input, softbuffer
//! to put Toss-drawn pixels on screen. Both load the display stack (X11,
//! Wayland, xkbcommon) at run time, so a machine without one is told so in
//! a sentence instead of failing to start the whole binary.

use std::num::NonZeroU32;
use std::rc::Rc;

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key as Logical, NamedKey};
use winit::platform::run_on_demand::EventLoopExtRunOnDemand;
use winit::window::{CursorIcon, Window, WindowId};

use crate::core::error::TossError;
use crate::platform::portable::display::{self, NoDisplay};

use super::view::{Effect, Key, View};

/// Show `view` in a window and return when it closes.
pub(super) fn run(view: View) -> Result<(), TossError> {
    let mut app = App {
        view,
        window: None,
        error: None,
        cursor: (0, 0),
    };
    display::with_event_loop(|event_loop| event_loop.run_app_on_demand(&mut app))
        .map_err(|NoDisplay(reason)| TossError::other(format!("no display: {reason}")))?
        .map_err(|err| TossError::other(format!("image viewer stopped: {err}")))?;
    match app.error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

struct Shown {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
}

struct App {
    view: View,
    window: Option<Shown>,
    error: Option<TossError>,
    cursor: (i32, i32),
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, step: &str, err: impl std::fmt::Display) {
        self.error = Some(TossError::other(format!(
            "image viewer could not {step}: {err}"
        )));
        self.close(event_loop);
    }

    fn close(&mut self, event_loop: &ActiveEventLoop) {
        // Dropping the window closes it now rather than at process exit, so
        // the next viewer this process opens starts from a clean screen.
        self.window = None;
        event_loop.exit();
    }

    /// The initial client size: the picture (at least 320×200) capped at half
    /// the screen, or a page fitted inside half the screen at its aspect —
    /// the same rule as the Windows viewer.
    fn initial_size(&self, event_loop: &ActiveEventLoop) -> PhysicalSize<u32> {
        let (width, height) = self.view.image_size();
        let screen = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())
            .map(|monitor| monitor.size())
            .map(|size| (size.width as i32, size.height as i32))
            .unwrap_or((0, 0));
        let half = (screen.0 / 2, screen.1 / 2);
        let (w, h) = if self.view.preserve_initial_aspect() && half.0 > 0 && half.1 > 0 {
            crate::platform::pdf::initial_client_size((width, height), half, (0, 0))
        } else {
            let want = ((width as i32).max(320), (height as i32).max(200));
            (
                if half.0 > 0 {
                    want.0.min(half.0)
                } else {
                    want.0
                },
                if half.1 > 0 {
                    want.1.min(half.1)
                } else {
                    want.1
                },
            )
        };
        PhysicalSize::new(w.max(1) as u32, h.max(1) as u32)
    }

    fn redraw(&mut self) -> Result<(), String> {
        let Some(shown) = self.window.as_mut() else {
            return Ok(());
        };
        let size = shown.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return Ok(());
        };
        shown.surface.resize(w, h).map_err(|err| err.to_string())?;
        let mut buffer = shown.surface.buffer_mut().map_err(|err| err.to_string())?;
        self.view.paint(&mut buffer, (size.width, size.height));
        buffer.present().map_err(|err| err.to_string())
    }

    fn apply(&mut self, effect: Effect, event_loop: &ActiveEventLoop) {
        let Some(shown) = self.window.as_ref() else {
            return;
        };
        match effect {
            Effect::Nothing => {}
            Effect::Redraw => shown.window.request_redraw(),
            Effect::Retitle => {
                shown.window.set_title(&self.view.title());
                shown.window.request_redraw();
            }
            Effect::Close => self.close(event_loop),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.view.title())
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
        self.view.resize((size.width as i32, size.height as i32));
        window.focus_window();
        window.request_redraw();
        self.window = Some(Shown { window, surface });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => self.close(event_loop),
            WindowEvent::Resized(size) => {
                self.view.resize((size.width as i32, size.height as i32));
                self.apply(Effect::Redraw, event_loop);
            }
            WindowEvent::RedrawRequested => {
                if let Err(err) = self.redraw() {
                    self.fail(event_loop, "draw the picture", err);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x.round() as i32, position.y.round() as i32);
                let effect = self.view.moved(self.cursor);
                self.apply(effect, event_loop);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Windows units: 120 per notch, positive = away from the user.
                let units = match delta {
                    MouseScrollDelta::LineDelta(_, lines) => (lines * 120.0) as f64,
                    MouseScrollDelta::PixelDelta(PhysicalPosition { y, .. }) => y * 2.0,
                };
                let units = units.clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
                let effect = self.view.wheel(units, self.cursor);
                self.apply(effect, event_loop);
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let icon = match state {
                    ElementState::Pressed => {
                        self.view.press(self.cursor);
                        CursorIcon::Grabbing
                    }
                    ElementState::Released => {
                        self.view.release();
                        CursorIcon::Default
                    }
                };
                if let Some(shown) = &self.window {
                    shown.window.set_cursor(icon);
                }
            }
            WindowEvent::Focused(false) if self.view.dragging() => {
                // The mouse was taken away; the drag is over.
                self.view.release();
                if let Some(shown) = &self.window {
                    shown.window.set_cursor(CursorIcon::Default);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let key = match event.logical_key.as_ref() {
                    Logical::Named(NamedKey::ArrowRight | NamedKey::PageDown) => Key::Next,
                    Logical::Named(NamedKey::ArrowLeft | NamedKey::PageUp) => Key::Previous,
                    Logical::Named(NamedKey::Escape) => Key::Close,
                    Logical::Character("f" | "F") => Key::Fit,
                    Logical::Character("1") => Key::ActualSize,
                    _ => return,
                };
                let effect = self.view.key(key);
                self.apply(effect, event_loop);
            }
            _ => {}
        }
    }
}
