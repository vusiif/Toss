//! Backend abstraction.
//!
//! Handlers depend on the traits declared here; concrete native and portable
//! implementations sit behind them (§11). Raw FFI stays below this line and
//! never leaks upward into handlers or core (§32): no `AVPacket*`, no `HRESULT`,
//! no COM pointers, unless an architectural review says otherwise.
//!
//! `cfg` for platform selection belongs here, in `platform/`, and in the
//! bootstrap — nowhere else (§12).
//!
//! Phases 4 through 8 fill this module.

// Step 7 now routes an archive through here, so the trait, the router and
// the policy are all live. What this allowance still covers is deliberate
// rather than forgotten:
//
// - `ArchiveBackend::probe` / `list` — §45 step 5 built them as the safer,
//   read-only surface, but no product path calls them yet; their caller
//   arrives with the `toss info` verb (§4.2, §20).
// - `ArchiveRouter::len` / `is_empty`, `ArchiveProbe`, `CompressionMethod`
//   and the `MultiVolume` variant — used by tests and reserved by §9/§6
//   without a product caller yet.
// - a handful of `ARCHIVE_*` constants that `reader.rs` compares against
//   implicitly rather than naming.
//
// Delete entries here as they gain callers; do not add to this list to make
// a warning go away.
#[allow(dead_code)]
pub mod archive;
