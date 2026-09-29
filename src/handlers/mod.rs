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

use std::path::Path;

use crate::backend::archive::ArchiveError;
use crate::core::error::TossError;

pub mod archive;
pub mod directory;

/// Turn a backend failure into something Toss can report (§23).
///
/// Lives here rather than in either handler because both ask the archive
/// backend for something, and the wording a user sees must not depend on
/// which of the two asked. The path is added at this layer because it is the
/// one that knows which input was being worked on; a backend deep inside
/// extraction only knows what went wrong.
pub(crate) fn translate(err: ArchiveError, path: &Path) -> TossError {
    match err {
        ArchiveError::UnsupportedFormat => TossError::UnsupportedFormat(path.to_path_buf()),
        ArchiveError::CorruptArchive(context) => TossError::CorruptInput {
            path: path.to_path_buf(),
            context,
        },
        ArchiveError::OutputConflict(target) => TossError::OutputConflict(target),
        ArchiveError::PermissionDenied(target) => TossError::PermissionDenied(target),
        ArchiveError::UnsafePath(entry) => TossError::other(format!(
            "archive entry would escape the output directory: {}",
            entry.display()
        )),
        ArchiveError::BackendFailure(context) => {
            TossError::other(format!("archive backend failed: {context}"))
        }
    }
}
