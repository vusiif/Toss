//! Toss-owned archive vocabulary.
//!
//! Nothing in this file knows that libarchive exists. These are the types
//! every layer above the backend speaks, so a second backend can be added
//! later without anything outside `backend/archive/libarchive/` changing
//! (§1, §50).
//!
//! Only what Phase 4 actually needs is modelled here. The extension points
//! the architecture requires are preserved — container and compression stay
//! separate concepts, and an archive is not assumed to be one file — but
//! unused variants are deliberately left out (§45 step 1).

use std::path::{Path, PathBuf};

/// The container format.
///
/// Deliberately separate from [`CompressionMethod`]: `foo.tar.zst` is a TAR
/// container behind a Zstd filter, and collapsing the two would make that
/// impossible to express (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveFormat {
    SevenZip,
    Zip,
    Rar,
    Unknown,
}

impl ArchiveFormat {
    /// The word this format is reported under.
    pub const fn label(self) -> &'static str {
        match self {
            Self::SevenZip => "7z",
            Self::Zip => "zip",
            Self::Rar => "rar",
            Self::Unknown => "unknown",
        }
    }
}

/// How the contents were compressed, independent of the container (§6).
///
/// Only the methods the v0.1 formats use are named. `.zst` and `.lz4`
/// arrive with the phases that need them (§12, §13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompressionMethod {
    Store,
    Deflate,
    Lzma,
    Lzma2,
    Unknown,
}

/// The archive to operate on.
///
/// An archive is not necessarily one physical file: `.7z.001` and
/// `.part1.rar` families are real, and the model must not foreclose them
/// forever (§9). Volume discovery itself is deliberately not implemented —
/// only the assumption that one archive equals one file is removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveInput {
    Single(PathBuf),
    MultiVolume {
        primary: PathBuf,
        volumes: Vec<PathBuf>,
    },
}

impl ArchiveInput {
    /// A single-file archive.
    pub fn single(path: impl Into<PathBuf>) -> Self {
        Self::Single(path.into())
    }

    /// The file a backend should open first.
    pub fn primary(&self) -> &Path {
        match self {
            Self::Single(path) => path,
            Self::MultiVolume { primary, .. } => primary,
        }
    }
}

/// One member of an archive, exactly as the archive records it.
///
/// [`ArchiveEntry::path`] is untrusted input: it is whatever the archive
/// claims, and must be validated against the output root before anything is
/// written (§16). Validation belongs to Toss, never to the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: Option<u64>,
}

/// What probing learned about an input (§41).
///
/// A probe answers "what is this, truthfully?" so that a suffix which
/// disagrees with the contents is caught before anything is extracted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveProbe {
    pub format: ArchiveFormat,
    pub compression: CompressionMethod,
}

/// Where to extract an archive.
///
/// [`ExtractRequest::output_root`] is chosen by Toss (§21), not by the
/// backend: the backend is told where to write and never decides from the
/// archive's own names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractRequest {
    pub input: ArchiveInput,
    pub output_root: PathBuf,
}

/// What an extraction did.
///
/// Kept observable so archive-bomb limits can be enforced later without
/// redesigning the API (§18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractResult {
    pub entries: u64,
    pub bytes_written: u64,
}

/// What a backend can do, so routing can be decided by capability rather
/// than by library identity (§7).
#[derive(Debug, Clone, Copy)]
pub struct ArchiveCapabilities {
    pub read: &'static [ArchiveFormat],
    pub write: &'static [ArchiveFormat],
}

/// Archive failures owned by Toss rather than by any backend (§23).
///
/// These carry Toss's meaning, not libarchive's integer codes; whatever
/// diagnostic text a backend produced is retained as context rather than
/// leaked as the user-facing message.
///
/// Only the variants Phase 4 produces are present. Password handling (§19),
/// resource limits (§18) and similar arrive with the phases that need them
/// — §23 asks for no speculative variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    /// The input is not an archive Toss can read.
    UnsupportedFormat,
    /// The archive is damaged, with the backend's own words kept for context.
    CorruptArchive(String),
    /// An entry tried to write outside the output root (§16).
    UnsafePath(PathBuf),
    /// The destination already exists and must not be overwritten (§22).
    OutputConflict(PathBuf),
    PermissionDenied(PathBuf),
    /// The backend failed for a reason that fits no category above.
    BackendFailure(String),
}
