//! Document parsing and data structures module
//!
//! This module provides functionality for parsing Microsoft Word (.docx) documents
//! and converting them into a structured representation.
//!
//! During refactoring: Incrementally extracting modules

pub(crate) mod cleanup;
pub(crate) mod io;
pub(crate) mod loader;
pub mod models;
pub mod package;
pub(crate) mod parsing;
pub mod query;

// Re-export all models and query functions
pub use models::*;
pub use package::{DocxPackage, DocxRelationship, PackageError, PackageLimits};
pub use query::*;

// Re-export main document loading function
pub use anyhow::Error as DocumentError;
pub use loader::load_document;
