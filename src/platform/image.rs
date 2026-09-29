//! The image-viewing capability, stated once for every platform.
//!
//! This module is the seam between what Toss wants and how a particular
//! machine does it (IMAGE_VIEWER.md §3). Everything above it is portable;
//! everything platform-specific sits behind the single `cfg` inside [`view`].
//!
//! The rule the whole design turns on: **Toss owns behaviour, the platform
//! provides capabilities.** Which images belong to a browsing session, which
//! one is open and in what order are decided here and above — never inside a
//! message handler (IMAGE_VIEWER.md §4).

use std::path::{Path, PathBuf};

use crate::core::error::TossError;

/// What a viewer has to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewRequest {
    /// One browsing session, in the order previous/next walks it.
    pub images: Vec<PathBuf>,
    /// Which entry of `images` opens first.
    pub index: usize,
}

impl ViewRequest {
    /// The image the window should open on, or `None` if the request does
    /// not name one.
    pub fn current(&self) -> Option<&Path> {
        self.images.get(self.index).map(PathBuf::as_path)
    }
}

/// Open the viewer for `request`.
///
/// A build without a viewer to offer — a non-Windows target, or Windows
/// without the `image` feature — answers with the same refusal Toss gives for
/// every capability it was not compiled with. That is a supported state rather
/// than a broken one (§14), and it keeps `toss <anything>` defined on every
/// platform §7 asks for.
///
/// The signature is the entire boundary. When the window exists, nothing above
/// this line changes: no handle, no message, no pixel format crosses it.
pub fn view(request: &ViewRequest) -> Result<(), TossError> {
    // Unreachable from `handlers::image`, which always fills the request in.
    // Expressed as an error rather than an assertion because §23 forbids
    // panicking on anything that could reach here from the outside.
    if request.current().is_none() {
        return Err(TossError::other("image viewer was given no image to open"));
    }

    Err(TossError::not_implemented("image viewing"))
}
