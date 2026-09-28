可以。下面这段可以直接追加到现有 `Toss_AGENTS.md`，或者单独保存成 `docs/ARCHIVE_BACKEND.md`。我把**当前决策、为什么这么选、未来分卷/Zstd/LZ4 的扩展方式、Backend/Capability 设计、安全边界，以及 Agent 明确禁止做什么**都写死了，避免 Agent 自己又重新拍脑袋选技术路线。

````
# Toss Archive Subsystem — Architecture, Backend Strategy & Agent Constraints

> Status: **Architecture Decision**
>
> Scope: Toss archive subsystem, beginning with Phase 4.
>
> This document defines the intended archive architecture and constrains implementation decisions made by coding agents.
>
> If this document conflicts with a generic implementation preference, this document takes precedence for the archive subsystem.
>
> The objective is not merely to make ZIP/7z/RAR work today. The objective is to build an archive subsystem that remains small and understandable in v0.1 while allowing future support for additional archive formats, compression filters, multi-volume archives, and specialized backends without redesigning Toss Core.

---

# 1. Current Decision

For the first archive implementation:

```text
Primary Archive Backend:
    libarchive

Integration:
    vendored / reproducible
    statically linked
    feature-gated

Rust boundary:
    raw FFI
        ↓
    safe Rust wrapper
        ↓
    LibarchiveBackend
        ↓
    ArchiveBackend abstraction
        ↓
    ArchiveHandler
```

However:

> **libarchive is the first backend, not the permanent definition of Toss's archive architecture.**

Toss must not architect itself as:

```text
Toss
 ↓
libarchive
```

Instead:

```text
Toss
 ↓
ArchiveHandler
 ↓
ArchiveRouter
 ↓
ArchiveBackend
 ↓
LibarchiveBackend
```

This distinction is mandatory.

Future backends may coexist when they provide capabilities that libarchive cannot provide adequately.

For example:

```text
ArchiveRouter
     │
     ├── LibarchiveBackend
     │
     └── SevenZipBackend        [future, only if justified]
```

Do not implement `SevenZipBackend` during Phase 4 unless a concrete requirement proves it necessary.

---

# 2. Why libarchive Is the Initial Backend

Toss v0.1 currently requires approximately:

```text
Read / extract:
    ZIP
    7z
    RAR
    RAR5

Write:
    7z
```

A direct integration of selected 7-Zip source code could satisfy much of this.

However, Toss is expected to grow beyond those formats.

Potential future requirements include:

```text
.tar
.tar.gz
.tar.xz
.tar.zst
.tar.lz4

.gz
.xz
.zst
.lz4

.cab
.iso
.cpio
and other useful emergency formats
```

The long-term archive model therefore looks more like:

```text
Archive Format
      +
Compression / Filter
```

Examples:

```text
tar + gzip
tar + xz
tar + zstd
tar + lz4
```

rather than merely:

```text
ZIP
7z
RAR
```

libarchive is designed around this broader archive/filter model and is therefore a better initial foundation for Toss's expected expansion.

This does **not** mean every format supported by libarchive should be enabled immediately.

---

# 3. Do Not Enable Everything

The fact that libarchive supports many formats does not mean Toss should ship all of them.

The project remains governed by the binary-size and dependency-budget rules.

v0.1 should enable only the functionality required by the current milestone.

Conceptually:

```text
v0.1

Read:
    ZIP
    7z
    RAR/RAR5

Write:
    7z
```

Future versions may intentionally add:

```text
tar
gzip
xz
zstd
lz4
cab
iso
...
```

only when they provide useful emergency functionality.

Do not enable formats merely because they are available.

Every newly enabled format or filter must still pass the Toss feature-admission test.

---

# 4. Archive Architecture Must Be Backend-Agnostic

Do NOT expose libarchive concepts outside the libarchive backend.

The following must never become public archive-domain types:

```text
archive*
archive_entry*
ARCHIVE_OK
ARCHIVE_EOF
ARCHIVE_WARN
libarchive-specific format constants
libarchive-specific filter constants
```

These belong inside:

```text
backend/archive/libarchive/
```

or the equivalent implementation directory.

The rest of Toss must use Toss-owned domain types.

Desired dependency direction:

```text
ArchiveHandler
      ↓
ArchiveRouter
      ↓
ArchiveBackend trait
      ↓
Toss archive domain types
      ↓
backend implementation
      ↓
libarchive FFI
```

Never:

```text
ArchiveHandler
      ↓
libarchive FFI
```

---

# 5. Do Not Design the Trait Around Today's Formats

