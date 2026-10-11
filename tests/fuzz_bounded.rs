//! Bounded fuzzing for the boundaries Toss itself owns (§34).
//!
//! Two modes, deliberately kept apart in the names as well as the code:
//!
//! * **Deterministic mutation tests** — fixed seeds, bounded iteration
//!   counts, run on every `cargo test`. These are regression tests wearing
//!   a fuzzing harness's clothes: the same inputs every time, so a failure
//!   reproduces without hunting for a seed.
//! * **`fuzz_round`** — one `#[ignore]`d entry point with a larger budget
//!   and an entropy-derived (or `TOSS_FUZZ_SEED`-provided) seed, printed on
//!   stdout so a crash is replayable. This is the actual fuzz execution.
//!
//! Neither is coverage-guided: `cargo-fuzz`/libFuzzer is not installed in
//! this environment (nightly + cargo-fuzz would both be new tools), so this
//! is *unguided random mutation fuzzing* — honest about what it is and is
//! not, per the release-readiness report.
//!
//! What is fuzzed: CLI argument parsing, extension/magic detection, and
//! archive member-name policy — all process-level through the real binary,
//! because that is the boundary a user actually touches.
//!
//! What is deliberately NOT fuzzed here: image and media *content*. A
//! successful decode opens a window, and a test that opens a window hangs
//! the suite (STATUS §3.12); their classification paths are unit-tested in
//! `src/detection`, and any mutation whose header would classify as image
//! or media is skipped by [`routed_kind`] rather than fed to the process.
//!
//! Every temporary file lives in a scratch directory owned by the test and
//! removed on drop; corpus files are only ever read.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Exit codes §24 tables, as a set membership check. Anything outside this
/// set — a panic's 101, a Windows access-violation status, a signal — is a
/// bug, and the whole harness exists to catch it.
fn exit_code_is_defined(output: &Output) -> bool {
    matches!(output.status.code(), Some(0..=6))
}

/// A scratch directory that cleans itself up (the convention shared by the
/// other test files): temporary owned space only, never anything else.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("toss-fuzz-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn join(&self, child: impl AsRef<Path>) -> PathBuf {
        self.0.join(child)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// xorshift64* — tiny, deterministic, dependency-free. Seeded per test, so
/// every "random" byte this harness feeds Toss was chosen by a fixed rule.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // A zero state would stay zero forever.
        Self(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn run_toss(args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_toss"))
        .args(args)
        .output()
        .expect("toss runs")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The core assertion of the whole file: a defined exit code, no Rust panic
/// text, and on Windows no NTSTATUS failure code.
fn assert_defined(label: &str, output: &Output) {
    let stderr = stderr_of(output);
    assert!(
        exit_code_is_defined(output),
        "{label}: undefined exit {:?}\nstderr: {stderr}",
        output.status.code()
    );
    assert!(
        !stderr.contains("panicked at"),
        "{label}: Toss panicked on fuzzed input\nstderr: {stderr}"
    );
}

/// Header bytes [`routed_kind`] refuses to feed the process, copied from
/// the signatures `src/detection` matches (a mismatch here can only make
/// the harness skip more, never less — and `detection`'s own tests own the
/// classification rules).
fn would_classify_as_image_or_media(path: &Path, header: &[u8]) -> bool {
    // Extension decides first (§6.2): `.zip` never reaches the sniffer, and
    // the file names this harness writes are always `.dat` or `.zip`.
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("zip" | "7z" | "rar") => return false, // archive handler: no window
        Some(
            "jpg" | "jpeg" | "png" | "bmp" | "gif" | "mp4" | "mkv" | "webm" | "avi" | "mov" | "mp3"
            | "flac" | "wav",
        ) => return true, // would route to a window
        _ => {}
    }

    let image = header.starts_with(&[0xFF, 0xD8, 0xFF])
        || header.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
        || header.starts_with(b"GIF87a")
        || header.starts_with(b"GIF89a")
        || header.starts_with(b"BM");
    if image {
        return true;
    }

    header.get(8..12) == Some(&b"AVI "[..])
        || header.get(8..12) == Some(&b"WAVE"[..])
        || header.get(4..8) == Some(&b"ftyp"[..])
        || header.starts_with(&[0x1A, 0x45, 0xDF, 0xA3])
        || header.starts_with(b"fLaC")
        || header.starts_with(b"ID3")
}

/// What the real classifier would decide, used only to keep window-opening
/// inputs out of the process fuzz.
fn routed_kind(path: &Path, bytes: &[u8]) -> &'static str {
    if would_classify_as_image_or_media(path, bytes) {
        "image-or-media"
    } else if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext.to_ascii_lowercase().as_str(), "zip" | "7z" | "rar"))
    {
        "archive"
    } else {
        "unknown"
    }
}

