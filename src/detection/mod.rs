//! File classification.
//!
//! The order is fixed by the detection strategy (§6):
//!
//! 1. **Directory first** — a directory is decided before any extension
//!    logic runs, because extensions do not describe directories.
//! 2. **Extension next** — for files the extension is the primary routing
//!    hint, because Toss is deliberately a file-type-driven utility (§6.2).
//! 3. **Bounded content sniffing as fallback** — when the extension says
//!    nothing usable, read a small header and match magic bytes (§6.3).
//!
//! The sniffer is capped at [`SNIFF_LEN`] bytes, so classifying a 40 GB file
//! costs the same as classifying a 1 KB one: never read a whole large file
//! just to decide what it is (§35).
//!
//! Classification never fails. An unrecognised file becomes [`Kind::Unknown`]
//! rather than an error, because `toss <anything>` must keep defined
//! behaviour (§7).

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::core::input::Input;

/// What Toss has decided an input is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Directory,
    Archive,
    Image,
    Media,
    Unknown,
}

impl Kind {
    /// The feature that would act on this kind, for diagnostics (§5).
    ///
    /// [`Kind::Unknown`] has none: there is no handler to name, only a file
    /// Toss cannot classify, and the honest reply is about the file itself.
    pub const fn default_action(self) -> Option<&'static str> {
        match self {
            Self::Directory => Some("directory compression"),
            Self::Archive => Some("archive extraction"),
            Self::Image => Some("image viewing"),
            Self::Media => Some("media playback"),
            Self::Unknown => None,
        }
    }

    /// The word this kind is reported under (§7).
    pub const fn label(self) -> &'static str {
        match self {
            Self::Directory => "directory",
            Self::Archive => "archive",
            Self::Image => "image",
            Self::Media => "media",
            Self::Unknown => "unknown",
        }
    }
}

/// Bytes a fallback probe may read, whatever the file's size (§35).
///
/// Sixteen covers the longest signature used here — the 12-byte RIFF form
/// type — with room to spare.
pub const SNIFF_LEN: usize = 16;

/// Classify a resolved input. Never returns an error.
pub fn classify(input: &Input) -> Kind {
    match input {
        Input::Directory(_) => Kind::Directory,
        Input::File(path) => classify_file(path),
    }
}

fn classify_file(path: &Path) -> Kind {
    // Extension first (§6.2); sniffing is reached only when it says nothing.
    if let Some(kind) = kind_from_extension(path) {
        return kind;
    }

    sniff(path).unwrap_or(Kind::Unknown)
}

fn kind_from_extension(path: &Path) -> Option<Kind> {
    // A non-UTF-8 extension falls through to sniffing rather than being
    // unwrapped or lossily decoded (§22).
    let extension = path.extension()?.to_str()?;

    match extension.to_ascii_lowercase().as_str() {
        // v0.1 archives (§18.1)
        "zip" | "7z" | "rar" => Some(Kind::Archive),
        // v0.1 images (§18.3)
        "jpg" | "jpeg" | "png" | "bmp" | "gif" => Some(Kind::Image),
        // v0.1 media (§18.4)
        "mp4" | "mkv" | "webm" | "avi" | "mov" | "mp3" | "flac" | "wav" => Some(Kind::Media),
        _ => None,
    }
}

fn sniff(path: &Path) -> Option<Kind> {
    let mut header = [0u8; SNIFF_LEN];
    let read = read_header(path, &mut header)?;

    kind_from_magic(&header[..read])
}

/// Fill `buf` with at most `buf.len()` bytes from the head of `path`.
///
/// Returns `None` if the file cannot be opened or read; the caller then
/// falls back to whatever the extension said, rather than failing (§7).
fn read_header(path: &Path, buf: &mut [u8]) -> Option<usize> {
    let mut file = File::open(path).ok()?;

    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        }
    }

    Some(filled)
}

