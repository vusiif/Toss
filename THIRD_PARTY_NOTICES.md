# Third-Party Software and License Policy

Toss is licensed **GPL-3.0-or-later** (see [LICENSE](LICENSE)). This file
is the inventory of third-party components Toss links or vendors, the
license policy for everything admitted in the future, and the build-level
license invariants that are not allowed to regress.

This is a project policy document, not legal advice. A bounded
engineering license/source audit of the actual static release closure was
performed 2026-10-11: findings, clause-level evidence, the remaining
owner decisions and the pack-time checklist live in
`docs/LICENSE_AUDIT_REPORT.md` (local) and §5 below. It does not
constitute a legal-compliance verdict; the sign-off decisions it lists
remain open with the project owner.

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
GPL        off by default; may be enabled only when a specific required
           component needs it AND it passes the dependency gate (§1).
           Toss being GPL-3.0-or-later does not by itself widen a
           dependency's license surface.
version3   follows the GPL components — off while they are off
nonfree    FORBIDDEN — configure with --enable-nonfree fails the build
```

The default for the HEVC-only closure is therefore **no `--enable-gpl`,
no `--enable-version3`**: the spike built the exact needed capability as
LGPL 2.1+ with both off (Master Context §7.4/§7.5), and a smaller license
surface needs no justification. Turning GPL components on later is a
dependency-gate decision tied to a real capability need, made then —
not a side effect of Toss's own license.

Enforcement is implemented and runs on every build that compiles the
FFmpeg closure: after configure, `build.rs` (`assert_ffmpeg_license`)
reads the produced `config.h` — the artifact configure actually wrote,
not the flags we think we passed — and fails the build if `CONFIG_GPL 1`
or `CONFIG_NONFREE 1` is present, or if either is not declared `0`. The
`FFMPEG_CONFIGURATION` line recorded in that same `config.h` additionally
shows the exact configure invocation for audit. `--enable-nonfree`
produces binaries that are not redistributable at all (FFmpeg
LICENSE.md), which is categorically incompatible with Toss shipping as
one downloadable `toss.exe`.

Capability admission is **unaffected** by the license change: GPL
permission is not an invitation to widen the component set. The
HEVC-only minimal set stands until a real corpus need says otherwise:

```text
--disable-everything ... --enable-demuxer=matroska --enable-demuxer=mov
--enable-parser=hevc --enable-decoder=hevc --enable-protocol=file
(+ libswscale when the presentation path needs BGRA conversion)
--enable-gpl --enable-version3        NOT in the default build;
                                      only via the gate above
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
| `proc-macro2`, `quote`, `syn`, `unicode-ident` | MIT OR Apache-2.0 (`unicode-ident` **also** carries the Unicode License v3 for its derived identifier data) | build-time only (`windows` crate codegen) | YES |

No other runtime crates exist. The dependency closure is deliberately
15 crates (`ARCHIVE_BACKEND.md` §55).

### Vendored native libraries (`third_party/`, unmodified)

Provenance and tarball hashes: `third_party/UPSTREAM.md`.

| Library | Version | Upstream license | Role | Compatible |
|---|---|---|---|---|
| libarchive | 3.8.9 | **BSD-2-Clause-form primary notice** (2 conditions: retain / reproduce in docs) with per-file exceptions documented in its `COPYING` (3-clause UC Regents for two files, public domain for one, CC0/OpenSSL/Apache-2.0 for the BLAKE2 files) | archive read/write behind `ArchiveBackend` | YES |
| zlib | 1.3.1 | zlib license (permissive) | Deflate for ZIP entries | YES |
| xz / liblzma | 5.6.4 | 0BSD (liblzma only; the xz *tools* are GPL and are not built) | LZMA/LZMA2 for 7z entries | YES |

> Correction 2026-10-11 (bounded audit): the libarchive label used to
> read "BSD-3-Clause"; the tree's own `third_party/libarchive/COPYING`
> carries the two-condition form as its primary notice, so the table now
> says what the file says. The same one-line label correction was applied
> to `third_party/UPSTREAM.md`.

All three are permissive; static linking them under a GPL-3.0-or-later
whole carries no additional restriction.

### Native system APIs (no distribution)

Windows SDK / Media Foundation / WIC / GDI are used as OS capabilities
at runtime, not redistributed — no license artifact to track beyond the
platform floor (`Toss_AGENTS.md` §4).

### Microsoft C runtime (Windows MSVC)

Windows MSVC builds select a static C runtime in `.cargo/config.toml`.
`build.rs` gives the CMake libraries the same runtime choice and includes
that choice in their cache key. This removes the executable's dependency
on an independently installed Visual C++ Redistributable; Windows system
APIs remain dynamic. The C runtime was already a dependency of previous
builds; its linking and distribution mode has changed.

The pre-release audit read the exact 2026 Microsoft terms for the actual
toolchain (`docs/LICENSE_AUDIT_REPORT.md` §4; texts saved in
`docs/release-candidate/license-docs/`). Facts it established, stated
without overclaiming:

