//! Command-line surface.
//!
//! `toss <path>` (automatic mode) and `toss <verb> ...` (explicit mode) are
//! parsed here and handed to the dispatcher. Paths stay as `Path`/`OsStr`
//! from end to end (§22): never round-trip them through `str`, because a
//! valid path need not be valid Unicode.
//!
//! Phase 1 fills this module.
