//! Platform-specific code.
//!
//! This is one of the few places `#[cfg(target_os = ...)]` may appear (§12),
//! alongside `backend/` and the bootstrap. If platform conditionals start
//! showing up in `core/`, `handlers/`, `detect/` or `cli/`, the abstraction
//! boundary is wrong and needs rethinking rather than more `cfg`.
//!
//! The architectural invariant this module exists to protect (§10): removing
//! the entire Windows backend must never make the platform-independent core
//! impossible to compile. CI checks that by building on Linux as well.
//!
//! Phases 6 through 8 fill this module.

pub mod portable;

#[cfg(target_os = "windows")]
pub mod windows;
