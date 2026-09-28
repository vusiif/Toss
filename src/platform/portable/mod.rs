//! Portable backend.
//!
//! Provides the same user-visible semantics without leaning on any one
//! operating system's exclusive facilities (§13.2). This family may bundle
//! mature libraries; being substantially larger than a native build is
//! expected and is not a bug.
//!
//! "Portable" does not mean "never calls an OS API" — it means core does not
//! depend on a platform-exclusive implementation (§13.3).
//!
//! Phase 8 fills this module.
