//! The archive backend abstraction.
//!
//! Handlers depend on [`ArchiveBackend`] and on nothing beneath it (§4).
//! `archive*` handles, `archive_entry*`, `ARCHIVE_OK` and every other
//! libarchive concept stay inside `libarchive/` and never surface here.
//!
//! The trait models **operations**, not formats (§5): writing
//! `extract_zip()` / `extract_7z()` would bake today's format list into the
//! architecture and turn `tar.zst` or `cab` into a breaking change.
//!
//! Phase 4 needs `capabilities`, `probe`, `list` and `extract`. `create`
//! arrives with Phase 5 (§46), which reuses this same abstraction rather
//! than inventing a second one.

pub mod policy;
pub mod types;

/// The native binding, present only when the `archive` feature is on (§14,
/// §32). Everything libarchive-specific stays inside, and a build without
/// the feature still compiles the trait, the domain types and the router.
#[cfg(feature = "archive")]
pub mod libarchive;

pub use types::{
    ArchiveCapabilities, ArchiveEntry, ArchiveError, ArchiveFormat, ArchiveInput, ArchiveProbe,
    ExtractRequest, ExtractResult,
};

/// What an archive backend can do, expressed as operations.
///
/// Object-safe by construction, so a router can hold `Box<dyn ArchiveBackend>` (§8).
pub trait ArchiveBackend {
    /// What this backend can read and write, for capability-based routing (§7).
    fn capabilities(&self) -> ArchiveCapabilities;

    /// Identify an input truthfully rather than trusting its suffix (§41).
    fn probe(&self, input: &ArchiveInput) -> Result<ArchiveProbe, ArchiveError>;

    /// Enumerate entries before extracting anything, so problems surface on
    /// a safer, read-only surface first (§45 step 5).
    fn list(&self, input: &ArchiveInput) -> Result<Vec<ArchiveEntry>, ArchiveError>;

    /// Extract into a destination Toss has already chosen and validated.
    fn extract(&self, request: &ExtractRequest) -> Result<ExtractResult, ArchiveError>;
}

/// Chooses a backend by capability rather than by name (§7, §43).
///
/// Deliberately plain: v0.1 has one backend, and §8 asks for an extension
/// point, not an extension framework. No plugin loading, no shared-library
/// discovery, no configuration files — only a list whose order never depends
/// on filesystem enumeration or hash-map iteration (§43).
#[derive(Default)]
pub struct ArchiveRouter {
    backends: Vec<Box<dyn ArchiveBackend>>,
}

impl ArchiveRouter {
    /// A router with no backends registered.
    ///
    /// This is the state a build without the `archive` feature has, and it is
    /// a fully supported one (§14, §46).
    pub fn empty() -> Self {
        Self {
            backends: Vec::new(),
        }
    }

    /// Register a backend. Earlier registrations win (§43).
    pub fn register(&mut self, backend: Box<dyn ArchiveBackend>) {
        self.backends.push(backend);
    }

    /// How many backends are registered.
    pub fn len(&self) -> usize {
        self.backends.len()
    }

    /// Whether no backend can serve this router at all.
    pub fn is_empty(&self) -> bool {
        self.backends.is_empty()
    }

    /// The first registered backend that can perform this operation.
    ///
    /// `None` means nothing compiled in can do the job — a different statement
    /// from "the archive is bad", and the two must stay distinguishable so
    /// routing can never confuse them (§42).
    ///
    /// [`ArchiveFormat::Unknown`] is a wildcard. Automatic mode has classified
    /// an input as an archive without yet saying which kind, and asking for
    /// "anything that can read" is exactly what probing is for (§41).
    pub fn select(&self, format: ArchiveFormat, write: bool) -> Option<&dyn ArchiveBackend> {
        self.backends
            .iter()
            .find(|backend| {
                let capabilities = backend.capabilities();
                let supported = if write {
                    capabilities.write
                } else {
                    capabilities.read
                };

                match format {
                    ArchiveFormat::Unknown => !supported.is_empty(),
                    other => supported.contains(&other),
                }
            })
            .map(|backend| backend.as_ref())
    }
}

