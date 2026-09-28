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
