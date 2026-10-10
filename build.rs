//! Builds the vendored native libraries: zlib + liblzma + libarchive when
//! the `archive` feature is on, FFmpeg when the `media` feature is on and
//! the target is Windows (Slice 1 of the software HEVC decode core).
//!
//! Deliberately uses only `std::process` rather than a helper crate: the
//! project chose to vendor its native code so that release builds never
//! depend on network access or on whatever happens to be installed, and
//! pulling a crates.io build-dependency back in would undo that (§25).
//!
//! Without those features this script does nothing at all, so a default
//! `cargo build` neither compiles them nor pays for them (§14).
//!
//! Order matters: libarchive's own configuration asks where zlib and liblzma
//! are, so both must be built first. FFmpeg is independent of all three.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where native artifacts are kept, relative to the workspace root.
///
/// Outside `target/` on purpose: `cargo clean` must not throw them away, and
/// no other cache needs to know they exist.
const NATIVE_CACHE: &str = ".native-cache";

/// Configure flags that decide what zlib becomes. The install prefix is not
/// listed because it says *where* the result lands, not *what* it is.
const ZLIB_OPTIONS: &[&str] = &["-DBUILD_SHARED_LIBS=OFF", "-DZLIB_BUILD_EXAMPLES=OFF"];

/// Configure flags that decide what liblzma becomes.
const XZ_OPTIONS: &[&str] = &["-DBUILD_SHARED_LIBS=OFF"];

/// Configure flags that decide what libarchive becomes.
///
/// `CMAKE_BUILD_TYPE` is fixed at Release so the debug and release profiles
/// share one native build. The codec paths are excluded because they live in
/// `OUT_DIR` and differ per invocation while meaning the same thing.
const LIBARCHIVE_OPTIONS: &[&str] = &[
    "-DBUILD_SHARED_LIBS=OFF",
    "-DCMAKE_BUILD_TYPE=Release",
    // Everything below is switched off on purpose (§3): a format Toss does
    // not use still has to be configured, compiled and audited. Enabling a
    // format because libarchive happens to offer it is precisely what the
    // feature-admission test forbids (§37).
    "-DENABLE_TEST=OFF",
    "-DENABLE_TAR=OFF",
    "-DENABLE_CPIO=OFF",
    "-DENABLE_CAT=OFF",
    "-DENABLE_UNZIP=OFF",
    "-DENABLE_EXPAT=OFF",
    "-DENABLE_LIBXML2=OFF",
    "-DENABLE_WIN32_XMLLITE=OFF",
    "-DENABLE_OPENSSL=OFF",
    "-DENABLE_MBEDTLS=OFF",
    "-DENABLE_Nettle=OFF",
    "-DENABLE_CNG=OFF",
    "-DENABLE_ACL=OFF",
    "-DENABLE_XATTR=OFF",
    "-DENABLE_ICONV=OFF",
    "-DENABLE_LZO=OFF",
    "-DENABLE_LIBB2=OFF",
    "-DENABLE_LZ4=OFF",
    "-DENABLE_PCREPOSIX=OFF",
    "-DENABLE_PCRE2POSIX=OFF",
    "-DENABLE_BZip2=OFF",
    "-DENABLE_ZSTD=OFF",
    // Left ON on purpose. libarchive's CMakeLists spells the disabled branch
    // as a FATAL_ERROR saying libgcc not found — twice, at lines 1334 and
    // 1396 — so turning it off fails the configure.
    "-DENABLE_LIBGCC=ON",
    // The two codecs this milestone needs (§18.1): Deflate for ZIP entries,
    // LZMA/LZMA2 for 7z entries.
    "-DENABLE_ZLIB=ON",
    "-DENABLE_LZMA=ON",
];

/// Configure flags for the vendored FFmpeg — the whole of the software
/// HEVC capability, stated once (Slice 1; spike-measured in
/// TOSS_PROJECT_MASTER_CONTEXT §7.4–§7.6).
///
/// Deliberate omissions, each a ruling rather than an oversight:
///
/// - **no `--disable-asm`**: ASM is on (7.6-C ruling), so NASM is a hard
///   build requirement — the spike measured ~2× decode from it.
/// - **no `--enable-gpl` / `--enable-version3`**: GPL components stay off
///   by default (`THIRD_PARTY_NOTICES.md` §2); this closure is LGPL as
///   configured and needs nothing else.
/// - **no `--enable-nonfree`**: forbidden outright (`THIRD_PARTY_NOTICES.md`
///   §2) — and re-checked against `config.h` after every configure, below.
/// - **no swscale disable**: swscale is *on* (default) because BGRA is the
///   only pixel format Toss presents, and 365/366 of the corpus is 10-bit
///   Main10 — the conversion is the capability, not an extra.
const FFMPEG_OPTIONS: &[&str] = &[
    "--toolchain=msvc",
    "--disable-everything",
    "--disable-programs",
    "--disable-doc",
    "--disable-debug",
    "--disable-avdevice",
    "--disable-avfilter",
    "--disable-swresample",
    "--disable-network",
    "--enable-static",
    "--disable-shared",
    "--disable-zlib",
    "--disable-bzlib",
    "--disable-iconv",
    "--disable-libxml2",
    "--disable-xlib",
    "--disable-sdl2",
    "--enable-demuxer=matroska",
    "--enable-demuxer=mov",
    "--enable-parser=hevc",
    "--enable-decoder=hevc",
    "--enable-protocol=file",
];

