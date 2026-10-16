# RELEASING — local-draft packaging tooling (v0.1 release-engineering slice)

Status: **developer tooling for LOCAL DRAFT packages only.** Nothing produced
by these scripts has been published, uploaded or conveyed. Publishing is a
separate, owner-decided step. Older artifacts under `docs/release-candidate/`
and `docs/logs/build_candidate.py` are reference material only; they are not
authority for this tool and their legal wording is not imported here.

## Tools

### v0.1.0 baseline decision (2026-10-15)

The maintainer chose to publish a first functional baseline and collect
remaining media/GUI feedback after release. Current Tiny10 startup and archive
evidence is accepted; no-extra-runtime and stock Win10 22H2 runs are optional
future coverage. TESTING.md retains every unverified functional item without
claiming it passed. Archive resource caps remain deferred to v0.2.

Freeze the release commit before building. Use a clean checkout of that commit
so local `.sandbox-test/`, unrelated untracked sources, caches and diagnostics
cannot enter the source package. Build with the existing locked offline
all-features recipe and verify the assembled local package at Full level.

The primary public asset is the standalone Windows x64 `toss.exe`; it needs
no adjacent DLLs, resource files or configuration directory. An optional
Windows x64 ZIP includes the same executable, licenses, notices and build
information. Also publish matching first-party/vendored source ZIP, locked
crate source ZIP, source manifest and SHA256SUMS. The local-draft builder's
LOCAL-DRAFT.md and local-only status record are preparation evidence rather
than the public release wrapper. Record the actual source commit, build
command, executable hash and known limitations in the public build information
and release notes. Publish assets and source together under the same tag.

README remains unchanged during preparation; its update is deferred until
after the first release as requested by the maintainer.

### v0.1.1 performance baseline decision (2026-10-16)

The maintainer selected v0.1.1 as the baseline for future executable-size and
runtime-efficiency comparisons. This patch selects the explicit `info` action
and image interactions from `experimental-dev`, including corrected image
fitting. Archive/media progress and unused indicatif sources are excluded.
Dependencies and the single-executable distribution model are unchanged.

Use the published v0.1.1 `toss.exe` and its release SHA256SUMS as the canonical
baseline, and keep a byte-identical local copy at `dist/v0.1.1/toss.exe`.
Future versions belong in separate `dist/<version>/` folders. Do not overwrite
older baselines. Local dist files are excluded locally, not source inputs.

Compare full `cargo build --release --locked --offline --all-features`
artifacts with the same toolchain and static-CRT configuration. Record exact
bytes, toolchain, SHA-256, workload, environment, sample count and dispersion.
Separate warm-cache CLI startup from cold startup, image opening, archive
throughput, media CPU usage and peak memory. A previous preview measured only
CLI wall time: 3 warmups, 40 samples per binary/workload, alternating order,
redirected output. Its timing ranges overlapped; no startup performance change
was established. Do not promote those preview timings into measurements of a
different release executable without measuring it.

This patch's scoped image/info checks passed 127 tests and strict Clippy on
the host with explicit maintainer authorization; this was not isolated testing.
GUI/VM interaction acceptance remains deferred. Manual window resizing needs
`F` to fit again. Release notes retain these limitations and the existing
archive/media caveats; unperformed tests are never recorded as passed.

```text
scripts/New-ReleasePackage.ps1    create a local-draft package
scripts/Test-ReleasePackage.ps1   verify a package; fixture self-tests; smoke run
```

Dependency-light PowerShell 7 tooling (no Pester, no modules, no network).
Both scripts `#Requires -Version 7.0`. All errors go to stderr with a
truthful message; all writes stay inside the workspace.

## Creating a package

```powershell
pwsh -NoProfile -File scripts/New-ReleasePackage.ps1              # current repo
pwsh -NoProfile -File scripts/New-ReleasePackage.ps1 -AllowDirty  # record a dirty tree
```

The default run:

1. refuses to start unless `RepoRoot` is the git toplevel and every required
   input exists (`LICENSE`, `THIRD_PARTY_NOTICES.md`, `Cargo.toml`,
   `Cargo.lock`);
2. runs the real build **`cargo build --release --locked --offline
   --all-features`** (never a network build, never a silently reused stale
   exe — see Freshness below);
3. inventories the source as `git ls-files` (tracked) plus
   `git ls-files --others --exclude-standard`, hashes every file into
   `SOURCE_MANIFEST.sha256`, and packages those exact working-tree bytes;
4. copies the GPL `LICENSE`, `THIRD_PARTY_NOTICES.md`, the required upstream
   license files of each vendored tree (`third_party/*/COPYING|LICENSE*`),
   and the license files of every `Cargo.lock` crate read from the local
   cargo registry cache (`~/.cargo/registry/src`);
5. builds `source/toss-0.1.0-source.zip` (all inventoried files) and
   `source/toss-0.1.0-crate-sources.zip` (exact locked crate versions);
