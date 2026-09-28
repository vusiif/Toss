# Vendored dependency

This directory holds an unmodified upstream release, committed so that
release builds never depend on whichever copy of the library happens to
exist on the build machine (ARCHIVE_BACKEND.md §25).

| | |
|---|---|
| Library | libarchive |
| Version | **3.8.9** |
| Released | 2026-07-28 |
| Source | `https://www.libarchive.org/downloads/libarchive-3.8.9.tar.xz` |
| SHA-256 | `888c934f9d95648ecb9163dc8e23ab80a476ecb81a8f1154704a227b5b676dde` |
| License | BSD-3-Clause (New BSD), see `libarchive/COPYING` |

## Why this one

Toss v0.1 needs ZIP, 7z and RAR/RAR5 on read, and 7z on write
(ARCHIVE_BACKEND.md §2). One mature backend covers all of them, and
libarchive's archive-plus-filter model is the one that keeps room for the
tar/zstd/lz4 shaped inputs expected later.

## Why vendored

The full release tarball is committed as-is — the whole tree, not a pruned
subset — so that what we build is byte-for-byte what upstream shipped.
Pruning would save space and buy a build that only breaks in ways nobody
can compare against upstream.

Size before committing: **27,702,476 bytes, 1,683 files**.

## Rules

- Do not edit files under here. Fixes belong in an adapter or upstream.
- Do not enable formats or filters merely because they exist; every one
  must pass the admission test in ARCHIVE_BACKEND.md §37 and §3.
- Record the binary-size delta whenever the enabled set changes
  (ARCHIVE_BACKEND.md §26, §27).