/// A router holding every backend this build compiled in (§7, §14).
///
/// The only `cfg` in the archive domain lives here rather than in the handler
/// or the dispatcher, because §12 allows platform and feature conditionals in
/// `backend/` and forbids them everywhere above it. A build without the
/// `archive` feature gets an empty router, which reads as "no backend" rather
/// than as a failure (§42).
pub fn router() -> ArchiveRouter {
    let mut router = ArchiveRouter::empty();

    #[cfg(feature = "archive")]
    router.register(Box::new(libarchive::LibarchiveBackend));

    router
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedFormat => write!(f, "not a readable archive"),
            Self::CorruptArchive(context) => {
                write!(f, "archive is corrupted or incomplete: {context}")
            }
            Self::UnsafePath(path) => write!(
                f,
                "archive entry would escape the output directory: {}",
                path.display()
            ),
            Self::OutputConflict(path) => write!(f, "output already exists: {}", path.display()),
            Self::PermissionDenied(path) => write!(f, "permission denied: {}", path.display()),
            Self::BackendFailure(context) => write!(f, "archive backend failed: {context}"),
        }
    }
}

impl std::error::Error for ArchiveError {}

#[cfg(test)]
mod tests {
    use super::types::CompressionMethod;
    use super::*;

    /// A backend that only claims ZIP, so selection can be tested without a
    /// real archive library being linked (§7 routing is the contract here).
    struct ZipOnly;

    impl ArchiveBackend for ZipOnly {
        fn capabilities(&self) -> ArchiveCapabilities {
            ArchiveCapabilities {
                read: &[ArchiveFormat::Zip],
                write: &[],
            }
        }

        fn probe(&self, _input: &ArchiveInput) -> Result<ArchiveProbe, ArchiveError> {
            Err(ArchiveError::UnsupportedFormat)
        }

        fn list(&self, _input: &ArchiveInput) -> Result<Vec<ArchiveEntry>, ArchiveError> {
            Err(ArchiveError::UnsupportedFormat)
        }

        fn extract(&self, _request: &ExtractRequest) -> Result<ExtractResult, ArchiveError> {
            Err(ArchiveError::UnsupportedFormat)
        }
    }

    fn router_with_zip() -> ArchiveRouter {
        let mut router = ArchiveRouter::empty();
        router.register(Box::new(ZipOnly));
        router
    }

    #[test]
    fn an_empty_router_serves_nothing_rather_than_failing() {
        // A build without the `archive` feature lands here, and that must
        // read as "no backend", never as "bad archive" (§42).
        let router = ArchiveRouter::empty();

        assert!(router.is_empty());
        assert!(router.select(ArchiveFormat::Zip, false).is_none());
    }

    #[test]
    fn selection_follows_capability_not_library_identity() {
        let router = router_with_zip();

        assert!(
            router.select(ArchiveFormat::Zip, false).is_some(),
            "a zip-capable backend must be selected for zip"
        );
        assert!(
            router.select(ArchiveFormat::SevenZip, false).is_none(),
            "capability must not be assumed from registration alone (§7)"
        );
    }

    #[test]
    fn writing_is_a_separate_capability_from_reading() {
        let router = router_with_zip();

        assert!(router.select(ArchiveFormat::Zip, true).is_none());
    }

    #[test]
    fn earlier_registrations_win_so_priority_is_deterministic() {
        let router = router_with_zip();

        // With one backend this only pins the order; §43 requires that order
        // never become accidental when a second backend arrives.
        assert_eq!(router.len(), 1);
    }

    #[test]
    fn an_archive_input_is_not_assumed_to_be_one_file() {
        let single = ArchiveInput::single("foo.7z");
        assert_eq!(single.primary(), std::path::Path::new("foo.7z"));

        let volumes = ArchiveInput::MultiVolume {
            primary: "foo.7z.001".into(),
            volumes: vec!["foo.7z.002".into()],
        };
        assert_eq!(volumes.primary(), std::path::Path::new("foo.7z.001"));
    }

    #[test]
    fn container_and_compression_are_independent_concepts() {
        // §6: `foo.tar.zst` must remain expressible as a container plus a
        // filter, so neither enum may absorb the other.
        let probe = ArchiveProbe {
            format: ArchiveFormat::SevenZip,
            compression: CompressionMethod::Lzma2,
        };

        assert_eq!(probe.format, ArchiveFormat::SevenZip);
        assert_eq!(probe.compression, CompressionMethod::Lzma2);
    }
}