Forbidden design:

```rust
trait ArchiveBackend {
    fn extract_zip(...);
    fn extract_7z(...);
    fn extract_rar(...);
}
```

This hard-codes today's format list into the architecture.

Adding:

```text
tar.zst
lz4
cab
iso
```

would then require repeatedly expanding the backend interface.

Instead, model operations.

Conceptually:

```rust
trait ArchiveBackend {
    fn capabilities(&self) -> ArchiveCapabilities;

    fn probe(
        &self,
        input: &ArchiveInput,
    ) -> Result<ArchiveProbe>;

    fn list(
        &self,
        request: &ListRequest,
    ) -> Result<Vec<ArchiveEntry>>;

    fn extract(
        &self,
        request: &ExtractRequest,
    ) -> Result<ExtractResult>;

    fn create(
        &self,
        request: &CreateRequest,
    ) -> Result<CreateResult>;
}
```

The exact Rust API may evolve during implementation.

The architectural rule is more important than the exact signatures:

> Backends implement generic archive operations, not format-specific methods.

---

# 6. Separate Archive Format From Compression Method

Do not assume that:

```text
extension == compression algorithm
```

These are different concepts.

For example:

```text
foo.tar.zst
```

contains approximately:

```text
Archive format:
    TAR

Compression/filter:
    Zstandard
```

Similarly:

```text
foo.tar.gz

Archive:
    TAR

Compression:
    gzip
```

Domain types should therefore permit distinctions such as:

```rust
enum ArchiveFormat {
    SevenZip,
    Zip,
    Tar,
    Rar,
    Cpio,
    Cab,
    Iso,
    Unknown,
}
```

and:

```rust
enum CompressionMethod {
    Store,
    Deflate,
    Lzma,
    Lzma2,
    Bzip2,
    Xz,
    Zstd,
    Lz4,
    Unknown,
}
```

These enums are illustrative rather than frozen API.

Do not add every possible variant before it is needed.

The important architectural requirement is:

> Archive container and compression/filter are separate concepts.

---

# 7. Capability-Driven Backend Selection

Backends must eventually be selected by capability rather than hard-coded library identity.

Do not build logic such as:

```rust
if extension == "7z" {
    use_libarchive();
}
```

or:

```rust
if extension == "rar" {
    use_sevenzip();
}
```

Instead, the backend should be able to describe what it can do.

Conceptually:

```rust
struct ArchiveCapabilities {
    // supported read formats
    // supported write formats
    // supported compression/filter methods
    // encryption support
    // multi-volume support
    // streaming support
    // listing support
}
```

A more structured model may be used if it remains simple.

The important idea is:

```text
Requested operation
        +
Detected format
        +
Required features
        ↓
Capability matching
        ↓
Suitable backend
```

Example:

```text
Input:
    foo.7z

Operation:
    Extract

Required:
    SevenZip read

Available:
    LibarchiveBackend = yes

Result:
    use LibarchiveBackend
```

Future example:

```text
Input:
    foo.7z.001

Operation:
    Extract

Required:
    SevenZip read
    Multi-volume read

Available:
    LibarchiveBackend = insufficient
    SevenZipBackend    = supported

Result:
    use SevenZipBackend
```

This is the intended long-term model.

---

# 8. Do Not Overengineer the Router in v0.1

Although the architecture must allow multiple backends, v0.1 will initially have only one:

```text
LibarchiveBackend
```

Therefore do not create an elaborate plugin framework.

A simple router is sufficient.

For example:

```text
ArchiveRouter
    ↓
Vec<Box<dyn ArchiveBackend>>
```

or an even simpler static arrangement may be appropriate.

Do not create:

```text
DynamicArchiveBackendPluginRegistryFactory
```

Do not add runtime plugin loading.

Do not add shared-library discovery.

Do not add configuration files for backend priorities.

The architecture needs an extension point, not an extension framework.

---

# 9. Future Multi-Volume Support

Multi-volume archives are an important possible future requirement.

Examples include:

```text
archive.7z.001
archive.7z.002
archive.7z.003
```

and RAR families such as:

```text
archive.part1.rar
archive.part2.rar
archive.part3.rar
```

or other historical naming schemes.

Potential future creation:

```text
toss pack folder --volume 4G
```

However:

> Multi-volume support is NOT required for Phase 4 unless explicitly added to the milestone.

Do not prematurely introduce another archive engine solely because multi-volume support may be needed later.

Instead, ensure the domain model can represent it.

Potential concepts:

