//! Handlers.
//!
//! A handler states *what* Toss wants to do — extract, compress, view, play —
//! and never calls a platform API itself; it asks a backend for that (§11).
//! Handlers must not import WIC, Direct2D, Cocoa, X11 or any other
//! platform-specific facility directly.
//!
//! Default actions stay on the safe side of the line (§5): read-only, then
//! create new output, then modify original, then delete. Nothing here may
//! become an automatic destructive default.
//!
//! Dispatching lives in `dispatch/`; this module is where handlers themselves
//! are declared. Each is a plain function rather than a registry: there is
//! nothing yet that would justify machinery to look one up (§42).
//!
//! Phases 4 through 7 fill this module.

pub mod archive;
