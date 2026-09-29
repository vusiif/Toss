//! Builds the three vendored native libraries when the `archive` feature is on.
//!
//! Deliberately uses only `std::process` rather than a helper crate: the
//! project chose to vendor its native code so that release builds never
//! depend on network access or on whatever happens to be installed, and
//! pulling a crates.io build-dependency back in would undo that (§25).
//!
//! Without the `archive` feature this script does nothing at all, so a
//! default `cargo build` neither compiles them nor pays for them (§14).
//!
//! Order matters: libarchive's own configuration asks where zlib and liblzma
//! are, so both must be built first.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    // Only these inputs may retrigger a rebuild. libarchive alone takes ~200s
    // to configure, so it must not rerun because an unrelated Rust file moved.
    println!("cargo:rerun-if-changed=third_party/libarchive");
    println!("cargo:rerun-if-changed=third_party/zlib");
    println!("cargo:rerun-if-changed=third_party/xz");
    println!("cargo:rerun-if-env-changed=CMAKE");

    if env::var_os("CARGO_FEATURE_ARCHIVE").is_none() {
        return;
    }

    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets this"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets this"));
    let cmake = find_cmake();

    // zlib first, then liblzma: neither depends on the other, but libarchive
    // depends on both, so they have to exist before its configure runs.
    let zlib = build_zlib(&cmake, &manifest, &out);
    let lzma = build_lzma(&cmake, &manifest, &out);
    let archive = build_libarchive(&cmake, &manifest, &out, &zlib, &lzma);

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
    // CMake reads a bare backslash as an escape, so every path we hand it is
    // rewritten to forward slashes. On POSIX this is a no-op.
    for option in options {
        command.arg(option.replace('\\', "/"));
    }

    run(&mut command, "configure");
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