```rust
struct ArchiveInput {
    primary_path: PathBuf,
    volumes: Vec<PathBuf>,
}
```

or:

```rust
enum ArchiveInput {
    Single(PathBuf),
    MultiVolume {
        primary: PathBuf,
        volumes: Vec<PathBuf>,
    },
}
```

Exact representation is not fixed.

Do not implement speculative volume discovery complexity before required.

The architecture must simply avoid assuming forever that:

```text
one archive == one physical file
```

---

# 10. Future Specialized Backend

If a future requirement exposes a real libarchive limitation, Toss may add a specialized backend.

The most likely candidate is a backend based on selected 7-Zip source code.

Possible reasons include:

```text
better multi-volume behavior
specific 7z features
specific RAR behavior
advanced 7z creation options
performance
compatibility
```

But this backend must only be introduced after a concrete requirement and measurement justify it.

Future conceptual architecture:

```text
                   ArchiveHandler
                         │
                   ArchiveRouter
                         │
             capability matching
                         │
          ┌──────────────┴──────────────┐
          ↓                             ↓
 LibarchiveBackend              SevenZipBackend
          │                             │
 broad archive/filter             specialized
 compatibility                    capabilities
```

The presence of multiple backends must remain invisible to ordinary users.

Users interact with:

```text
toss archive.7z
```

not:

```text
toss --archive-engine=libarchive archive.7z
```

Backend selection is an implementation detail unless explicit diagnostics are requested.

---

# 11. If SevenZipBackend Is Ever Added

Do not directly expose 7-Zip C++ APIs to Rust.

Forbidden architecture:

```text
Rust
 ↓
direct C++ class bindings
 ↓
7-Zip internal interfaces everywhere
```

Preferred:

```text
Rust
 ↓
safe Rust wrapper
 ↓
small Toss-owned C ABI
 ↓
C++ adapter
 ↓
7-Zip source
```

Example conceptual C ABI:

```c
toss_archive_open(...);
toss_archive_close(...);

toss_archive_probe(...);
toss_archive_list(...);

toss_archive_extract(...);
toss_archive_extract_all(...);

toss_archive_create(...);
```

The C ABI should expose Toss concepts rather than 7-Zip implementation concepts wherever practical.

Do not expose:

```text
IInArchive
IOutArchive
IUnknown
PROPVARIANT
CMyComPtr
```

to Rust.

If 7-Zip source is vendored:

> Prefer keeping upstream source unchanged.

Do not copy random `.cpp` files into Toss and modify them until their origin becomes unclear.

Prefer:

```text
third_party/
└── 7zip/
    └── upstream source

native/
└── archive/
    └── sevenzip_adapter.cpp
```

Build only the required upstream translation units.

Keep Toss modifications in the adapter whenever possible.

---

# 12. Zstandard Support

Zstandard is a likely future feature.

Potential inputs include:

```text
file.zst
archive.tar.zst
```

Potential output:

```text
toss pack folder --format tar --compression zstd
```

Do not implement Zstd in Phase 4 unless required.

When added, model it as a compression/filter capability rather than creating an entirely separate Toss subsystem.

Conceptually:

```text
foo.tar.zst
     │
     ├── ArchiveFormat::Tar
     └── CompressionMethod::Zstd
```

A raw `.zst` stream may represent a compressed single stream rather than a multi-entry archive.

The Toss domain model should permit this distinction.

Do not assume every compressed file contains an archive directory structure.

---

# 13. LZ4 Support

LZ4 follows the same architectural principle.

Potential examples:

```text
file.lz4
archive.tar.lz4
```

Treat LZ4 as a compression/filter capability where appropriate.

Do not add a separate top-level `Lz4Handler` merely because the extension is `.lz4` unless future requirements prove that abstraction necessary.

Prefer:

```text
File detection
      ↓
Archive/compression subsystem
      ↓
CompressionMethod::Lz4
```

---

# 14. Single-Stream Compression vs Archive Containers

Toss must distinguish:

```text
Archive container
```

from:

```text
Compressed stream
```

For example:

```text
foo.zip
```

may contain many files.

But:

```text
foo.txt.zst
```

may simply mean:

```text
foo.txt
    ↓
Zstd stream
```

Therefore future automatic behavior may differ.

Conceptually:

```text
foo.tar.zst
→ decompress filter
→ read TAR
→ extract entries
```

while:

```text
foo.txt.zst
→ decompress
→ foo.txt
```

Do not force both through an artificial "archive entries" model if the backend/domain model can represent them more accurately.

---

# 15. File Detection Must Not Depend Only on Extension