fn main() {
    // Only these inputs may retrigger a rebuild. libarchive alone takes ~350s
    // to configure, so it must not rerun because an unrelated Rust file moved.
    println!("cargo:rerun-if-changed=third_party/libarchive");
    println!("cargo:rerun-if-changed=third_party/zlib");
    println!("cargo:rerun-if-changed=third_party/xz");
    println!("cargo:rerun-if-changed=third_party/ffmpeg");
    println!("cargo:rerun-if-changed=src/platform/windows/media/hevc_shim.c");
    println!("cargo:rerun-if-env-changed=CMAKE");

    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets this"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets this"));

    if env::var_os("CARGO_FEATURE_ARCHIVE").is_some() {
        let cache = manifest
            .join(NATIVE_CACHE)
            .join(native_fingerprint(&manifest));

        let (archive, zlib, lzma) = match restore(&out, &cache) {
            Some(libraries) => libraries,
            None => {
                let cmake = find_cmake();

                // zlib first, then liblzma: neither depends on the other, but
                // libarchive depends on both, so both exist before its configure.
                let zlib = build_zlib(&cmake, &manifest, &out);
                let lzma = build_lzma(&cmake, &manifest, &out);
                let archive = build_libarchive(&cmake, &manifest, &out, &zlib, &lzma);

                store(&out, &cache, [&archive, &zlib, &lzma]);
                (archive, zlib, lzma)
            }
        };

        // Order is deliberate: archive references the codecs, so it comes first.
        for library in [&archive, &zlib, &lzma] {
            println!(
                "cargo:rustc-link-search=native={}",
                library
                    .parent()
                    .expect("a library file has a parent")
                    .display()
            );
        }
        for library in [&archive, &zlib, &lzma] {
            println!("cargo:rustc-link-lib=static={}", link_name(library));
        }

        if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            // libarchive reaches for BCryptGenRandom on Windows even with CNG
            // off in some configurations, and crypt32 covers the certificate and
            // CAB paths. Naming them is free: msvc's linker only pulls what is
            // actually referenced.
            for system in ["bcrypt", "crypt32"] {
                println!("cargo:rustc-link-lib={system}");
            }
        }
    }

    // Slice 1 — the software HEVC decode core (docs/Phase76_Fallback_
    // Boundary_Design.md). The gates mirror `platform::windows::media`
    // exactly — Windows target plus `media` feature — plus the msvc target,
    // because that is the product's (§36). Linux and no-feature builds
    // never hear of FFmpeg at all.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var_os("CARGO_FEATURE_MEDIA").is_some()
    {
        build_ffmpeg(&manifest, &out);
    }
}

// ---------------------------------------------------------------------------
// Fingerprint
// ---------------------------------------------------------------------------

/// What the native libraries would have been built from.
///
/// `OUT_DIR` is deliberately absent: cargo hands every profile and every
/// invocation a different one, and keying on it would mean paying for a full
/// CMake build each time — the exact cost this exists to avoid.
fn native_fingerprint(manifest: &Path) -> String {
    let mut state = Fnv::new();

    state.feed(env::consts::OS.as_bytes());
    // /MT and /MD libraries must never share a cache: Rust's CRT choice
    // controls the CMake runtime below, even when sources are identical.
    state.feed(msvc_runtime().as_bytes());
    for name in [
        "CARGO_CFG_TARGET",
        "CARGO_CFG_TARGET_ARCH",
        "CARGO_CFG_TARGET_ENV",
    ] {
        state.feed(name.as_bytes());
        state.feed(env::var(name).unwrap_or_default().as_bytes());
    }

    for option in ZLIB_OPTIONS
        .iter()
        .chain(XZ_OPTIONS)
        .chain(LIBARCHIVE_OPTIONS)
    {
        state.feed(option.as_bytes());
    }

    // The native-build logic itself: changing how these are built must not
    // reuse artifacts built the old way.
    if let Ok(source) = fs::read(manifest.join("build.rs")) {
        state.feed(&source);
    }

    for library in ["zlib", "xz", "libarchive"] {
        state.feed(tree_hash(&manifest.join("third_party").join(library)).as_bytes());
    }

    state.feed(c_toolchain_fingerprint().as_bytes());

    format!("{:016x}", state.finish())
}

/// FNV-1a. std's `DefaultHasher` is explicitly unspecified across releases,
/// and a cache key that silently changes with the compiler is not a key.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn feed(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// Content hash of a directory: each relative name and its bytes, in a
/// stable order.
///
/// Content rather than mtime, because a fresh checkout rewrites timestamps
/// while leaving the bytes identical — keying on mtime would miss every clone.
fn tree_hash(root: &Path) -> String {
    let mut names = Vec::new();
    collect_names(root, root, &mut names);
    names.sort();

    let mut state = Fnv::new();
    for name in &names {
        state.feed(&(name.len() as u64).to_le_bytes());
        state.feed(name.as_bytes());

        let bytes = fs::read(root.join(name)).unwrap_or_default();
        state.feed(&(bytes.len() as u64).to_le_bytes());
        state.feed(&bytes);
    }

    format!("{:016x}", state.finish())
}

fn collect_names(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };

        if path.is_dir() {
            collect_names(root, &path, out);
            continue;
        }

        // Separators normalised so the hash does not depend on the platform.
        out.push(
            relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/"),
        );
    }
}

/// A coarse identity for the C toolchain: enough to notice a different
/// compiler or ABI generation, coarse enough not to invalidate on a patch
/// release. The libraries are C, and their ABI does not move with patches.
fn c_toolchain_fingerprint() -> String {
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let toolsets = msvc_toolsets();
        return if toolsets.is_empty() {
            "msvc:none".to_owned()
        } else {
            format!("msvc:{}", toolsets.join(","))
        };
    }

    let Some(machine) = cc_flag("-dumpmachine") else {
        return "cc:unknown".to_owned();
    };
    let Some(version) = cc_flag("-dumpversion") else {
        return format!("cc:{machine}:unknown");
    };

    let major = version.split('.').next().unwrap_or(&version);
    format!("cc:{machine}:{major}")
}

