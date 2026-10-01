//! Windows native backend.
//!
//! Reuses what the operating system already provides — Win32 for windows and
//! input, WIC for decoding, GDI for drawing — because that keeps the binary
//! small, the startup fast and the behaviour native (§13.1).
//!
//! Direct2D is deliberately absent. Phase 6 measures no capability gap that
//! needs it (`IMAGE_VIEWER.md` §2), and writing a Direct2D-shaped abstraction
//! for a day that has not arrived is exactly what §11 of the Phase 6 guide
//! rules out.
//!
//! Nothing declared here is reachable from outside `platform/` — that is what
//! makes the boundary in `IMAGE_VIEWER.md` §3 a boundary rather than a
//! convention. Phase 6 fills `image`; Phase 7 fills `media`; Phase 8 looks
//! at the rest.

#[cfg(feature = "image")]
pub mod image;

#[cfg(feature = "media")]
pub mod media;