The Toss-wide rule remains:

```text
extension
    ↓
content/magic detection when needed
    ↓
backend probe when useful
```

Archive detection may use:

```text
filename
extension
multi-extension
magic bytes
backend probing
```

Examples:

```text
foo.tar.zst
```

must not be reduced merely to:

```text
extension = .zst
```

The detector should eventually understand meaningful compound extensions.

Likewise:

```text
archive.7z.001
```

may require recognizing a volume naming pattern.

Do not implement every pattern in Phase 4.

Design detection so these additions remain possible.

---

# 16. Archive Security Is Toss-Owned Policy

Using libarchive does not transfer security responsibility away from Toss.

Toss must enforce its own extraction policy.

Archive entries are untrusted input.

At minimum protect against:

```text
../
../../
absolute paths
drive-prefixed paths
UNC escape paths
path normalization tricks
```

The final extraction destination for every entry must remain inside the selected output root.

Conceptually:

```text
output_root
    +
entry_path
    ↓
normalize / validate
    ↓
must remain within output_root
```

If it escapes:

```text
reject entry
```

Do not trust the archive filename.

Do not assume the backend handles every Toss security requirement automatically.

---

# 17. Symlink and Link Safety

Archive entries may represent:

```text
symbolic links
hard links
special files
```

These can create extraction-path security problems even if the entry pathname itself appears safe.

Before enabling restoration of such entries, define explicit policy.

For early Toss versions, conservative behavior is preferred.

Do not allow an archive to create a symlink outside the extraction root and then write subsequent entries through that symlink.

Security policy must consider both:

```text
lexical path traversal
```

and:

```text
filesystem traversal through links
```

Do not silently weaken this rule for compatibility.

---

# 18. Archive Bomb / Resource Protection

The architecture should permit resource limits involving:

```text
entry count
compressed size
uncompressed size
compression ratio
individual entry size
total extracted size
nesting where relevant
```

Potential future policy:

```text
maximum file count
maximum total output
suspicious compression ratio warning
```

Do not necessarily implement every limit in Phase 4.

But do not design extraction APIs that make such limits impossible to enforce later.

Prefer streaming metadata and extraction callbacks/state that allow Toss to observe progress and totals.

---

# 19. Password / Encryption Future Support

Encrypted archives are likely to matter eventually.

Do not make v0.1 depend on implementing every encryption scheme.

But the API should not assume:

```text
archives never require credentials
```

Potential future request structure:

```rust
struct ExtractRequest {
    // ...
    password: Option<SecretString>,
}
```

The exact secret type is not prescribed here.

Do not log passwords.

Do not include passwords in normal diagnostics.

Do not store them in configuration by default.

Do not implement speculative credential management during Phase 4.

---

# 20. Streaming and Progress

Future Toss operations may need:

```text
progress display
cancellation
resource limits
streaming
```

Do not design backend APIs that require Toss to load an entire archive or file into RAM before processing.

Archive extraction should remain fundamentally stream-oriented.

Future progress may expose:

```text
entries completed
entries total
bytes processed
bytes total when known
current filename
```

v0.1 may use a simple CLI progress model.

Do not add a GUI solely for archive progress.

---

# 21. Output Naming

Default extraction remains predictable.

Example:

```text
D:\Downloads\foo.7z
```

should normally produce:

```text
D:\Downloads\foo\
```

Compound extensions require sensible future handling.

Examples:

```text
foo.tar.gz
→ foo/

foo.tar.zst
→ foo/
```

A single compressed stream may behave differently:

```text
foo.txt.zst
→ foo.txt
```

Exact naming rules should be covered by tests.

Never silently extract into an unrelated directory.

---

# 22. Existing Destination Policy

Never silently destroy existing output.

If:

```text
foo.7z
```

maps to:

```text
foo/
```

and `foo/` already exists, automatic mode must behave conservatively.

Future explicit policies may include:

```text
--overwrite
--skip
--rename
```

Do not invent implicit overwrite semantics.

---

# 23. Error Model

Backend-specific errors must be translated into Toss-owned archive errors.

Do not leak raw libarchive integer codes through high-level APIs.

Conceptually:

```rust
enum ArchiveError {
    UnsupportedFormat,
    CorruptArchive,
    PasswordRequired,
    InvalidPassword,
    UnsafePath,
    OutputConflict,
    PermissionDenied,
    ResourceLimitExceeded,
    BackendFailure,
}
```

This enum is illustrative.

Do not add speculative variants without need.

When preserving backend diagnostic text is useful, retain it as error context.