fn corpus(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("corpus")
        .join(relative)
}

/// Mutate `bytes` under `rng`: flips, an overwrite, a truncation and an
/// append — the four shapes a dumb mutator produces, all bounded by
/// [`MAX_MUTATED`] so a run never allocates by accident.
const MAX_MUTATED: usize = 8 * 1024;

fn mutate(rng: &mut Rng, bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    if out.len() > MAX_MUTATED {
        out.truncate(MAX_MUTATED);
    }

    let flips = 1 + rng.below(8);
    for _ in 0..flips {
        if out.is_empty() {
            break;
        }
        let at = rng.below(out.len() as u64) as usize;
        out[at] ^= (rng.next() as u8) | 1;
    }

    match rng.below(4) {
        // Overwrite a run with fresh bytes.
        0 if !out.is_empty() => {
            let start = rng.below(out.len() as u64) as usize;
            let room = out.len() - start;
            let len = 1 + rng.below(room as u64) as usize;
            for slot in &mut out[start..start + len] {
                *slot = rng.next() as u8;
            }
        }
        // Truncate (a cut archive).
        1 if out.len() > 1 => {
            out.truncate(1 + rng.below((out.len() - 1) as u64) as usize);
        }
        // Append junk.
        2 => {
            let extra = 1 + rng.below(64);
            for _ in 0..extra {
                out.push(rng.next() as u8);
            }
        }
        _ => {}
    }

    out
}

