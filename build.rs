//! Builds the vendored libarchive when the `archive` feature is on.
//!
//! Deliberately uses only `std::process` rather than a helper crate: the
//! project chose to vendor its native code so that release builds never
//! depend on network access or on whatever happens to be installed, and
//! pulling a crates.io build-dependency back in would undo that (§25).
//!
//! Without the `archive` feature this script does nothing at all, so a
//! default `cargo build` neither compiles libarchive nor pays for it (§14).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // Only these two inputs may retrigger a rebuild. libarchive takes ~200s
    // to configure, so it must not rerun because an unrelated Rust file moved.
    println!("cargo:rerun-if-changed=third_party/libarchive");
    println!("cargo:rerun-if-env-changed=CMAKE");

    if env::var_os("CARGO_FEATURE_ARCHIVE").is_none() {
        return;
    }

    let source = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets this"))
        .join("third_party")
        .join("libarchive");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets this"));
    let build = out.join("libarchive-build");

    let cmake = find_cmake();
    configure(&cmake, &source, &build);
    compile(&cmake, &build);

    let library = locate_archive_library(&build);
    println!(
        "cargo:rustc-link-search=native={}",
        library
            .parent()
            .expect("a library file has a parent")
            .display()
    );
    println!("cargo:rustc-link-lib=static=archive");

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

/// Resolve the CMake executable: `CMAKE` wins, then `PATH`.
///
/// The failure message is written for a developer rather than a log file,
/// because the only fix is installing CMake or pointing at it.
fn find_cmake() -> String {
    if let Some(path) = env::var_os("CMAKE") {
        return path.to_string_lossy().into_owned();
    }

    if which("cmake").is_some() {
        return "cmake".to_owned();
    }

    panic!(
        "\n\nCMake was not found, and libarchive needs it to build.\n\
         Install CMake and put it on PATH, or point at it directly:\n\n  \
         CMAKE=\"C:/path/to/cmake.exe\" cargo build --features archive\n"
    );
}

fn which(program: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        let candidate = directory.join(format!("{program}.exe"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn configure(cmake: &str, source: &Path, build: &Path) {
    let mut command = Command::new(cmake);
    command
        .arg("-S")
        .arg(source)
        .arg("-B")
        .arg(build)
        // Static, and into OUT_DIR rather than TEMP: MSVC warns that a build
        // tree under TEMP breaks incremental builds (MSB8029).
        .arg("-DBUILD_SHARED_LIBS=OFF")
        .arg("-DCMAKE_BUILD_TYPE=Release")
        // Everything below is switched off on purpose (§3): a format Toss
        // does not use still has to be configured, compiled and audited.
        // Enabling a format because libarchive happens to offer it is
        // precisely what the feature-admission test forbids (§37).
        .args([
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
            // Left ON on purpose. libarchive's CMakeLists spells the
            // disabled branch as `ELSE() MESSAGE(FATAL_ERROR "libgcc not
            // found.")` — two places, lines 1334 and 1396 — so turning it
            // off fails the configure. Enabling it runs FindLIBGCC instead,
            // which merely reports NOTFOUND and lets the PCRE guards above
            // take it from there.
            "-DENABLE_LIBGCC=ON",
            // Not available on a bare Windows box, and turning them on makes
            // CMake search PATH-derived prefixes — which on this project's
            // dev machine found MSYS2 headers and fed them to MSVC.
            "-DENABLE_ZLIB=OFF",
            "-DENABLE_BZip2=OFF",
            "-DENABLE_LZMA=OFF",
            "-DENABLE_ZSTD=OFF",
        ]);

    run(&mut command, "configure");
}

fn compile(cmake: &str, build: &Path) {
    let mut command = Command::new(cmake);
    command
        .arg("--build")
        .arg(build)
        .arg("--config")
        .arg("Release")
        // Only the library: the bundled `bsdunzip` tool links without the
        // system libraries it needs and is no use to Toss anyway.
        .arg("--target")
        .arg("archive_static");

    run(&mut command, "build");
}

/// Find the static library wherever the generator chose to put it.
///
/// The Visual Studio generator writes `Release\archive.lib` while Ninja
/// writes `archive.lib`, and hard-coding either would break the other CI leg.
fn locate_archive_library(build: &Path) -> PathBuf {
    let candidates = [
        build.join("libarchive").join("Release").join("archive.lib"),
        build.join("libarchive").join("archive.lib"),
        build
            .join("libarchive")
            .join("Release")
            .join("libarchive.a"),
        build.join("libarchive").join("libarchive.a"),
        build.join("Release").join("archive.lib"),
    ];

    for candidate in &candidates {
        if candidate.is_file() {
            return candidate.clone();
        }
    }

    panic!(
        "\n\nlibarchive built but no static library was found. Looked in:\n{}\n",
        candidates
            .iter()
            .map(|path| format!("  {}", path.display()))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn run(command: &mut Command, stage: &str) {
    let status = command
        .status()
        .unwrap_or_else(|err| panic!("failed to launch {command:?}: {err}"));

    if !status.success() {
        panic!("libarchive {stage} failed with {status}");
    }
}