The user-facing error should remain understandable.

---

# 24. Unsafe / FFI Boundary

libarchive is a C library.

Raw FFI belongs in a narrow module.

Preferred layering:

```text
libarchive raw bindings
        ↓
LibarchiveHandle / safe wrappers
        ↓
LibarchiveBackend
        ↓
ArchiveBackend
```

High-level code must not manipulate:

```text
*mut archive
*mut archive_entry
raw C strings
raw ownership
```

directly.

All significant unsafe blocks must document:

```text
ownership
lifetime
nullability
buffer validity
cleanup
thread assumptions where relevant
```

RAII wrappers should release native resources reliably on both success and error paths.

---

# 25. Vendoring and Reproducibility

The intended release is a genuine single binary.

Do not require users to install:

```text
libarchive.dll
7z.exe
unrar.exe
tar
system package dependencies
```

for ordinary archive functionality.

Release builds should use a reproducible strategy suitable for static integration.

If libarchive is vendored:

```text
third_party/libarchive/
```

or another clearly defined dependency mechanism should identify the upstream version.

Do not silently depend on whichever system libarchive happens to exist on the build machine for official release artifacts.

Developer builds may optionally support system libraries if useful, but official release behavior must remain controlled and reproducible.

---

# 26. Dependency Closure Must Be Measured

libarchive itself may rely on compression libraries depending on enabled features.

Do not assume:

```text
"libarchive is one dependency"
```

The real cost is:

```text
libarchive
+
required compression/filter dependencies
+
runtime support
+
linker consequences
```

During implementation, record:

```text
enabled formats
enabled filters
native dependencies
final binary size
```

Do not guess binary size.

Measure release artifacts.

If enabling one feature unexpectedly adds substantial binary weight, report it before casually accepting the increase.

---

# 27. Binary-Size Baseline

Before integrating the archive backend, record the release binary size.

Example:

```text
Baseline toss.exe:
    X bytes
```

After minimal archive integration:

```text
Archive-enabled toss.exe:
    Y bytes

Delta:
    Y - X
```

When future formats are enabled, record deltas where practical.

Example:

```text
+ Zstd:
    +N bytes

+ LZ4:
    +M bytes
```

These numbers must come from actual release builds.

Do not invent estimates and present them as measurements.

---

# 28. Performance Must Be Measured, Not Assumed

Do not choose or reject an archive backend based solely on claims such as:

```text
"7-Zip must be faster"
```

or:

```text
"libarchive must be lighter"
```

When performance becomes relevant, benchmark representative files.

Potential corpus:

```text
many tiny files
few large files
highly compressible data
already-compressed media
Unicode filenames
deep directory trees
large archives
```

Correctness and security remain more important than benchmark wins.

---

# 29. Test Corpus Requirements

Phase 4 must introduce archive corpus tests.

Minimum categories:

```text
tests/corpus/archive/
├── valid/
├── corrupt/
├── unicode/
├── security/
└── edge/
```

Examples should include, where legally distributable and practical:

```text
simple.zip
simple.7z
simple.rar
simple-rar5.rar

empty archive
zero-byte member
Unicode filenames
spaces
deep directories
duplicate filenames

truncated archive
invalid header
corrupt member

../ traversal attempt
absolute path attempt
link-related escape case
```

Future additions:

```text
multi-volume/
zstd/
lz4/
encrypted/
large/
```

Every fixed archive bug should become a regression test whenever practical.

---

# 30. Round-Trip Tests

For formats Toss writes, test:

```text
source directory
      ↓
Toss create
      ↓
archive
      ↓
Toss extract
      ↓
compare
```

Comparison should verify appropriate properties such as:

```text
file contents
relative paths
empty directories where supported
Unicode names
```

Do not require preservation of metadata Toss does not claim to preserve.

---

# 31. Cross-Backend Tests

If a second backend is ever added, shared behavior must be tested against the same abstract test suite where practical.

Example:

```text
ArchiveBackend conformance tests
        │
        ├── LibarchiveBackend
        └── SevenZipBackend
```

This prevents backend-specific semantics from leaking upward.

Do not create completely independent behavior definitions for each backend.

---

# 32. Feature Flags

Archive support must remain feature-gated according to the main Toss architecture.

Conceptually:

```toml
[features]
archive = []
```

Large optional filters/codecs may later receive narrower feature gates if measurements justify them.

Possible future concept:

```text
archive
archive-zstd
archive-lz4
archive-multivolume
```

Do not create these flags prematurely.

A feature flag is justified when it meaningfully controls:

