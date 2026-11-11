//! The one connection to the display that every portable window shares.
//!
//! winit allows a single event loop per process, and one `toss` run can
//! open several windows in turn — pictures out of a Word document, a batch
//! that holds a photo and then a film. The loop therefore lives here, is
//! created on first use and is lent to whichever viewer is on screen.
//!
//! X11, Wayland and xkbcommon are loaded at run time, so a machine without
//! any of them is told so in a sentence ([`NoDisplay`]) instead of failing
//! to start the whole binary.

use std::cell::RefCell;

use winit::event_loop::EventLoop;

thread_local! {
    static EVENT_LOOP: RefCell<Option<EventLoop<()>>> = const { RefCell::new(None) };
}

/// Why there is no window: the display could not be reached at all.
#[derive(Debug)]
pub(crate) struct NoDisplay(pub(crate) String);

/// Make sure an event loop exists, or say why one cannot.
pub(crate) fn available() -> Result<(), NoDisplay> {
    EVENT_LOOP.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(new_event_loop().map_err(|err| NoDisplay(reason(&err.to_string())))?);
        }
        Ok(())
    })
}

/// Lend the event loop to `run`. Fails with [`NoDisplay`] exactly when
/// [`available`] would.
pub(crate) fn with_event_loop<T>(
    run: impl FnOnce(&mut EventLoop<()>) -> T,
) -> Result<T, NoDisplay> {
    available()?;
    EVENT_LOOP.with(|cell| {
        let mut slot = cell.borrow_mut();
        let event_loop = slot.as_mut().expect("event loop was just created");
        Ok(run(event_loop))
    })
}

/// winit prefixes OS errors with the source line that raised them
/// (`os error at /…/mod.rs:788: `). That is its build machine's path, not
/// something to show a user: keep only the sentence after it.
fn reason(text: &str) -> String {
    match text
        .split_once(".rs:")
        .and_then(|(_, rest)| rest.split_once(": "))
    {
        Some((line, sentence)) if !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()) => {
            sentence.to_string()
        }
        _ => text.to_string(),
    }
}

fn new_event_loop() -> Result<EventLoop<()>, winit::error::EventLoopError> {
    let mut builder = EventLoop::builder();
    #[cfg(any(
        target_os = "linux",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        use winit::platform::wayland::EventLoopBuilderExtWayland;
        use winit::platform::x11::EventLoopBuilderExtX11;
        EventLoopBuilderExtX11::with_any_thread(&mut builder, true);
        EventLoopBuilderExtWayland::with_any_thread(&mut builder, true);
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::reason;

    #[test]
    fn a_display_failure_is_reported_without_the_build_path() {
        assert_eq!(
            reason(
                "os error at /root/.cargo/registry/winit/src/linux/mod.rs:788: Failed to load one of xlib's shared libraries"
            ),
            "Failed to load one of xlib's shared libraries"
        );
        assert_eq!(
            reason("neither WAYLAND_DISPLAY nor DISPLAY is set."),
            "neither WAYLAND_DISPLAY nor DISPLAY is set."
        );
        assert_eq!(reason("see lib.rs: later"), "see lib.rs: later");
    }
}
