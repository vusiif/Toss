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

// Phase 4 is being landed in the sequence §45 prescribes: domain model and
// backend trait first, then the native binding, then dispatch. Nothing calls
// this module until step 7 wires it up, so the allowance covers the whole
// subtree and must be deleted the moment dispatch routes an archive to it.
#[allow(dead_code)]
pub mod archive;