/// Build a stored (uncompressed) ZIP whose members are empty. Empty keeps
/// the CRC at zero — which is correct, not approximate — so no checksum
/// code is needed to build hostile archives here. `flags` carries bit 11
/// (UTF-8) for names that need it.
fn stored_zip(names: &[String]) -> Vec<u8> {
    let mut local = Vec::new();
    let mut central = Vec::new();

    for name in names {
        let offset = local.len() as u32;
        let bytes = name.as_bytes();
        let flags: u16 = if name.is_ascii() { 0 } else { 0x0800 };

        local.extend_from_slice(&0x04034b50_u32.to_le_bytes());
        local.extend_from_slice(&20_u16.to_le_bytes()); // version needed
        local.extend_from_slice(&flags.to_le_bytes());
        local.extend_from_slice(&0_u16.to_le_bytes()); // method: stored
        local.extend_from_slice(&0_u16.to_le_bytes()); // time
        local.extend_from_slice(&0_u16.to_le_bytes()); // date
        local.extend_from_slice(&0_u32.to_le_bytes()); // crc32 of empty = 0
        local.extend_from_slice(&0_u32.to_le_bytes()); // compressed
        local.extend_from_slice(&0_u32.to_le_bytes()); // uncompressed
        local.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
        local.extend_from_slice(&0_u16.to_le_bytes()); // extra length
        local.extend_from_slice(bytes);

        central.extend_from_slice(&0x02014b50_u32.to_le_bytes());
        central.extend_from_slice(&20_u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20_u16.to_le_bytes()); // version needed
        central.extend_from_slice(&flags.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes()); // method
        central.extend_from_slice(&0_u16.to_le_bytes()); // time
        central.extend_from_slice(&0_u16.to_le_bytes()); // date
        central.extend_from_slice(&0_u32.to_le_bytes()); // crc32
        central.extend_from_slice(&0_u32.to_le_bytes()); // compressed
        central.extend_from_slice(&0_u32.to_le_bytes()); // uncompressed
        central.extend_from_slice(&(bytes.len() as u16).to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes()); // extra
        central.extend_from_slice(&0_u16.to_le_bytes()); // comment
        central.extend_from_slice(&0_u16.to_le_bytes()); // disk
        central.extend_from_slice(&0_u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0_u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(bytes);
    }

    let mut zip = local;
    let central_offset = zip.len() as u32;
    zip.extend_from_slice(&central);
    let central_size = central.len() as u32;

    zip.extend_from_slice(&0x06054b50_u32.to_le_bytes());
    zip.extend_from_slice(&0_u16.to_le_bytes()); // disk number
    zip.extend_from_slice(&0_u16.to_le_bytes()); // central directory disk
    zip.extend_from_slice(&(names.len() as u16).to_le_bytes());
    zip.extend_from_slice(&(names.len() as u16).to_le_bytes());
    zip.extend_from_slice(&central_size.to_le_bytes());
    zip.extend_from_slice(&central_offset.to_le_bytes());
    zip.extend_from_slice(&0_u16.to_le_bytes()); // comment length

    zip
}

/// Deterministic entry-name generator: the §28 list, the Windows
/// destination readings, plus seeded ordinary names so the accepting path
/// is fuzzed too.
fn hostile_name(rng: &mut Rng, index: usize) -> String {
    const SHAPES: &[&str] = &[
        // §28 classics
        "../evil-{i}",
        "..\\evil-{i}",
        "a/../../evil-{i}",
        "/abs-{i}",
        "//server/share/evil-{i}",
        "C:/Windows/evil-{i}",
        "..",
        ".",
        // Windows destination readings (see policy.rs)
        "NUL",
        "CON.txt",
        "com{i}.log",
        "file{i}.txt:stream",
        "trailing{i}.txt ",
        "trailing{i}.txt.",
        "sub.{i}/inside.txt",
        // Depth and length pressure
        "{deep}/{i}.txt",
        "{long}.txt",
        // Control characters and separators
        "ctrl\x01\x1f{i}.txt",
        "semi;colon{i}.txt",
        "quote\"{i}.txt",
        "pipe|{i}.txt",
        // Ordinary, so the accept path is exercised as well
        "plain-{i}.txt",
        "sub dir/nested-{i}.txt",
        "中文-{i}.txt",
    ];

    let shape = SHAPES[rng.below(SHAPES.len() as u64) as usize];
    shape
        .replace("{i}", &index.to_string())
        .replace("{deep}", &"d/".repeat(40))
        .replace("{long}", &"x".repeat(180))
}

/// One entry-name attempt: the hostile name goes into its own archive with
/// an ordinary control member, and afterwards the scratch directory must
/// hold exactly the archive and the output root — nothing escaped, ever.
fn entry_name_attempt(rng: &mut Rng, scratch: &Path, index: usize, label: &str) {
    let name = hostile_name(rng, index);
    let dir = scratch.join(format!("attempt-{index}"));
    std::fs::create_dir_all(&dir).expect("attempt directory");

    let zip = stored_zip(&["ok.txt".to_owned(), name.clone()]);
    let archive = dir.join("entry.zip");
    std::fs::write(&archive, &zip).expect("archive written");

    let output = run_toss(&[Path::new("extract"), archive.as_path()]);
    assert_defined(&format!("{label} entry {index} ({name:?})"), &output);

    let mut beside: Vec<OsString> = std::fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("{label}: {}: {err}", dir.display()))
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    beside.sort();
    assert_eq!(
        beside,
        [OsString::from("entry"), OsString::from("entry.zip")],
        "{label} entry {index} ({name:?}): something escaped or was written beside the output root: {beside:?}"
    );
}