fn cc_flag(flag: &str) -> Option<String> {
    let output = Command::new("cc").arg(flag).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

/// MSVC toolsets visible in the usual install locations.
///
/// A targeted three-level walk rather than a general search: the layout is
/// `<root>/<edition>/<year>/VC/Tools/MSVC/<toolset>`, and a full search would
/// walk the whole Visual Studio tree for four directory names.
///
/// Only the major version is kept — `14.51.36231` and `14.50.33307` are the
/// same ABI generation, and keying on the patch number would drop the cache
/// every time Visual Studio updates.
fn msvc_toolsets() -> Vec<String> {
    let mut found = Vec::new();

    for root in [
        r"C:\Program Files\Microsoft Visual Studio",
        r"C:\Program Files (x86)\Microsoft Visual Studio",
        r"C:\BuildTools",
    ] {
        let Ok(editions) = fs::read_dir(root) else {
            continue;
        };

        for edition in editions.flatten() {
            let Ok(years) = fs::read_dir(edition.path()) else {
                continue;
            };

            for year in years.flatten() {
                let tools = year.path().join("VC").join("Tools").join("MSVC");
                let Ok(toolsets) = fs::read_dir(&tools) else {
                    continue;
                };

                for toolset in toolsets.flatten() {
                    let name = toolset.file_name().to_string_lossy().into_owned();
                    let major = name.split('.').next().unwrap_or(&name).to_owned();
                    found.push(major);
                }
            }
        }
    }

    found.sort();
    found.dedup();
    found
}

// ---------------------------------------------------------------------------
// Cache transfer
// ---------------------------------------------------------------------------

/// Copy a cached build into `out`, returning the three libraries.
///
/// Any shortfall yields `None` and the caller performs an ordinary build: a
/// damaged or foreign cache must never be able to break compilation.
fn restore(out: &Path, cache: &Path) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let manifest = fs::read_to_string(cache.join("manifest.txt")).ok()?;

    let mut libraries = Vec::new();
    for line in manifest.lines() {
        let relative = line.trim();
        if relative.is_empty() {
            continue;
        }

        let from = cache.join(relative);
        let to = out.join(relative);
        if !from.is_file() {
            return None;
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).ok()?;
        }
        fs::copy(&from, &to).ok()?;
        libraries.push(to);
    }

    if libraries.len() == 3 {
        Some((
            libraries[0].clone(),
            libraries[1].clone(),
            libraries[2].clone(),
        ))
    } else {
        None
    }
}

/// Copy the three libraries into the cache. Best effort: failing to cache a
/// build that already succeeded must not fail the build.
fn store(out: &Path, cache: &Path, libraries: [&Path; 3]) {
    let _ = fs::remove_dir_all(cache);
    let mut manifest = String::new();

    for library in libraries {
        let Ok(relative) = library.strip_prefix(out) else {
            // Not produced under OUT_DIR: nothing we can attribute to a key.
            return;
        };

        let destination = cache.join(relative);
        if let Some(parent) = destination.parent() {
            if fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        if fs::copy(library, &destination).is_err() {
            return;
        }

        manifest.push_str(&relative.to_string_lossy().replace('\\', "/"));
        manifest.push('\n');
    }

    // Written last: a directory without a manifest is never restored, so a
    // partial copy above leaves no cache rather than a broken one.
    if fs::write(cache.join("manifest.txt"), &manifest).is_err() {
        let _ = fs::remove_dir_all(cache);
    }
}

/// The name `cargo:rustc-link-lib` needs for a library we just located.
///
/// MSVC calls it `zlibstatic.lib` and GNU calls it `libz.a`, and the two
/// toolchains want different link names — hard-coding `zlibstatic` is what
/// made every Linux leg die with `cannot find -lzlibstatic` while Windows
/// passed. Deriving the name from the file actually found keeps both honest.
fn link_name(library: &Path) -> String {
    let name = library
        .file_name()
        .expect("a library file has a name")
        .to_string_lossy();

    if let Some(stem) = name.strip_suffix(".lib") {
        return stem.to_owned();
    }

    let stem = name.strip_prefix("lib").unwrap_or(&name);
    stem.strip_suffix(".a").unwrap_or(stem).to_owned()
}

/// Resolve the CMake executable: `CMAKE` wins, then `PATH`.
///
/// Presence is decided by actually running it rather than by guessing at
/// file names: the first attempt checked only for `cmake.exe` and therefore
/// declared CMake missing on Linux, where the binary is plain `cmake`.
fn find_cmake() -> String {
    if let Some(path) = env::var_os("CMAKE") {
        return path.to_string_lossy().into_owned();
    }

    let runs = Command::new("cmake")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);

    if runs {
        return "cmake".to_owned();
    }

    panic!(
        "\n\nCMake was not found, and libarchive needs it to build.\n\
         Install CMake and put it on PATH, or point at it directly:\n\n  \
         CMAKE=/path/to/cmake cargo build --features archive\n"
    );
}

