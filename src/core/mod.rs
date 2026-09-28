//! Platform-independent core.
//!
//! Every module below must still compile with the entire Windows backend
//! deleted (§10). No Win32, WIC, Direct2D, DirectWrite, COM, Cocoa, X11 or
//! Wayland reference may appear anywhere in this tree; if one becomes
//! necessary, it belongs behind a backend trait instead.

pub mod error;
pub mod exit_code;
pub mod log;