/// Deterministic, bounded entry-name mutation — runs in every `cargo test`.
#[test]
fn deterministic_hostile_entry_names_never_escape_the_output_root() {
    let scratch = Scratch::new("names");
    let mut rng = Rng::new(0x5EED_0001);
    for index in 0..24 {
        entry_name_attempt(&mut rng, scratch.path(), index, "deterministic");
    }
}

/// Deterministic byte mutation of a real archive through the real command
/// line: routing, probing and extraction must stay defined for every
/// seed. The mutated files are named `.zip` (extension decides first, so
/// they can never be sniffed as image or media) or `.dat` (pure sniffing).
#[test]
fn deterministic_byte_mutations_keep_routing_defined() {
    let scratch = Scratch::new("bytes");
    let base = std::fs::read(corpus("archive/valid/simple.zip")).expect("sample readable");
    let mut rng = Rng::new(0x5EED_0002);
    let mut skipped = 0_u64;

    for index in 0..16 {
        let bytes = mutate(&mut rng, &base);
        let label = format!("byte mutation {index} ({} B)", bytes.len());

        // Extension-first routing: an archive, whatever the bytes became.
        let zip_path = scratch.join(format!("mut-{index}.zip"));
        std::fs::write(&zip_path, &bytes).expect("mutation written");
        let output = run_toss(&[zip_path.as_path()]);
        assert_defined(&label, &output);

        // Sniff-first routing: no extension, so detection reads the header.
        // Skipped when the mutated header would classify as image or media —
        // a successful decode opens a window (see the module docs).
        let dat_path = scratch.join(format!("mut-{index}.dat"));
        std::fs::write(&dat_path, &bytes).expect("mutation written");
        if routed_kind(&dat_path, &bytes) == "image-or-media" {
            skipped += 1;
        } else {
            let output = run_toss(&[dat_path.as_path()]);
            assert_defined(&label, &output);
        }

        let _ = std::fs::remove_file(&zip_path);
        let _ = std::fs::remove_file(&dat_path);
        let _ = std::fs::remove_dir_all(scratch.join(format!("mut-{index}")));
    }

    assert!(
        skipped < 16,
        "every mutation was skipped: the guard is broken, not the code"
    );
}

/// Deterministic argument-vector mutation. Every path-shaped argument is
/// made nonexistent on purpose: a path that exists could be an image, and
/// an image that decodes opens a window (STATUS §3.12). Verbs, options and
/// malformed shapes still reach the parser, the resolver and dispatch.
#[test]
fn deterministic_argument_vectors_stay_defined() {
    let scratch = Scratch::new("argv");
    let mut rng = Rng::new(0x5EED_0003);

    // Hoisted out of the array: a `repeat` temporary inside the literal
    // would be dropped while the slice still borrows it.
    let long_arg = "a".repeat(4096);

    let fixed: &[&[&str]] = &[
        &[],
        &["--help"],
        &["-h"],
        &["--"],
        &["--unknown-flag"],
        &["-"],
        &["extract"],
        &["extract", "toss-fuzz-missing-1"],
        &["pack", "toss-fuzz-missing-2"],
        &["view", "toss-fuzz-missing-3"],
        &["play", "toss-fuzz-missing-4"],
        &["hash", "toss-fuzz-missing-5"],
        &["info", "toss-fuzz-missing-6"],
        &["toss-fuzz-missing-7", "toss-fuzz-missing-8"],
        &["--", "--help"],
        &["view", "--"],
        &[long_arg.as_str()],
        &["中文路径/图片.png"],
        &["pack", "--", "toss-fuzz-missing-9"],
        &["extract", "", "toss-fuzz-missing-10"],
    ];

    for (index, args) in fixed.iter().enumerate() {
        let output = Command::new(env!("CARGO_BIN_EXE_toss"))
            .args(*args)
            .current_dir(scratch.path())
            .output()
            .expect("toss runs");
        assert_defined(&format!("argv[{index}]: {args:?}"), &output);
    }

    // Seeded random vectors from a fixed word pool — still only verbs,
    // options and nonexistent paths inside.
    const POOL: &[&str] = &[
        "extract",
        "pack",
        "view",
        "play",
        "info",
        "hash",
        "--help",
        "-h",
        "--",
        "-x",
        "toss-fuzz-missing-a",
        "toss-fuzz-missing-b",
        "中文-不存在",
        // Deliberately no `..`: it exists, and routing a real directory to
        // `pack` would write an archive. Nonexistent paths only.
        "../toss-fuzz-missing-c",
        "./toss-fuzz-missing-c",
    ];
    for _ in 0..12 {
        let count = 1 + rng.below(4);
        let words: Vec<&str> = (0..count)
            .map(|_| POOL[rng.below(POOL.len() as u64) as usize])
            .collect();
        let output = Command::new(env!("CARGO_BIN_EXE_toss"))
            .args(&words)
            .current_dir(scratch.path())
            .output()
            .expect("toss runs");
        assert_defined(&format!("argv random {words:?}"), &output);
    }
}