- No Microsoft redistributable *file* ships — `dumpbin /DEPENDENTS` shows
  only Windows system DLLs; the import-table check proves the absence of
  a runtime DLL dependency, not license compliance or successful
  execution on a clean Windows installation.
- The static CRT object code **is embedded** in `toss.exe`. The exact
  2026 "Distributable Code" grant is affirmative and scoped to "code
  listed on the Distributable List" (VC Runtime merge modules and
  `VC\redist` files); the static `.lib` is not on that list, and the
  build-tool use grant authorizes building, not distribution.
- The 2026-10-15 release assessment records a supported project
  interpretation: the normal compiler CRT is treated as a GPL-3 System
  Library, and static application deployment relies on Microsoft's
  documented deployment method under the applicable product license.
  The maintainer confirmed eligible Community personal/free use for the
  current build. A missing standalone `.lib` entry alone does not establish
  a deployment ban. This is not a blanket license grant or legal guarantee;
  the proprietary CRT is not GPL-relicensed by Toss. Reassess when the
  toolchain or license basis changes. References and scope: `RELEASING.md`.

### Vendored FFmpeg (enters with the HEVC decode core)

| Item | Value |
|---|---|
| Version | 9.0.2 (verbatim release tarball, `third_party/ffmpeg/`) |
| Upstream | FFmpeg project — licenses preserved upstream, tree never edited |
| License of the built closure | **LGPL-2.1-or-later** — GPL components off, nonfree off (§2) |
| Configure line | lives in `build.rs` (`FFMPEG_OPTIONS`); capability = hevc decoder + matroska/mov demuxers + file protocol + swscale, from `--disable-everything` up |
| Build mode | MSVC static (`--toolchain=msvc`), ASM on (NASM required), driven by `build.rs` into `.native-cache`, only on Windows with the `media` feature |
| Distribution | LGPL source obligations apply at release time; re-checked in the pre-release audit (§1) |

---

## 4. What this file is not

- Not a copy of every upstream license text — upstream trees carry
  their own (`third_party/libarchive/COPYING`, `third_party/zlib/README`,
  `third_party/xz/COPYING`, `third_party/ffmpeg/COPYING.LGPLv2.1`,
  `LICENSE` for GPLv3), and the crate texts are staged under
  `docs/release-licenses/` for packing.
- Not the audit itself, and not a legal verdict — the bounded engineering
  audit (clause-level evidence and the open decisions) is
  `docs/LICENSE_AUDIT_REPORT.md`; the notices file intended to ship with
  the exe is `docs/RELEASE_NOTICES_DRAFT.md`.

---

## 5. Release license/source checklist (from the 2026-10-11 audit)

Run against the actual release artifact before publishing. Evidence and
wording for each item: `docs/LICENSE_AUDIT_REPORT.md` §6–§7.

1. **Decide the source-conveyance route** for GPL-3 §6 / LGPL-2.1 §6 —
   **decided for the reviewable candidate: source published alongside the
   exe** (LGPL §6(a)+(d), GPL §6); draft packaging decision recorded in
   `docs/release-candidate/PACKAGE.md`. Not used: written offer. Nothing
   published yet; hosting happens only at the publishing step.
2. **Confirm the exact toolchain's Distributable Code clauses** — **read
   2026-10-11** (second pass): the exact VS2026 BuildTools MSLT, VC++ V14
   runtime terms, the product MSLT Distributable-Code section and the
   exact 2026 Distributable List page were read (texts saved under
   `docs/release-candidate/license-docs/`); toolchain verified stable
   (not preview); zero Distributable-List files ship, but the static CRT
   object code is embedded. Two scoped residuals are itemized in
   `docs/LICENSE_AUDIT_REPORT.md`: **R1** (§4.3 — whether the static CRT
   qualifies as a GPL-3 §1 System Library; the carve-out text itself
   exists) and **M1** (§4.2 — distribution authorization of the embedded
   CRT). The 2026-10-15 documented interpretation in `RELEASING.md`
   supersedes their historical unresolved status; no absolute verdict is claimed.
3. **Ship with the exe**: `LICENSE`, this file, the notices file built
   from `docs/RELEASE_NOTICES_DRAFT.md`, and
   `third_party/ffmpeg/COPYING.LGPLv2.1`.
4. **Include the staged crate texts** from `docs/release-licenses/`
   (windows-rs + proc-macro2 family MIT/Apache-2.0, unicode-ident
   Unicode License v3).
5. **Notices must reproduce** the libarchive `COPYING` block, the zlib
   README notice, and `xz/COPYING.0BSD` (draft has all three).
6. **State prominently** that FFmpeg is used, LGPL-2.1-or-later, built
   with GPL/nonfree off (`config.h` invariants), and where its source
   lives.
7. **Tie source to the binary**: record the release tag/commit next to
   the exe's SHA-256 (`docs/logs/release-candidate.sha256`).
8. **Re-run** the license invariant (automatic in `build.rs`), the
   `avcodec.lib` symbol grep for GPL/IJG-sensitive objects, and items 3–5
   if any build input changed.