```text
binary size
native dependencies
platform support
security surface
```

not merely because every format could theoretically have its own switch.

---

# 33. Native vs Portable Builds

The archive abstraction must work in both Toss build families.

Do not assume:

```text
Windows Native == libarchive
Portable == something else
```

Backend selection and build composition are separate decisions.

For v0.1, using libarchive across builds may be the simplest and most reliable choice.

A future native build may choose a different backend only if measurements show a meaningful advantage.

Do not duplicate archive implementations solely for architectural symmetry.

---

# 34. Windows Shell Is Not the Primary Archive Backend

Windows provides some native ZIP-related functionality.

That does not make it an adequate Toss archive engine for the current requirements.

Toss requires at least:

```text
ZIP
7z
RAR/RAR5
7z creation
```

Therefore do not create a Windows-only ZIP backend merely to satisfy the "OS capability first" rule.

That rule is subordinate to:

```text
reliability
consistent behavior
reasonable architecture
actual feature requirements
```

OS-native functionality should be used when it genuinely improves Toss, not mechanically.

---

# 35. External Programs Are Not an Archive Backend

Official Toss releases must not implement archive functionality by invoking:

```text
7z.exe
unrar.exe
tar
powershell archive commands
system-installed utilities
```

The emergency-tool premise requires Toss to work when those tools are absent.

External programs may be useful during development or compatibility testing, but not as required runtime dependencies.

---

# 36. Do Not Reimplement Compression Algorithms

Toss-owned code must not implement:

```text
Deflate
LZMA
LZMA2
Zstandard
LZ4
RAR codecs
```

merely to reduce dependencies or remain "pure Rust."

Use mature implementations.

Toss's value is:

```text
detection
dispatch
safe defaults
integration
portable emergency behavior
```

not inventing new codec implementations.

---

# 37. Do Not Add Formats Without a Use Case

libarchive supporting a format is not sufficient reason to expose it through Toss.

Before adding a format, ask:

```text
Is this a plausible emergency file-handling need?

Does Toss provide meaningful value when the specialized tool is missing?

Is the default action obvious and safe?

What dependency/binary-size cost does it introduce?

Can we test it reliably?
```

Only then add it.

---

# 38. Archive CLI Direction

Automatic mode remains primary:

```text
toss archive.7z
→ extract
```

Explicit mode:

```text
toss extract archive.7z
```

Creation:

```text
toss pack folder/
```

Future explicit options may include:

```text
toss pack folder --format 7z
toss pack folder --format tar --compression zstd
toss pack folder --volume 4G

toss extract archive --output path
toss list archive
```

Do not implement all future syntax during Phase 4.

CLI design should merely avoid blocking these future forms.

---

# 39. Automatic Behavior for Future Formats

Possible future defaults:

```text
archive.zip
→ extract

archive.7z
→ extract

archive.rar
→ extract

archive.tar.zst
→ extract

file.txt.zst
→ decompress to file.txt

folder/
→ create default archive
```

These are architectural examples, not an instruction to implement them now.

Default behavior must always satisfy the Toss safe-default rule.

---

# 40. Multi-Extension Detection

Future detector logic should permit:

```text
.tar.gz
.tar.xz
.tar.zst
.tar.lz4
.7z.001
.part1.rar
```

Do not implement extension matching as only:

```rust
Path::extension()
```

followed by a permanent single-extension switch.

It is acceptable for v0.1 to use simple extension detection internally if the detector abstraction can later evolve.

The architecture must not make compound extension support invasive.

---

# 41. Probe vs Trust

A file extension selects an initial expectation.

The backend may then probe the actual input.

Example:

```text
foo.zip
    ↓
expected ZIP
    ↓
probe
    ↓
actual archive differs
```

Toss should prefer truthful detection over blindly trusting the suffix.

However, automatic fallback behavior must remain conservative.

Do not reinterpret arbitrary unknown data as an archive and perform extraction merely because a weak heuristic matched.

---

# 42. Backend Failure and Fallback

If multiple backends exist in the future:

```text
Backend A says:
    unsupported capability
```

may allow routing to:

```text
Backend B
```

But:

```text
Backend A says:
    archive is corrupt
```

must not automatically mean:

```text
try every backend until one accepts it
```

Blind fallback can create inconsistent or unsafe behavior.

Differentiate:

```text
Unsupported
```

from:

```text
Recognized but invalid/corrupt
```

and:

```text
Backend internal failure
```

Routing policy must use that distinction.

---

# 43. Backend Priority