fn kind_from_magic(header: &[u8]) -> Option<Kind> {
    // Archives
    if header.starts_with(b"PK\x03\x04")
        || header.starts_with(b"PK\x05\x06")
        || header.starts_with(b"PK\x07\x08")
    {
        return Some(Kind::Archive);
    }
    if header.starts_with(b"7z\xBC\xAF\x27\x1C") {
        return Some(Kind::Archive);
    }
    // Both RAR4 (`Rar!\x1A\x07\x00`) and RAR5 (`Rar!\x1A\x07\x01\x00`)
    // share this six-byte prefix.
    if header.starts_with(b"Rar!\x1A\x07") {
        return Some(Kind::Archive);
    }

    // Images
    if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Kind::Image);
    }
    if header.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(Kind::Image);
    }
    if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        return Some(Kind::Image);
    }
    if header.starts_with(b"BM") {
        return Some(Kind::Image);
    }

    // Media. RIFF carries its form type at offset 8, so AVI and WAVE share
    // a prefix and must be told apart by those four bytes.
    if header.get(8..12) == Some(&b"AVI "[..]) || header.get(8..12) == Some(&b"WAVE"[..]) {
        return Some(Kind::Media);
    }
    // ISO base media: `....ftyp` identifies MP4, MOV and friends.
    if header.get(4..8) == Some(&b"ftyp"[..]) {
        return Some(Kind::Media);
    }
    if header.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(Kind::Media); // Matroska / WebM
    }
    if header.starts_with(b"fLaC") || header.starts_with(b"ID3") {
        return Some(Kind::Media);
    }

    None
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{Kind, SNIFF_LEN, classify, kind_from_extension, kind_from_magic, read_header};
    use crate::core::input::Input;

    fn manifest(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    #[test]
    fn a_directory_is_decided_before_any_extension_logic() {
        assert_eq!(
            classify(&Input::Directory(manifest("src"))),
            Kind::Directory
        );
    }

    #[test]
    fn every_v001_family_maps_from_its_extension() {
        let cases = [
            ("a.zip", Kind::Archive),
            ("a.7z", Kind::Archive),
            ("a.rar", Kind::Archive),
            ("a.jpg", Kind::Image),
            ("a.jpeg", Kind::Image),
            ("a.png", Kind::Image),
            ("a.bmp", Kind::Image),
            ("a.gif", Kind::Image),
            ("a.mp4", Kind::Media),
            ("a.mkv", Kind::Media),
            ("a.webm", Kind::Media),
            ("a.avi", Kind::Media),
            ("a.mov", Kind::Media),
            ("a.mp3", Kind::Media),
            ("a.flac", Kind::Media),
            ("a.wav", Kind::Media),
        ];

        for (name, expected) in cases {
            assert_eq!(
                kind_from_extension(Path::new(name)),
                Some(expected),
                "wrong kind for {name}"
            );
        }
    }

    #[test]
    fn extensions_are_case_insensitive() {
        for name in ["A.JPG", "Archive.7Z", "clip.MKV"] {
            assert!(
                kind_from_extension(Path::new(name)).is_some(),
                "{name} was not recognised"
            );
        }
    }

    #[test]
    fn an_unusable_extension_falls_through_rather_than_deciding() {
        // No extension, an unknown one, and a dotfile with no extension at all.
        for name in ["README", "notes.dat", ".gitignore"] {
            assert_eq!(kind_from_extension(Path::new(name)), None, "for {name}");
        }
    }

    #[test]
    fn magic_bytes_identify_each_family_without_a_usable_extension() {
        let cases: [(&[u8], Kind); 15] = [
            (b"PK\x03\x04rest", Kind::Archive),
            (b"PK\x05\x06", Kind::Archive),
            (b"7z\xBC\xAF\x27\x1C", Kind::Archive),
            (b"Rar!\x1A\x07\x00", Kind::Archive),
            (b"Rar!\x1A\x07\x01\x00", Kind::Archive),
            (&[0xFF, 0xD8, 0xFF, 0xE0], Kind::Image),
            (
                &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0],
                Kind::Image,
            ),
            (b"GIF89a...", Kind::Image),
            (b"BM\x36\x00", Kind::Image),
            (b"RIFF\x00\x00\x00\x00AVI LIST", Kind::Media),
            (b"RIFF\x00\x00\x00\x00WAVEfmt ", Kind::Media),
            (b"\x00\x00\x00\x18ftypisom", Kind::Media),
            (&[0x1A, 0x45, 0xDF, 0xA3, 0x01], Kind::Media),
            (b"fLaC\x00\x00", Kind::Media),
            (b"ID3\x04\x00", Kind::Media),
        ];

        for (header, expected) in cases {
            assert_eq!(kind_from_magic(header), Some(expected), "for {header:?}");
        }
    }

    #[test]
    fn an_unrecognised_header_is_left_unanswered_rather_than_guessed_at() {
        // `None` means "no signature matched"; the caller turns that into
        // `Kind::Unknown`, which is what lets the info fallback speak about
        // the file instead of pretending to know its type (§7).
        assert_eq!(kind_from_magic(b"not a known format at all"), None);
        assert_eq!(kind_from_magic(&[0x00, 0x01, 0x02, 0x03]), None);
    }

    #[test]
    fn a_header_shorter_than_a_signature_is_not_guessed_at() {
        // `ftyp` sits at offset 4, so a two-byte file cannot contain it.
        assert_eq!(kind_from_magic(b"\x00\x00"), None);
        assert_eq!(kind_from_magic(b""), None);
    }

    #[test]
    fn a_file_with_no_extension_is_identified_by_its_contents() {
        let path = std::env::temp_dir().join(format!("toss-detect-{}.bin", std::process::id()));
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.resize(4096, 0);
        std::fs::write(&path, &bytes).expect("temp file written");

        let kind = classify(&Input::File(path.clone()));

        let _ = std::fs::remove_file(&path);
        assert_eq!(kind, Kind::Image);
    }

    #[test]
    fn sniffing_reads_a_header_only_never_the_whole_file() {
        // One megabyte, far larger than the sniff budget.
        let path = std::env::temp_dir().join(format!("toss-bounded-{}.bin", std::process::id()));
        std::fs::write(&path, vec![0u8; 1024 * 1024]).expect("temp file written");

        let mut buf = [0u8; SNIFF_LEN];
        let read = read_header(&path, &mut buf).expect("readable");

        let _ = std::fs::remove_file(&path);
        assert!(
            read <= SNIFF_LEN,
            "read {read} bytes, budget is {SNIFF_LEN}"
        );
    }

    #[test]
    fn an_unreadable_or_missing_file_degrades_to_unknown_rather_than_failing() {
        let missing = manifest("definitely-not-here-for-detection");
        assert_eq!(classify(&Input::File(missing)), Kind::Unknown);
    }

    #[test]
    fn unknown_inputs_name_no_action_so_the_report_can_be_about_the_file() {
        assert_eq!(Kind::Unknown.default_action(), None);
        assert_eq!(Kind::Archive.default_action(), Some("archive extraction"));
        assert_eq!(
            Kind::Directory.default_action(),
            Some("directory compression")
        );
    }
}