fn build_zlib(cmake: &str, manifest: &Path, out: &Path) -> PathBuf {
    let source = manifest.join("third_party").join("zlib");
    let build = out.join("zlib-build");
    let native = out.join("native");

    discard_stale_prefix(&build, &native);
    configure(
        cmake,
        &source,
        &build,
        &[
            "-DBUILD_SHARED_LIBS=OFF",
            "-DZLIB_BUILD_EXAMPLES=OFF",
            &prefix_flag(&native),
        ],
    );
    // Built and installed as a pair rather than compiled straight from the
    // tree: `zconf.h` is *generated* during the build, so only installing
    // puts zlib.h and zconf.h side by side where libarchive can see them.
    // Pointing libarchive at the source tree alone is what failed first.
    build_all(cmake, &build, "zlib");
    install(cmake, &build, "zlib");

    locate_in(&native, &["zlibstatic.lib", "libz.a"])
}

fn build_lzma(cmake: &str, manifest: &Path, out: &Path) -> PathBuf {
    let source = manifest.join("third_party").join("xz");
    let build = out.join("xz-build");
    let native = out.join("native");

    discard_stale_prefix(&build, &native);
    configure(
        cmake,
        &source,
        &build,
        &["-DBUILD_SHARED_LIBS=OFF", &prefix_flag(&native)],
    );
    build_all(cmake, &build, "xz");
    install(cmake, &build, "xz");

    locate_in(&native, &["lzma.lib", "liblzma.a"])
}

/// Throw away a build directory whose cached install destinations are not ours.
///
/// CMake caches `INSTALL_LIB_DIR` and friends as absolute paths computed from
/// the prefix the *first* configure saw. Change the prefix later and those
/// keep pointing at the old destination even though `CMAKE_INSTALL_PREFIX`
/// itself already reads correctly — so `cmake --install` tries to write under
/// `C:/Program Files (x86)` and fails without administrator rights.
/// Reconfiguring does not recompute them, so the directory has to go.
///
/// Only absolute destinations are treated as evidence: relative ones such as
/// `lib` are correct wherever the prefix points.
///
/// A build directory configured with the right prefix from the start never
/// triggers this; it exists for trees that predate a prefix change.
fn discard_stale_prefix(build: &Path, native: &Path) {
    let cache = build.join("CMakeCache.txt");
    let Ok(text) = std::fs::read_to_string(&cache) else {
        return;
    };

    let wanted = native.display().to_string().replace('\\', "/");
    let stale = text.lines().any(|line| {
        let Some(rest) = line.strip_prefix("INSTALL_") else {
            return false;
        };
        let Some(value) = rest.split_once(":PATH=").map(|(_, value)| value.trim()) else {
            return false;
        };

        let absolute = value.starts_with('/') || value.chars().nth(1) == Some(':');
        absolute && !value.starts_with(&wanted)
    });

    if stale {
        let _ = std::fs::remove_dir_all(build);
    }
}

/// The install destination has to be fixed at configure time: passing
/// `--prefix` to `cmake --install` was silently ignored here and the install
/// fell back to `C:/Program Files (x86)`, which needs administrator rights.
fn prefix_flag(native: &Path) -> String {
    format!(
        "-DCMAKE_INSTALL_PREFIX={}",
        native.display().to_string().replace('\\', "/")
    )
}

fn build_libarchive(cmake: &str, manifest: &Path, out: &Path, zlib: &Path, lzma: &Path) -> PathBuf {
    let source = manifest.join("third_party").join("libarchive");
    let build = out.join("libarchive-build");

    // Both codecs install their headers into one shared native prefix, so a
    // single include directory serves them. Deriving this from a library's
    // own location instead would point at a build directory that never
    // contains them, which is exactly how the first attempt failed on a
    // clean configure.
    let native_include = out.join("native").join("include");

    configure(
        cmake,
        &source,
        &build,
        &[
            "-DBUILD_SHARED_LIBS=OFF",
            "-DCMAKE_BUILD_TYPE=Release",
            // Everything below is switched off on purpose (§3): a format Toss
            // does not use still has to be configured, compiled and audited.
            // Enabling a format because libarchive happens to offer it is
            // precisely what the feature-admission test forbids (§37).
            "-DENABLE_TEST=OFF",
            "-DENABLE_TAR=OFF",
            "-DENABLE_CPIO=OFF",
            "-DENABLE_CAT=OFF",
            "-DENABLE_UNZIP=OFF",
            "-DENABLE_EXPAT=OFF",
            "-DENABLE_LIBXML2=OFF",
            "-DENABLE_WIN32_XMLLITE=OFF",
            "-DENABLE_OPENSSL=OFF",
            "-DENABLE_MBEDTLS=OFF",
            "-DENABLE_Nettle=OFF",
            "-DENABLE_CNG=OFF",
            "-DENABLE_ACL=OFF",
            "-DENABLE_XATTR=OFF",
            "-DENABLE_ICONV=OFF",
            "-DENABLE_LZO=OFF",
            "-DENABLE_LIBB2=OFF",
            "-DENABLE_LZ4=OFF",
            "-DENABLE_PCREPOSIX=OFF",
            "-DENABLE_PCRE2POSIX=OFF",
            "-DENABLE_BZip2=OFF",
            "-DENABLE_ZSTD=OFF",
            // Left ON on purpose. libarchive's CMakeLists spells the disabled
            // branch as a FATAL_ERROR saying libgcc not found — twice, at lines
            // 1334 and 1396 — so turning it off fails the configure.
            "-DENABLE_LIBGCC=ON",
            // The two codecs this milestone actually needs (§18.1): Deflate
            // for ZIP entries, LZMA/LZMA2 for 7z entries. Pointed at the
            // vendored static libraries so no import library and no DLL is
            // ever pulled in (§44).
            "-DENABLE_ZLIB=ON",
            "-DENABLE_LZMA=ON",
            &flag("ZLIB_INCLUDE_DIR", &native_include),
            &flag("ZLIB_LIBRARY", zlib),
            &flag("LIBLZMA_INCLUDE_DIR", &native_include),
            &flag("LIBLZMA_LIBRARY", lzma),
        ],
    );
    target(cmake, &build, "archive_static");

    locate(
        &build,
        &[
            "libarchive/Release/archive.lib",
            "libarchive/archive.lib",
            "libarchive/libarchive.a",
            "archive.lib",
        ],
    )
}

