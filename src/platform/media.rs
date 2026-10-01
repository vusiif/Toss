//! The media-playback capability, stated once for every platform.
//!
//! This module is the seam between what Toss wants and how a particular
//! machine does it — the same shape as `platform/image.rs`, and for the same
//! reason: everything above it is portable, everything platform-specific
//! sits behind the single `cfg` inside [`play`] (`MEDIA_PLAYBACK.md` §5).
//!
//! The rule the design turns on, from `Toss_AGENTS.md`: **Toss owns
//! behaviour, the platform provides capabilities.** Which file is being
//! played, what "next" would mean and how loud the volume should be reported
//! are questions above this line; how a session is created and what its
//! events mean belong below it.

use std::path::Path;

use crate::core::error::TossError;

/// Play `path` until it finishes or the user stops it.
///
/// Two answers, chosen by one `cfg`:
///
/// - Windows built with the `media` feature hands the path to the Media
///   Foundation backend, which owns a playback session the way the viewer
///   owns a window.
/// - Everywhere else — a non-Windows target, or Windows without the feature —
///   the answer is the refusal Toss gives for every capability it was not
///   compiled with: exit 3, `not implemented yet: media playback`. That is a
///   supported state rather than a broken one, and it keeps `toss <anything>`
///   defined on every platform, exactly as §8 of `IMAGE_VIEWER.md` requires
///   for images.
///
/// The signature is the entire boundary. When a session exists, nothing
/// above this line changes: no handle, no event, no COM interface crosses it.
#[cfg(all(windows, feature = "media"))]
pub fn play(path: &Path) -> Result<(), TossError> {
    super::windows::media::play(path)
}

/// See the `cfg`'d sibling above; both answer the same way unless there is
/// a player to offer.
#[cfg(not(all(windows, feature = "media")))]
pub fn play(_path: &Path) -> Result<(), TossError> {
    Err(TossError::not_implemented("media playback"))
}
