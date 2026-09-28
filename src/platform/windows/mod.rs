//! Windows native backend.
//!
//! Reuses what the operating system already provides — Win32, WIC, Direct2D,
//! DirectWrite, the Shell, and Windows security APIs — because that keeps the
//! binary small, the startup fast, and the behaviour native (§13.1).
//!
//! Phase 6 fills this module.