If multiple backends can satisfy the same operation, priority should be deterministic.

Conceptually:

```text
1. backend known to provide required capability reliably
2. backend preferred by build/profile policy
3. fallback backend
```

Do not make backend order depend accidentally on:

```text
filesystem enumeration
hash-map iteration
link order
```

If multiple backends are eventually present, document the priority policy.

---

# 44. Do Not Expose Backend Choice as Normal UX

Users should think in terms of files and operations.

Good:

```text
toss archive.7z
toss extract archive.7z
```

Avoid normal UX such as:

```text
toss --backend=libarchive
toss --backend=7zip
```

A backend override may eventually exist as a hidden/debug/developer option if useful for testing.

It should not become part of ordinary usage unless a real user-facing reason emerges.

---

# 45. Phase 4 Implementation Sequence

Agents implementing Phase 4 should proceed in this order.

## Step 1 — Domain Model

Create the minimum Toss-owned types needed for:

```text
ArchiveInput
ArchiveEntry
ArchiveProbe
ExtractRequest
ExtractResult
ArchiveError
```

Do not model every future feature.

Only preserve the extension points described by this document.

---

## Step 2 — ArchiveBackend Interface

Define the smallest useful backend abstraction.

It should support at least the operations required by Phase 4.

Do not expose libarchive types.

Do not add format-specific methods.

---

## Step 3 — Raw libarchive Integration

Add the raw/native binding layer.

Keep it isolated.

Verify:

```text
build
link
cleanup
basic open/close
```

before building high-level functionality.

---

## Step 4 — Safe Wrapper

Wrap native ownership and lifetime.

No raw archive handles should escape into handler code.

Test cleanup on both success and failure.

---

## Step 5 — Probe / List

Implement archive opening and basic entry enumeration before extraction.

This provides a safer debugging surface.

Verify:

```text
ZIP
7z
RAR/RAR5
Unicode entry names
corrupt input
```

---

## Step 6 — Safe Extraction

Implement extraction with Toss-owned path policy.

Do not simply ask the backend to extract everything directly to arbitrary disk paths without Toss validation.

Validate each destination.

Add traversal regression tests immediately.

---

## Step 7 — Automatic Dispatch

Connect:

```text
toss archive.zip
toss archive.7z
toss archive.rar
```

to:

```text
ArchiveHandler
```

and then the backend.

Do not add unrelated archive features.

---

## Step 8 — Binary Measurement

Record:

```text
pre-archive release size
post-archive release size
```

Report native dependency closure.

Do not guess.

---

## Step 9 — Integration Corpus

Run:

```text
valid
corrupt
Unicode
security
```

tests.

Fix regressions before expanding scope.

---

## Step 10 — Phase 4 Completion Report

The agent must report:

```text
formats actually enabled
operations actually supported
native dependencies
binary-size delta
tests executed
security cases covered
known limitations
future capability gaps
```

Do not claim unsupported future functionality.

---

# 46. Phase 5 Interaction

Phase 5 introduces:

```text
directory
    ↓
7z archive creation
```

Reuse the same archive abstraction.

Do not create a separate:

```text
CompressionBackend
```

solely for directory-to-7z if the ArchiveBackend naturally owns archive creation.

Conceptually:

```text
DirectoryHandler
      ↓
CreateRequest
      ↓
ArchiveRouter
      ↓
LibarchiveBackend
```

This keeps extraction and creation under one coherent archive domain.

---

# 47. Future Zstd/LZ4 Phase

When Zstd or LZ4 is requested:

1. Confirm the actual user-facing use case.
2. Determine whether it is:
   - raw stream compression/decompression;
   - archive filter;
   - 7z compression method;
   - some combination.
3. Determine backend support.
4. Enable only required functionality.
5. Measure binary-size delta.
6. Add corpus files.
7. Add detection tests.
8. Add round-trip tests if Toss writes the format.
9. Document CLI/default behavior.
10. Do not redesign ArchiveHandler unless the existing abstraction genuinely cannot express the requirement.

---

# 48. Future Multi-Volume Phase

When multi-volume support becomes a real milestone:

1. Define exact required families:

```text
7z volumes?
RAR volumes?
ZIP split archives?
creation?
extraction only?
```

2. Build a legal test corpus.

3. Test LibarchiveBackend first.

4. Record exact unsupported or unreliable behavior.

5. Only then evaluate another backend.

6. If another backend is needed, implement it behind `ArchiveBackend`.

7. Add capability flags describing volume behavior.

8. Add deterministic routing.

9. Run the same security/path policy regardless of backend.