fn configure(cmake: &str, source: &Path, build: &Path, options: &[&str]) {
    let mut command = Command::new(cmake);
    command.arg("-S").arg(source).arg("-B").arg(build);
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        command.arg("-DCMAKE_POLICY_DEFAULT_CMP0091=NEW");
        command.arg(format!("-DCMAKE_MSVC_RUNTIME_LIBRARY={}", msvc_runtime()));
        // libarchive also has an older runtime-selection helper; keep its
        // switch consistent with CMake's runtime property, without editing
        // the vendored source or imposing that option on other projects.
        if source.file_name().is_some_and(|name| name == "libarchive") {
            command.arg(if msvc_runtime() == "MultiThreaded" {
                "-DMSVC_USE_STATIC_CRT=ON"
            } else {
                "-DMSVC_USE_STATIC_CRT=OFF"
            });
        }
    }
    // CMake reads a bare backslash as an escape, so every path we hand it is
    // rewritten to forward slashes. On POSIX this is a no-op.
    for option in options {
        command.arg(option.replace('\\', "/"));
    }

    run(&mut command, "configure");
}

/// Native libraries are always built in Release, including for Rust tests.
/// Match Cargo's selected CRT so a static Rust EXE never imports a dynamic
/// C heap through libarchive, zlib or liblzma.
fn msvc_runtime() -> &'static str {
    if env::var("CARGO_CFG_TARGET_FEATURE")
        .unwrap_or_default()
        .split(',')
        .any(|feature| feature == "crt-static")
    {
        "MultiThreaded"
    } else {
        "MultiThreadedDLL"
    }
}

fn target(cmake: &str, build: &Path, name: &str) {
    let mut command = Command::new(cmake);
    command
        .arg("--build")
        .arg(build)
        .arg("--config")
        .arg("Release")
        .arg("--target")
        .arg(name);

    run(&mut command, name);
}

/// Build every target a project defines.
///
/// Used for the two codec libraries rather than naming one target, because
/// their install rules refer to more than what a single target produces.
fn build_all(cmake: &str, build: &Path, stage: &str) {
    let mut command = Command::new(cmake);
    command
        .arg("--build")
        .arg(build)
        .arg("--config")
        .arg("Release");

    run(&mut command, &format!("{stage} build"));
}

/// Copy headers and libraries into the shared prefix fixed at configure time.
///
/// This is what makes `zconf.h` and `lzma.h` reachable: they are generated
/// during the build, so they exist only after an install.
fn install(cmake: &str, build: &Path, stage: &str) {
    let mut command = Command::new(cmake);
    command
        .arg("--install")
        .arg(build)
        .arg("--config")
        .arg("Release");

    run(&mut command, &format!("{stage} install"));
}

/// Find a library inside the shared native prefix.
fn locate_in(native: &Path, names: &[&str]) -> PathBuf {
    for directory in ["lib", "lib64", ""] {
        for name in names {
            let path = if directory.is_empty() {
                native.join(name)
            } else {
                native.join(directory).join(name)
            };
            if path.is_file() {
                return path;
            }
        }
    }

    panic!(
        "\n\nA vendored library installed but no static library was found in {}.\n\
         Looked for: {}\n",
        native.display(),
        names.join(", ")
    );
}