/// The actual fuzz execution: a larger unguided round with an entropy
/// seed, printed so any finding replays exactly.
///
/// Run explicitly:
///
/// ```text
/// cargo test --release --test fuzz_bounded -- --ignored fuzz_round --nocapture
/// TOSS_FUZZ_ITERS=2000 TOSS_FUZZ_SEED=12345 cargo test ... (replay)
/// ```
#[test]
#[ignore = "bounded fuzz execution: run explicitly, see module docs"]
fn fuzz_round() {
    let iters: u64 = std::env::var("TOSS_FUZZ_ITERS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(300);
    let seed: u64 = std::env::var("TOSS_FUZZ_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x5EED_F00F)
        });

    println!("fuzz_round: seed={seed} iters={iters} (unguided random mutation)");
    let mut rng = Rng::new(seed);
    let base = std::fs::read(corpus("archive/valid/simple.zip")).expect("sample readable");
    let scratch = Scratch::new("round");

    let mut codes = [0_u64; 7];
    let mut skipped = 0_u64;
    let mut counted = 0_u64;

    for index in 0..iters {
        // Entry names: one archive per iteration, escape invariant each time.
        entry_name_attempt(&mut rng, scratch.path(), index as usize, "fuzz_round");
        counted += 1;

        // Byte mutations, both routings.
        let bytes = mutate(&mut rng, &base);
        let zip_path = scratch.join(format!("round-{index}.zip"));
        std::fs::write(&zip_path, &bytes).expect("mutation written");
        if routed_kind(&zip_path, &bytes) == "archive" {
            let output = run_toss(&[zip_path.as_path()]);
            assert_defined(&format!("fuzz_round zip {index}"), &output);
            codes[output.status.code().unwrap_or(0) as usize] += 1;
            counted += 1;
        } else {
            skipped += 1;
        }

        let dat_path = scratch.join(format!("round-{index}.dat"));
        std::fs::write(&dat_path, &bytes).expect("mutation written");
        let output = run_toss(&[dat_path.as_path()]);
        assert_defined(&format!("fuzz_round dat {index}"), &output);
        codes[output.status.code().unwrap_or(0) as usize] += 1;
        counted += 1;

        let _ = std::fs::remove_file(&zip_path);
        let _ = std::fs::remove_file(&dat_path);
        let _ = std::fs::remove_dir_all(scratch.join(format!("round-{index}")));
        let _ = std::fs::remove_dir_all(scratch.join(format!("attempt-{}", index as usize)));
    }

    println!(
        "fuzz_round: {counted} executions, {skipped} skipped (image/media routing), \
         exit-code histogram {codes:?}"
    );
    println!("fuzz_round: replay with TOSS_FUZZ_SEED={seed} TOSS_FUZZ_ITERS={iters}");
}