6. writes `PROVENANCE.json` (build command/exit/duration/tool versions, git
   commit and development state, executable size+SHA-256, source content
   hash, crate list), a generated `LOCAL-DRAFT.md` banner, and finally
   `MANIFEST.sha256` covering every packaged file except itself.

Output always lands in a **new** folder under
`docs/release-work/packages/` (default name
`toss-local-draft-<utc>-<rand>`, or `-OutputName`). An existing folder is
never overwritten, merged or deleted; a failed run leaves its
`<name>.partial-<hex>` directory in place as evidence and exits nonzero.

### Exit codes (`New-ReleasePackage.ps1`)

```text
0  success
1  internal/unexpected failure (partial output left, clearly marked)
2  invalid argument, or -OutputRoot outside the repository / traversing a junction or reparse point
3  missing/invalid input: LICENSE, notices, source file, Cargo.lock crate
   source absent from the local registry cache, missing/empty/non-MZ
   executable, credential-like file that is tracked, or source/crate entry
   is a reparse point or symlink
4  cargo build failed, or the executable is older than the newest
   inventoried input (stale binary)
5  the requested output folder already exists
6  tracked development state is dirty and -AllowDirty was not given
```

## Local development state policy (explicit)

* **Default = strict.** If any tracked/staged/deleted file differs from
  HEAD, packaging refuses (exit 6). Commit or stash first.
* **`-AllowDirty`** packages the working tree as-is and records the truth in
  `PROVENANCE.json`: `git_dirty=true`, `dev_state_policy="allow-dirty"`, the
  dirty file list, and the untracked new files that were inventoried. The
  package is then tied to the working tree, not to a commit; reviewers must
  read the provenance before trusting the commit id.
* **Untracked, non-ignored files are inventoried** (they are real build
  inputs). Gitignored files (`docs/`, `target/`, `*.log`, `STATUS.md`, …)
  are excluded by construction.
* **Credential denylist.** Untracked files matching `.env*`, `.netrc`,
  private keys (`id_rsa`, `id_ed25519`, `id_ecdsa`, `*.pem|pfx|p12`),
  `credentials*` or `secrets.*` are excluded from the inventory and listed
  in `PROVENANCE.json` under `source.excluded_credential_files`. A *tracked*
  match is a hard failure (exit 3) — this tool will not silently ship or
  silently drop it.

## Freshness (no stale exe)

After the build (or when `-ExecutablePath` is supplied in fixture mode), the
executable's mtime must be **at least as new as the newest cargo build
input** in the source inventory (`src/`, `tests/`, `benches/`, `examples/`,
`third_party/`, `.cargo/`, `build.rs`, `Cargo.toml`, `Cargo.lock`,
`rust-toolchain*`); otherwise the run fails with exit 4 and names both
files. A changed source file therefore cannot quietly ship yesterday's
binary, while a documentation edit does not force a bogus rebuild.
Provenance records the newest build input, the newest inventoried file, the
executable timestamp and the check result.

`-ExecutablePath` is a testing/fixture escape hatch only: provenance records
`build.mode="provided-executable"` and the build command is marked skipped.
The real smoke path always builds.

## Verifying a package

```powershell
pwsh -NoProfile -File scripts/Test-ReleasePackage.ps1 -Mode Verify -PackageDir <dir> -Level Quick
pwsh -NoProfile -File scripts/Test-ReleasePackage.ps1 -Mode Verify -PackageDir <dir> -Level Full
```

Checks (exit 0 = verified, 1 = failed with reasons on stderr, 2 = bad
arguments):

* required files present (`toss.exe`, `LICENSE`, `THIRD_PARTY_NOTICES.md`,
  both zips, both manifests, `PROVENANCE.json`, `LOCAL-DRAFT.md`);
* `MANIFEST.sha256` and `SOURCE_MANIFEST.sha256` strictly formatted
  (`<64-lowercase-hex>  <relative/forward-slash path>`), no unsafe paths,
  no duplicates; every packaged file present and matching, no extras;
* provenance cross-checks: schema, executable hash/size, source content
  hash and count, artifact hashes, build mode/exit, freshness flag,
  development-state consistency, and the **non-claims block** (local draft,
  no legal clearance, no reproducibility guarantee, no byte-identical
  rebuild claim, TESTING checkboxes not asserted, R1/M1 assessment retained);
* GPL/notice text sanity and the executable's MZ signature;
* source zip entry set equals `SOURCE_MANIFEST.sha256` (Quick), and at
  Full level every entry is decompressed and re-hashed against it;
* crate zip contains exactly the provenance crate list, each with its
  recorded license file.

Tamper-detection limit (stated, not hidden): the verifier alone cannot
prove `MANIFEST.sha256`/`PROVENANCE.json` were not rewritten together with
the payload. The smoke log records the package's `manifest_sha256` out of
band (`docs/release-work/logs/smoke-summary-*.log`); compare that value to
detect whole-package rewriting.

## Self-tests

```powershell
pwsh -NoProfile -File scripts/Test-ReleasePackage.ps1 -Mode SelfTest
```

