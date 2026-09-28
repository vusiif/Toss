# Vendored dependencies

Unmodified upstream releases, committed so that release builds never depend
on whichever copy of a library happens to exist on the build machine
(ARCHIVE_BACKEND.md §25). Nothing under this directory is ever edited: fixes
belong in an adapter or upstream.

## What is here

| Library | Version | License | Role |
|---|---|---|---|
| [libarchive](https://www.libarchive.org/) | **3.8.9** | BSD-3-Clause | archive reader and writer behind `ArchiveBackend` |
| [zlib](https://zlib.net/) | **1.3.1** | zlib license (permissive) | Deflate, so ZIP entries can be read |
| [xz / liblzma](https://tukaani.org/xz/) | **5.6.4** | **0BSD** for liblzma | LZMA / LZMA2, so 7z entries can be read and written |

All three are permissive. Static linking them into a single executable
carries no copyleft obligation, which matters because Toss ships as one
binary (ARCHIVE_BACKEND.md §44).

The xz tree also contains GPL-licensed command-line tools; `xz/COPYING` is
explicit that **liblzma itself is 0BSD** and that the GPL parts are the
tools, which Toss does not build.

## Provenance

| | Source | SHA-256 of release tarball | Extracted |
|---|---|---|---|
| libarchive | `https://www.libarchive.org/downloads/libarchive-3.8.9.tar.xz` | `888c934f9d95648ecb9163dc8e23ab80a476ecb81a8f1154704a227b5b676dde` | 27,702,476 B / 1,683 files |
| zlib | `https://zlib.net/fossils/zlib-1.3.1.tar.gz` | `9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23` | 4,390,808 B / 253 files |
| xz | `https://tukaani.org/xz/xz-5.6.4.tar.gz` | `269e3f2e512cbd3314849982014dc199a7b2148cf5c91cedc6db629acdf5e09b` | 9,410,885 B / 605 files |

Each tree is committed whole — the release tarball contents, not a pruned
subset. Pruning would save space and buy a build that only fails in ways
nobody can diff against upstream. Verbatim is the cheaper risk.

## Why these three

Toss v0.1 needs ZIP, 7z and RAR/RAR5 on read, and 7z on write
(ARCHIVE_BACKEND.md §1, §18). One mature backend covers all of them, and
libarchive's archive-plus-filter model is the one that keeps room for the
tar/zstd/lz4 shaped inputs expected later (ARCHIVE_BACKEND.md §2).

zlib and liblzma are not optional extras: without them libarchive compiles
but refuses to inflate, so ZIP's Deflate entries and 7z's LZMA entries
cannot be read at all. RAR/RAR5 needs neither — libarchive carries its own
decoder and falls back to a bundled CRC32 when zlib is absent.

## What is deliberately not here

libarchive can be configured with bzip2, zstd, OpenSSL, expat, libxml2 and
more. None of them are vendored and none are enabled, because no current
milestone uses them (ARCHIVE_BACKEND.md §3, §37). A format or filter is
admitted when it has a use case, not because it is available.

## Rules

- Do not edit files under `third_party/`.
- Do not enable a format or filter without passing the admission test
  (ARCHIVE_BACKEND.md §37).
- Record the binary-size delta whenever the enabled set changes
  (ARCHIVE_BACKEND.md §26, §27). Measured deltas are in
  ARCHIVE_BACKEND.md, "Dependency decision record".