10. Measure binary-size impact before merging.

Do not add a second archive engine merely because it might theoretically be useful someday.

---

# 49. Criteria for Adding SevenZipBackend

A SevenZipBackend is justified only if at least one important requirement satisfies something like:

```text
LibarchiveBackend:
    unsupported
    or materially unreliable
    or materially incompatible
```

and:

```text
SevenZipBackend:
    demonstrably solves the requirement
```

Possible examples:

```text
required multi-volume behavior
specific 7z functionality
important archive compatibility
meaningful size/performance advantage
```

Before merging, compare:

```text
binary-size delta
build complexity
FFI LOC
unsafe surface
test results
format compatibility
maintenance cost
```

Do not select it based on brand preference.

---

# 50. No Backend Loyalty

Neither libarchive nor 7-Zip is part of Toss's identity.

Toss's identity is:

> Give Toss a file; Toss safely performs the useful operation.

Therefore:

```text
libarchive
7-Zip
Rust crates
OS APIs
```

are implementation tools.

Do not distort the public architecture to match one library's worldview.

Toss owns the domain model.

Backends adapt to Toss.

Toss does not adapt its entire architecture to a backend.

---

# 51. Decision Summary

Current decision:

```text
Phase 4:
    libarchive

Why:
    broad archive/filter model
    ZIP / 7z / RAR-family coverage
    future Zstd/LZ4/tar-style expansion
    mature C API
    cross-platform suitability
```

Current architecture:

```text
                    Toss
                     │
               ArchiveHandler
                     │
                ArchiveRouter
                     │
              ArchiveBackend
                     │
            LibarchiveBackend
                     │
                 libarchive
```

Future architecture if required:

```text
                    Toss
                     │
               ArchiveHandler
                     │
                ArchiveRouter
                     │
             capability matching
                     │
          ┌──────────┴───────────┐
          ↓                      ↓
 LibarchiveBackend       SpecializedBackend
                               │
                        possibly 7-Zip
```

Future formats such as:

```text
Zstd
LZ4
tar.zst
tar.lz4
```

should fit naturally into the archive/filter model.

Future multi-volume support should fit naturally into:

```text
ArchiveInput
+
ArchiveCapabilities
+
ArchiveRouter
```

without requiring Toss Core to know which library implements it.

---

# 52. Agent Hard Constraints

An agent working on the archive subsystem MUST NOT:

```text
- invoke external 7z.exe/unrar/tar as required runtime behavior;
- reimplement compression algorithms;
- expose libarchive types outside its backend;
- hard-code ArchiveBackend around ZIP/7z/RAR method names;
- assume one archive always equals one physical file forever;
- assume extension alone is authoritative;
- silently overwrite existing output;
- trust archive entry paths;
- allow path traversal;
- leak raw native handles into handlers;
- scatter unsafe FFI throughout the project;
- enable every libarchive format merely because it exists;
- add a second archive backend speculatively;
- add multi-volume support before it is a milestone;
- add Zstd/LZ4 before their use case is defined;
- guess binary-size impact;
- claim tests that were not run;
- perform unrelated refactors while implementing archive support;
- weaken the single-binary release requirement for convenience.
```

An agent MUST:

```text
- preserve backend independence;
- preserve future capability-based routing;
- keep native code behind a narrow boundary;
- use Toss-owned domain types;
- enforce Toss-owned extraction safety policy;
- test Unicode paths;
- test corrupt archives;
- test traversal attacks;
- record binary-size impact;
- document native dependency closure;
- convert fixed bugs into regression tests where practical;
- report known limitations honestly.
```

---

# 53. Final Architectural Principle

The archive subsystem must remain understandable as:

```text
                What operation is requested?
                           │
                           ↓
                 What kind of input is this?
                           │
                           ↓
                What capabilities are needed?
                           │
                           ↓
             Which compiled backend can do it?
                           │
                           ↓
                Apply Toss safety policy
                           │
                           ↓
                       Execute
```

Not:

```text
extension
   ↓
call some library
```

And not:

```text
Toss == libarchive
```

or:

```text
Toss == 7-Zip
```

The long-term invariant is:

> **Toss owns the behavior, safety rules, routing, and domain model. Libraries provide capabilities.**

This allows the archive subsystem to begin small with libarchive, expand naturally to Zstd/LZ4 and additional archive/filter formats, and add a specialized 7-Zip-based backend later if real multi-volume or compatibility requirements justify the additional complexity.

That flexibility must be preserved from Phase 4 onward without prematurely implementing features that Toss does not yet need.
````