Bounded, fixture-based (no Pester, no cargo build, no network): small
synthetic git repos are created under `docs/release-work/fixtures/run-*/`
(exclusively this tool's own files; the real repository history is never
touched). Covered with real failure cases: missing LICENSE, missing vendored
license file, `Cargo.lock` crate absent from the registry cache, overwrite
attempt on an existing package, strict vs `-AllowDirty` dirty policy,
output outside the workspace, missing/non-MZ/stale executable, tampered
executable, rewritten and malformed manifests, extra/missing package files,
tampered source-zip content, crate-zip gap, unicode/space paths, directory
junction output, and reparse-point/symlink source input.

### Self-test status reporting and exit policy

* **Status distinction**: Test results in console and summary logs distinguish
  `PASS`, `FAIL`, and `SKIP`. Every skip records a non-empty diagnostic reason
  (e.g., environment without symlink privileges) and never passes. Real
  assertion failures always fail and never convert into skips.
* **Count invariant**: `passed + failed + skipped = total` is asserted strictly
  at summary time.
* **Default exit policy for skipped capability tests**: By default, skipped tests
  due to unavailable host capabilities (such as unprivileged symlink or junction
  creation) do not fail the overall run (exit 0) if all executed assertions pass,
  preserving compatibility for unprivileged test runners while reporting the skip
  truthfully in console and summary logs (`<test>=SKIP: <reason>`). No claim of
  all-capability coverage is made when skips occur. If any test fails, the run exits 1.
* **Deterministic simulation**: Setting `$env:TOSS_TEST_FORCE_UNAVAILABLE` to
  `symlink`, `junction`, or `all` allows exercising the capability-unavailable
  code paths deterministically in automated testing without changing host privileges.
Logs: `docs/release-work/logs/selftest-*.log`.

Latest local verification (2026-10-15, explicitly authorized host PowerShell 7,
not isolated): normal suite 24 PASS / 0 FAIL / 0 SKIP; forced-unavailable suite
22 PASS / 0 FAIL / 2 SKIP; both exit 0. T24 now inserts its deliberate failure
before the final summary marker only, avoiding replacement of its own marker
literal. The actual timeout helper was also checked with a sleeping child:
the two-second limit triggered and the child exited. Local evidence is under
`.sandbox-test/results/release-confirm-20261015/`; this does not replace a
real candidate build or clean-machine runtime validation.

## Smoke run (real repository, real build)

```powershell
pwsh -NoProfile -File scripts/Test-ReleasePackage.ps1 -Mode Smoke             # clean tree
pwsh -NoProfile -File scripts/Test-ReleasePackage.ps1 -Mode Smoke -AllowDirty # recorded dirty tree
```

Packages the current checkout with the real locked offline all-features
build (`-AllowDirty` is passed through when a dirty working tree is being
packaged deliberately; the strict policy refuses it otherwise), verifies the
result at Full level, and writes evidence to
`docs/release-work/logs/smoke-pack-*.log`, `smoke-verify-*.log` and
`smoke-summary-*.log` (including the out-of-band `manifest_sha256`).

## Recorded CRT licensing position

* **R1** — whether the statically linked MSVC CRT qualifies as a GPL-3.0
  System Library (`docs/LICENSE_AUDIT_REPORT.md` §4.3).
* **M1** — Microsoft Distributable Code authorization for the embedded
  static CRT object code (§4.2).

On 2026-10-15 the maintainer confirmed eligible Visual Studio Community
personal/free use as the current builder's license basis. The project records
the following supported engineering position: the normal compiler CRT is
treated as a GPL-3 System Library, and deployment of the statically linked
application relies on Microsoft's documented static-link deployment method,
subject to the applicable product license. Static embedding and absence of
standalone `.lib` entries from a runtime-file list do not by themselves prove
incompatibility. R1/M1 are recorded interpretations rather than unconditional
implementation blockers; they must be reassessed if the toolchain or license
basis changes.

Primary references: [GNU System Library FAQ](https://www.gnu.org/licenses/gpl-faq.en.html#SystemLibraryException),
[Microsoft deployment guidance](https://learn.microsoft.com/en-us/cpp/windows/deployment-in-visual-cpp),
and [Microsoft licensing guidance](https://www.microsoft.com/licensing/guidance/Visual-Studio).
The latter is guidance and defers to the actual agreement; it is not a blanket
grant covering arbitrary Microsoft libraries. No legal guarantee, CRT relicensing
or future builder license attestation is made by the tooling. GPL-3.0-or-later,
required notices/source delivery, and the separate publication decision remain.

## Explicit non-goals of this slice

* No publish/upload/install/push; no changes to Rust sources, `Cargo.toml`,
  `Cargo.lock`, `build.rs`, vendored trees, CI, or README (README stays
  untouched until after the first release).
* No archive resource caps (user-accepted for v0.1; deferred to v0.2).
* `TESTING.md` checkboxes are neither required nor marked by this tooling;
  the Rust test suite is not re-run for unchanged sources (scripts-only
  change; PowerShell parse/self-tests/smoke are the applicable checks).