/// Find a built library wherever the generator chose to put it.
///
/// Visual Studio writes `<build>/Release/foo.lib` while Ninja and Makefiles
/// write `<build>/foo.lib`, and hard-coding either would break the other CI
/// leg. Callers may pass nested paths for targets that live in a subdirectory.
fn locate(build: &Path, candidates: &[&str]) -> PathBuf {
    for candidate in candidates {
        for directory in [
            build.join("Release"),
            build.to_path_buf(),
            build.join("libarchive").join("Release"),
            build.join("libarchive"),
        ] {
            let path = directory.join(candidate);
            if path.is_file() {
                return path;
            }
            // POSIX static libraries carry a `lib` prefix and a `.a` suffix.
            let name = Path::new(candidate)
                .file_name()
                .expect("a candidate has a file name")
                .to_string_lossy();
            if let Some(stem) = name.strip_suffix(".lib") {
                let path = directory.join(format!("lib{stem}.a"));
                if path.is_file() {
                    return path;
                }
            }
        }
    }

    panic!(
        "\n\nA vendored library built but no static library was found. Looked for:\n{}\n",
        candidates
            .iter()
            .map(|name| format!("  {name}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn flag(name: &str, value: &Path) -> String {
    format!(
        "-D{name}={}",
        value.display().to_string().replace('\\', "/")
    )
}

fn run(command: &mut Command, stage: &str) {
    let status = command
        .status()
        .unwrap_or_else(|err| panic!("failed to launch {command:?}: {err}"));

    if !status.success() {
        panic!("vendored build step '{stage}' failed with {status}");
    }
}

// ---------------------------------------------------------------------------
// FFmpeg — software HEVC decode (Slice 1)
// ---------------------------------------------------------------------------

/// Build the vendored FFmpeg (or reuse its cache) and emit its link lines.
///
/// The pipeline mirrors the spike that proved it (7.6-B): capture the MSVC
/// environment once, let an MSYS2 shell run `configure` + `make`
/// out-of-tree, compile Toss's one-file shim with `cl`, check the license
/// invariants in `config.h`, then park the five libraries in
/// `.native-cache`. A cache hit skips every process spawn — no vcvars, no
/// MSYS2, no NASM — and rustc links straight from the cache directory.
fn build_ffmpeg(manifest: &Path, out: &Path) {
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        panic!(
            "\n\nThe `media` feature's FFmpeg build supports only the msvc target\n\
             (the product's target, §36). Build with --target x86_64-pc-windows-msvc.\n"
        );
    }

    let fingerprint = ffmpeg_fingerprint(manifest);
    let cache = manifest
        .join(NATIVE_CACHE)
        .join(format!("{fingerprint}-ffmpeg"));
    let cached = [
        "toss_shim.lib",
        "avformat.lib",
        "avcodec.lib",
        "avutil.lib",
        "swscale.lib",
    ];

    if ffmpeg_cache_usable(&cache, &cached) {
        emit_ffmpeg_links(&cache);
        return;
    }

    let vcvars = find_vcvarsall();
    let env_map = vcvars_env(&vcvars);
    let bash = find_bash();
    let nasm_dir = find_nasm_dir().unwrap_or_else(|| {
        panic!(
            "\n\nNASM was not found, and the FFmpeg build keeps ASM on (7.6-C ruling).\n\
             Install NASM and put it on PATH — for example:\n\n  \
             winget install NASM.NASM\n\n\
             or point TOSS_NASM at the directory holding nasm.exe.\n"
        )
    });

    let build = out.join("ffmpeg-build");
    fs::create_dir_all(&build).unwrap_or_else(|err| {
        panic!("could not create {}: {err}", build.display());
    });
    let source = manifest.join("third_party").join("ffmpeg");
    let jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // PATH for the msys side: NASM's directory first (winget installs it
    // outside PATH), then MSYS2's own usr/bin (make, sed, grep, sh — the
    // shell the spike ran in), then ucrt64/bin if present, then whatever
    // vcvars contributed (cl, lib, link, INCLUDE, LIB ride along too).
    let mut path = nasm_dir.display().to_string();
    if let Some(usr_bin) = bash.parent() {
        path.push(';');
        path.push_str(&usr_bin.display().to_string());
        if let Some(msys_root) = usr_bin.parent().and_then(Path::parent) {
            let ucrt = msys_root.join("ucrt64").join("bin");
            if ucrt.is_dir() {
                path.push(';');
                path.push_str(&ucrt.display().to_string());
            }
        }
    }
    path.push(';');
    path.push_str(env_map.get("PATH").map(String::as_str).unwrap_or(""));

    // Out-of-tree configure: the source tree stays verbatim (UPSTREAM.md),
    // everything generated lands in build/. `command -v make` first so a
    // runner without MSYS2's make says so instead of dying mid-configure.
    let script = format!(
        "set -eu\n\
         command -v make >/dev/null 2>&1 || \
         {{ echo 'MSYS2 make not found — install it: pacman -S make' >&2; exit 127; }}\n\
         cd '{build}'\n\
         '{source}/configure' {options}\n\
         make -j{jobs}\n",
        build = windows_to_posix(&build),
        source = windows_to_posix(&source),
        options = FFMPEG_OPTIONS.join(" "),
    );

    let mut shell = Command::new(&bash);
    shell.arg("-c").arg(&script);
    shell.envs(&env_map);
    shell.env("PATH", &path);
    run(&mut shell, "FFmpeg configure + make");

    // Build-time license invariant (THIRD_PARTY_NOTICES.md §2): GPL
    // components off, nonfree forbidden. Checked against config.h — the
    // artifact configure actually produced — rather than against the flags
    // we *think* we passed.
    assert_ffmpeg_license(&build.join("config.h"));

    // The one Toss-owned C file, compiled against the freshly configured
    // headers with the same MSVC environment the libraries just used.
    let shim = manifest
        .join("src")
        .join("platform")
        .join("windows")
        .join("media")
        .join("hevc_shim.c");
    let obj = build.join("toss_shim.obj");
    let shim_lib = build.join("toss_shim.lib");
    let cl = find_tool_in_env(&env_map, "cl.exe").unwrap_or_else(|| {
        panic!("vcvars did not put cl.exe on PATH: {}", vcvars.display());
    });
    let lib_tool = find_tool_in_env(&env_map, "lib.exe").unwrap_or_else(|| {
        panic!("vcvars did not put lib.exe on PATH: {}", vcvars.display());
    });

    let mut compile = Command::new(&cl);
    compile
        // `/utf-8` matches the flag FFmpeg's own MSVC build passes to cl.
        .args(["/nologo", "/O2", "/W3", "/utf-8", "/c"])
        .arg(&shim)
        .arg(format!("/Fo{}", obj.display()))
        // Two roots: the vendored headers, plus the *generated* ones —
        // `libavutil/avconfig.h` exists only in the configure output tree
        // (out-of-tree build; the spike was in-tree and never needed this).
        .arg(format!("/I{}", source.display()))
        .arg(format!("/I{}", build.display()))
        .envs(&env_map);
    run(&mut compile, "FFmpeg shim compile");

    let mut archive = Command::new(&lib_tool);
    archive
        .arg("/nologo")
        .arg(format!("/OUT:{}", shim_lib.display()))
        .arg(&obj)
        .envs(&env_map);
    run(&mut archive, "FFmpeg shim lib");

    store_ffmpeg(
        &cache,
        [
            build.join("libavformat").join("avformat.lib"),
            build.join("libavcodec").join("avcodec.lib"),
            build.join("libavutil").join("avutil.lib"),
            build.join("libswscale").join("swscale.lib"),
            shim_lib,
        ],
    );

    emit_ffmpeg_links(&cache);
}

/// What the FFmpeg build would have been built from (mirrors
/// `native_fingerprint`, with FFmpeg's own inputs and two extra tool
/// identities: NASM decides whether asm objects exist at all).
fn ffmpeg_fingerprint(manifest: &Path) -> String {
    let mut state = Fnv::new();

    state.feed(env::consts::OS.as_bytes());
    for name in [
        "CARGO_CFG_TARGET",
        "CARGO_CFG_TARGET_ARCH",
        "CARGO_CFG_TARGET_ENV",
    ] {
        state.feed(name.as_bytes());
        state.feed(env::var(name).unwrap_or_default().as_bytes());
    }

    for option in FFMPEG_OPTIONS {
        state.feed(option.as_bytes());
    }

    // The native-build logic itself: changing how this builds must not
    // reuse artifacts built the old way.
    if let Ok(source) = fs::read(manifest.join("build.rs")) {
        state.feed(&source);
    }
    if let Ok(source) = fs::read(
        manifest
            .join("src")
            .join("platform")
            .join("windows")
            .join("media")
            .join("hevc_shim.c"),
    ) {
        state.feed(&source);
    }

    state.feed(tree_hash(&manifest.join("third_party").join("ffmpeg")).as_bytes());
    state.feed(c_toolchain_fingerprint().as_bytes());
    state.feed(nasm_fingerprint().as_bytes());

    format!("{:016x}", state.finish())
}

/// Whether a complete, previously stored FFmpeg cache is present. Written
/// manifest-last by `store_ffmpeg`, so a partial cache never restores —
/// the same discipline `restore` relies on.
fn ffmpeg_cache_usable(cache: &Path, names: &[&str]) -> bool {
    cache.join("manifest.txt").is_file() && names.iter().all(|name| cache.join(name).is_file())
}

/// Park the five libraries in the cache. Unlike `store`, this fails hard:
/// the link lines point at the cache, so a cache that could not be written
/// must stop the build with a real message rather than surface as a
/// mysterious rustc error later.
fn store_ffmpeg(cache: &Path, libraries: [PathBuf; 5]) {
    let _ = fs::remove_dir_all(cache);
    fs::create_dir_all(cache)
        .unwrap_or_else(|err| panic!("could not create {}: {err}", cache.display()));

    let mut manifest = String::new();
    for library in &libraries {
        let name = library
            .file_name()
            .expect("a library file has a name")
            .to_string_lossy()
            .into_owned();
        fs::copy(library, cache.join(&name)).unwrap_or_else(|err| {
            panic!("could not cache {}: {err}", library.display());
        });
        manifest.push_str(&name);
        manifest.push('\n');
    }

    fs::write(cache.join("manifest.txt"), manifest)
        .unwrap_or_else(|err| panic!("could not write cache manifest: {err}"));
}

/// Emit the link lines for a usable FFmpeg cache.
///
/// Library order is deliberate: every dependency appears *after* the
/// dependant that needs it (shim → avformat → swscale → avcodec → avutil),
/// which is the order both spike linkers accepted.
fn emit_ffmpeg_links(cache: &Path) {
    println!("cargo:rustc-link-search=native={}", cache.display());
    for library in ["toss_shim", "avformat", "swscale", "avcodec", "avutil"] {
        println!("cargo:rustc-link-lib=static={library}");
    }
    // From FFmpeg's own EXTRALIBS for this configuration: avcodec wants
    // ole32 and user32, avutil wants user32 and bcrypt. Naming them is
    // free — the linker pulls only what is referenced (same reasoning as
    // the archive block above).
    for system in ["ole32", "user32", "bcrypt"] {
        println!("cargo:rustc-link-lib={system}");
    }
}

/// The license invariant, read from configure's own output
/// (`THIRD_PARTY_NOTICES.md` §2): GPL components off, nonfree forbidden.
fn assert_ffmpeg_license(config_h: &Path) {
    let text = fs::read_to_string(config_h).unwrap_or_else(|err| {
        panic!(
            "FFmpeg configured but {} is unreadable: {err}",
            config_h.display()
        );
    });

    let gpl_on = text.contains("#define CONFIG_GPL 1");
    let nonfree_on = text.contains("#define CONFIG_NONFREE 1");
    let both_declared =
        text.contains("#define CONFIG_GPL 0") && text.contains("#define CONFIG_NONFREE 0");

    if gpl_on || nonfree_on || !both_declared {
        panic!(
            "\n\nFFmpeg's build-time license invariant failed — see\n\
             THIRD_PARTY_NOTICES.md §2:\n\n  \
             GPL components: {}  (must be off)\n  \
             nonfree:        {}  (forbidden)\n\n\
             config.h: {}\n",
            if gpl_on { "ENABLED" } else { "off" },
            if nonfree_on { "ENABLED" } else { "off" },
            config_h.display(),
        );
    }
}

/// `vcvarsall.bat`'s install root, via vswhere first (any edition, any
/// year) and a bounded walk as fallback for machines without the installer.
fn find_vcvarsall() -> PathBuf {
    const VSWHERE: &str = r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe";

    if Path::new(VSWHERE).is_file() {
        if let Some(output) = Command::new(VSWHERE)
            .args([
                "-latest",
                "-products",
                "*",
                "-requires",
                "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                "-property",
                "installationPath",
            ])
            .output()
            .ok()
            .filter(|output| output.status.success())
        {
            let install = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !install.is_empty() {
                let candidate = Path::new(&install)
                    .join("VC")
                    .join("Auxiliary")
                    .join("Build")
                    .join("vcvarsall.bat");
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }

    for root in [
        r"C:\Program Files\Microsoft Visual Studio",
        r"C:\Program Files (x86)\Microsoft Visual Studio",
        r"C:\BuildTools",
    ] {
        let Ok(editions) = fs::read_dir(root) else {
            continue;
        };
        for edition in editions.flatten() {
            let Ok(years) = fs::read_dir(edition.path()) else {
                continue;
            };
            for year in years.flatten() {
                let candidate = year
                    .path()
                    .join("VC")
                    .join("Auxiliary")
                    .join("Build")
                    .join("vcvarsall.bat");
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }

    panic!(
        "\n\nvcvarsall.bat was not found, and the FFmpeg build compiles with\n\
         MSVC (the product's toolchain, §36). Install Visual Studio Build\n\
         Tools with the C++ workload, or run from a Developer prompt.\n"
    );
}

/// Run `vcvarsall x64` and capture the environment it produces.
///
/// Deliberately *no hand-written quotes*: vcvarsall is passed as its own
/// argument, so the only quotes on the command line are the automatic ones
/// around a path containing spaces — cmd handles those cleanly. Building
/// `call "..." x64 && set` as one string is the classic cmd quoting trap:
/// Rust escapes the nested quotes with backslashes that cmd does not
/// understand, the path mangles, and vcvarsall dies with exit 1. vcvars'
/// banner lines are not `KEY=VALUE` and simply fall out of the parser.
fn vcvars_env(vcvars: &Path) -> HashMap<String, String> {
    let output = Command::new("cmd")
        .arg("/c")
        .arg(vcvars)
        .arg("x64")
        .arg("&&")
        .arg("set")
        .output()
        .unwrap_or_else(|err| panic!("failed to launch cmd for vcvars: {err}"));

    if !output.status.success() {
        panic!(
            "vcvarsall failed with {} while preparing the FFmpeg build",
            output.status
        );
    }

    let mut map = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some((key, value)) = line.split_once('=') {
            if !key.is_empty() && !key.contains(['[', ']', ' ']) {
                // Windows environment names are case-insensitive and cmd
                // prints `Path`, not `PATH`: normalising the key here is
                // what keeps every lookup below one spelling.
                map.insert(key.to_ascii_uppercase(), value.to_owned());
            }
        }
    }
    if !map.contains_key("PATH") {
        panic!("vcvars produced no PATH — cannot place cl.exe or the msys tools");
    }
    map
}

/// Find an executable inside a captured environment's PATH. Resolved here
/// rather than left to `Command::new(name)` so the child never depends on
/// which PATH CreateProcess happens to consult.
fn find_tool_in_env(env_map: &HashMap<String, String>, name: &str) -> Option<PathBuf> {
    let path = env_map.get("PATH")?;
    for dir in env::split_paths(path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// MSYS2's bash, by explicit location. A PATH scan is deliberately absent:
/// `C:\Windows\System32\bash.exe` is *WSL*, and running an msys build
/// script under it fails in ways that look like configure bugs.
fn find_bash() -> PathBuf {
    if let Some(path) = env::var_os("TOSS_BASH") {
        return PathBuf::from(path);
    }

    let candidate = PathBuf::from(r"C:\msys64\usr\bin\bash.exe");
    if candidate.is_file() {
        return candidate;
    }

    panic!(
        "\n\nMSYS2's bash was not found at {}, and FFmpeg's configure is a\n\
         POSIX shell script (7.6-C measured this: VS Build Tools alone is\n\
         not enough — sh and GNU make are hard requirements). Install MSYS2\n\
         there, or set TOSS_BASH to your bash.exe. WSL's bash will not do.\n",
        candidate.display()
    );
}

/// The directory holding `nasm.exe`: PATH first, then the two places the
/// known installers put it. `None` means "not installed" — the caller
/// decides whether that is fatal (it is, while ASM stays on).
fn find_nasm_dir() -> Option<PathBuf> {
    if let Some(path) = env::var_os("TOSS_NASM") {
        let dir = PathBuf::from(path);
        if dir.join("nasm.exe").is_file() {
            return Some(dir);
        }
    }

    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            if dir.join("nasm.exe").is_file() {
                return Some(dir);
            }
        }
    }

    let local = env::var_os("LOCALAPPDATA")
        .map(|dir| PathBuf::from(dir).join("bin").join("NASM"))
        .unwrap_or_default();
    [local, PathBuf::from(r"C:\Program Files\NASM")]
        .into_iter()
        .find(|dir| dir.join("nasm.exe").is_file())
}

/// NASM's identity for the fingerprint — asm on with NASM X and asm on
/// with NASM Y are not the same build. `nasm:none` covers the fingerprint
/// computed on a machine that cannot build (the build step reports that
/// properly, with an install hint).
fn nasm_fingerprint() -> String {
    let Some(dir) = find_nasm_dir() else {
        return "nasm:none".to_owned();
    };

    Command::new(dir.join("nasm.exe"))
        .arg("-v")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_else(|| "nasm:unknown".to_owned())
}

/// A Windows path as an msys shell wants it: `C:\x\y` → `/c/x/y`.
/// Already-POSIX paths pass through unchanged.
fn windows_to_posix(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if let Some((drive, rest)) = text.split_once(':') {
        if drive.len() == 1
            && drive
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
        {
            return format!("/{}{}", drive.to_lowercase(), rest);
        }
    }
    text
}
