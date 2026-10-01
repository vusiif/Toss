//! Media playback handler.
//!
//! §18.4's third input family: a file classified as Media is *played*, which
//! is the safest useful default for it (§5) — nothing here reads, writes or
//! modifies the file.
//!
//! One path in and one playback session out: a playlist is explicitly not
//! v0.1 (`MEDIA_PLAYBACK.md` §3), so unlike the image handler this one takes
//! a single path rather than curating a set to browse. What happens after
//! the platform half accepts it — a video window, a console companion, or a
//! refusal because this build has no player — is decided there.

use std::path::Path;

use crate::core::error::TossError;

/// Play `path`, or report — in words, and with the exit code §24 tables —
/// why it cannot be played.
///
/// The precondition mirrors the image handler's: anything that is not a
/// plain file is refused *here*, before the platform half is asked for
/// anything, so the complaint is about what the caller passed rather than
/// about a decoder that never saw it (§23). Dispatch has already classified
/// the input as Media by this point (§6), so the file itself is only checked
/// for being a file — re-running detection here would be a second opinion
/// where one is enough.
pub fn play(path: &Path) -> Result<(), TossError> {
    if !path.is_file() {
        return Err(TossError::invalid_arguments(format!(
            "{} is not a media file",
            path.display()
        )));
    }

    crate::platform::media::play(path)
}
