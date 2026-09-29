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
/// Two answers, chosen by one `cfg`:
///
/// - Windows built with the `image` feature hands the request to the native
///   viewer, which is the only place in Toss that owns a window.
/// - Everywhere else — a non-Windows target, or Windows without the feature —
///   the answer is the refusal Toss gives for every capability it was not
///   compiled with. That is a supported state rather than a broken one (§14),
///   and it keeps `toss <anything>` defined on every platform §7 asks for.
///
/// The signature is the entire boundary. When a window exists, nothing above
/// this line changes: no handle, no message, no pixel format crosses it.
#[cfg(all(windows, feature = "image"))]
pub fn view(request: &ViewRequest) -> Result<(), TossError> {
    ensure_openable(request)?;
    super::windows::image::view(request)
}

/// See the `cfg`'d sibling above; both answer the same way unless there is a
/// viewer to offer.
#[cfg(not(all(windows, feature = "image")))]
pub fn view(request: &ViewRequest) -> Result<(), TossError> {
    ensure_openable(request)?;
    Err(TossError::not_implemented("image viewing"))
}

/// Reject a request that names no image.
///
/// Unreachable from `handlers::image`, which always fills the request in, and
/// expressed as an error rather than an assertion because §23 forbids
/// panicking on anything that could reach here from the outside.
fn ensure_openable(request: &ViewRequest) -> Result<(), TossError> {
    if request.current().is_none() {
        return Err(TossError::other("image viewer was given no image to open"));
    }
    Ok(())
}

/// The zoom rule: which levels exist, where a viewer starts, and what
/// stepping does.
///
/// **Toss-owned** (`IMAGE_VIEWER.md` §4): the ladder and its two limits are
/// rules, so a future backend on another platform steps through the same five
/// levels without being told what they are. Which level is *current* is
/// interaction state and belongs to the platform half — the wheel event that
/// moves it happens there.
///
/// The `cfg_attr` on the module is not a way to hide unused code. These
/// items are consumed only by the Windows renderer, so a build with no viewer
/// in it has nothing that calls them — but gating them out instead would take
/// their tests off Linux, leaving a rule that only one platform can check.
/// The tests run everywhere; the suppression is scoped to the builds where
/// the consumer is structurally absent.
#[cfg_attr(not(all(windows, feature = "image")), allow(dead_code))]
pub mod zoom {
    /// The levels, as multiples of the image's own size.
    ///
    /// Five rather than a continuous range: §18.3 asks for zoom, not for a
    /// magnifier, and a stepped ladder is the smallest thing that satisfies it
    /// while keeping the rule testable and the state a single index.
    pub const STEPS: [f64; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

    /// The level a viewer opens on: one to one.
    ///
    /// 100% is the only starting level that cannot be wrong for an image whose
    /// real size Toss has not been asked to second-guess — and fit-to-window
    /// is a question about the window, which is §11's "not yet".
    pub const START: usize = 2;

    /// The factor at `step`.
    ///
    /// An index with no level falls back to the starting one instead of
    /// panicking. The index is nudged by a scroll wheel, and §23 does not
    /// allow a wheel — or an index arithmetic bug — to take Toss down.
    #[must_use]
    pub fn factor(step: usize) -> f64 {
        STEPS.get(step).copied().unwrap_or(STEPS[START])
    }

    /// One level closer to the pixels, stopping at the largest.
    #[must_use]
    pub fn step_up(step: usize) -> usize {
        step.saturating_add(1).min(STEPS.len() - 1)
    }

    /// One level further from the pixels, stopping at the smallest.
    ///
    /// Stopping rather than wrapping: running off the end of a zoom ladder
    /// into the opposite direction is a different rule from previous/next,
    /// which does wrap (`IMAGE_VIEWER.md` §5).
    #[must_use]
    pub fn step_down(step: usize) -> usize {
        step.saturating_sub(1)
    }

    #[cfg(test)]
    mod tests {
        use super::{START, STEPS, factor, step_down, step_up};

        #[test]
        fn the_ladder_is_ordered_and_starts_one_to_one() {
            for pair in STEPS.windows(2) {
                assert!(
                    pair[0] < pair[1],
                    "the levels must increase strictly, got {pair:?}"
                );
            }

            assert!(
                STEPS[START].eq(&1.0),
                "a viewer must open at the image's own size"
            );
        }

        #[test]
        fn stepping_up_stops_at_the_largest_level() {
            let mut step = START;
            for _ in 0..10 {
                step = step_up(step);
            }

            assert_eq!(step, STEPS.len() - 1, "it ran off the top");
            assert_eq!(factor(step), *STEPS.last().expect("not empty"));
        }

        #[test]
        fn stepping_down_stops_at_the_smallest_level() {
            let mut step = START;
            for _ in 0..10 {
                step = step_down(step);
            }

            assert_eq!(step, 0, "it ran off the bottom");
            assert_eq!(factor(step), STEPS[0]);
        }

        #[test]
        fn neither_end_of_the_ladder_wraps() {
            // The difference from previous/next, which does wrap: zooming past
            // the end has to stop, or the smallest level would become the
            // largest and the user would have no idea where they were.
            assert_eq!(step_down(step_down(0)), 0);
            assert_eq!(step_up(step_up(STEPS.len() - 1)), STEPS.len() - 1);
        }

        #[test]
        fn an_index_with_no_level_is_not_a_panic() {
            // The index is what a scroll wheel moves; nothing here may unwind.
            assert_eq!(factor(START + 99), STEPS[START]);
            assert_eq!(factor(usize::MAX), STEPS[START]);
            assert_eq!(step_up(usize::MAX), STEPS.len() - 1);
            assert_eq!(step_down(0), 0);
        }
    }
}
