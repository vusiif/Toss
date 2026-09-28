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
//! are declared and registered, and it stays empty until the first real one
//! arrives in Phase 4. Building the registration machinery before there is
//! anything to register would be abstraction for its own sake (§42).
//!
//! Phases 4 through 7 fill this module.
