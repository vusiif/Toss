# Third-Party Software and License Policy

Toss is licensed **GPL-3.0-or-later** (see [LICENSE](LICENSE)). This file
is the inventory of third-party components Toss links or vendors, the
license policy for everything admitted in the future, and the build-level
license invariants that are not allowed to regress.

This is a project policy document, not legal advice. A dependency
license audit is still required before the first public release.

---

## 1. License policy

```text
Toss itself                GPL-3.0-or-later
Third-party components     upstream licenses preserved, unchanged
Combined work (toss.exe)   must be GPL-3.0-or-later compatible as a whole
--enable-nonfree           FORBIDDEN, forever
```

Why GPLv3+ rather than GPLv2-only:

- FFmpeg builds with `--enable-gpl --enable-version3` are GPLv3-compatible;
  Apache-2.0 material is compatible with GPLv3 but not with GPLv2-only.
- Future capabilities (PDF engines, DOCX layouts, codecs, image
  decoders) should not be locked out of the dependency pool by an
  artificially narrow license choice.

Rules:

1. **Never rewrite upstream license headers.** Vendored code keeps its
   own copyright notices and licenses. GPL applies to the combined
   work, not by relabeling someone else's source.
2. **Every new dependency must answer, before admission:**

   ```text
   License:
   GPL-3.0-or-later compatible: YES / NO / REVIEW
   Static linking implications:
   Redistribution requirements:
   ```

   `NO` does not enter Toss. `REVIEW` stops the task and goes to the
   user.
3. The permissive licenses already in the tree (BSD/zlib/0BSD/MIT/Apache)
   remain what they are; this file records them so nobody has to
   re-derive the license of a static library from `build.rs` archaeology.

---

## 2. Build invariants (license side)

These apply to any future FFmpeg (or other media stack) build wiring:

```text
GPL        allowed (must stay allowed)
version3   allowed (must stay allowed)
nonfree    FORBIDDEN — configure with --enable-nonfree fails the build
```

Recommended future enforcement (not yet implemented): a build-script
check that greps the configure line / `config.h` for
`CONFIG_NONFREE 1` and fails hard if found. `--enable-nonfree` produces
binaries that are not redistributable at all (FFmpeg LICENSE.md), which
is categorically incompatible with Toss shipping as one downloadable
`toss.exe`.

Capability admission is **unaffected** by the license change: GPL
permission is not an invitation to widen the component set. The
HEVC-only minimal set stands until a real corpus need says otherwise:

```text
--disable-everything ... --enable-demuxer=matroska --enable-demuxer=mov
--enable-parser=hevc --enable-decoder=hevc --enable-protocol=file
--enable-gpl --enable-version3        (when the media stack lands)
```

---

## 3. Inventory

### Toss first-party code

| Item | Value |
|---|---|
| Copyright | Copyright (C) 2026 IceLolly |
| License | GPL-3.0-or-later |
| Scope | everything outside `third_party/` and future vendored trees |

### Rust crates (Cargo.toml / Cargo.lock)

| Crate | License | Role | GPL-3.0-or-later compatible |
|---|---|---|---|
| `windows` (+ its `windows-*` transitive crates) | MIT OR Apache-2.0 | Win32 bindings, behind the `image`/`media` features | YES |
| `proc-macro2`, `quote`, `syn`, `unicode-ident` | MIT OR Apache-2.0 | build-time only (`windows` crate codegen) | YES |

No other runtime crates exist. The dependency closure is deliberately
15 crates (`ARCHIVE_BACKEND.md` §55).

### Vendored native libraries (`third_party/`, unmodified)

Provenance and tarball hashes: `third_party/UPSTREAM.md`.

| Library | Version | Upstream license | Role | Compatible |
|---|---|---|---|---|
| libarchive | 3.8.9 | BSD-3-Clause | archive read/write behind `ArchiveBackend` | YES |
| zlib | 1.3.1 | zlib license (permissive) | Deflate for ZIP entries | YES |
| xz / liblzma | 5.6.4 | 0BSD (liblzma only; the xz *tools* are GPL and are not built) | LZMA/LZMA2 for 7z entries | YES |

All three are permissive; static linking them under a GPL-3.0-or-later
whole carries no additional restriction.

### Native system APIs (no distribution)

Windows SDK / Media Foundation / WIC / GDI are used as OS capabilities
at runtime, not redistributed — no license artifact to track beyond the
platform floor (`Toss_AGENTS.md` §4).

### Reserved: media software decode stack (not yet in the tree)

| Item | Status |
|---|---|
| FFmpeg / libav* | Spike-only so far (out-of-tree, `%TEMP%`). When admitted: **upstream licenses preserved**; combined build expected under `--enable-gpl --enable-version3`; `nonfree` forbidden; LGPL/GPL source obligations to be met at distribution time and re-checked in the pre-release audit. |
| Exact version / configure line | to be recorded here at integration time |

---

## 4. What this file is not

- Not a copy of every upstream license text — upstream trees carry
  their own (`third_party/libarchive/COPYING`, `third_party/zlib/README`,
  `third_party/xz/COPYING`, `LICENSE` for GPLv3).
- Not a substitute for the pre-release license audit required before
  v0.1 (see §1).
