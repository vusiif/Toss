//! File classification.
//!
//! Directory first, then extension, then bounded content sniffing as a
//! fallback (§6). Sniffing must stay cheap: never read a whole large file
//! just to decide what it is (§35).
//!
//! An extension is a hint, not a security guarantee, and Toss must never
//! silently rename a file whose contents disagree with its name (§6.3).
//!
//! Phase 2 fills this module.
