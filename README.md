# Toss

**Drop a file on it. Read, view, play or unpack it with one executable.**

Toss is an offline emergency file utility. On Windows, drag a file or directory
onto `toss.exe`, or pass its path on the command line. There is no home screen,
installer, adjacent DLL directory or required Office installation.

It aims to handle common tasks with minimal setup, while keeping the original
input unchanged. It complements specialised applications rather than replacing
their editing tools, format coverage or layout fidelity.

## What it does

The table describes the current development source. An older release executable
may support fewer features.

| Input | Default action |
|---|---|
| ZIP, 7z, RAR | Extract beside the archive into a new directory |
| Directory | Create a 7z archive |
| JPEG, PNG, BMP, GIF | Open the native Windows image viewer |
| MP4, MKV, WebM, AVI, MOV, MP3, FLAC, WAV | Attempt playback through Windows Media Foundation |
| DOCX | Read text, paragraphs and tables in the terminal, with supported picture previews |
| Word 97–2003 DOC | Read supported text, tables and pictures in the terminal |
| PDF | View rasterized pages in the native Windows window |
| Unrecognised input | Show basic file information |

A media extension identifies a container, not a guarantee that its codecs are
available. A bundled HEVC software fallback handles supported MP4/MKV video when
the system decoder is unavailable; **that fallback is video-only, without audio**.

Windows x64 is the executable target. The core and selected backend tests also
build on Linux; native image, PDF-window and media viewing are Windows-only.

## Get started

Download `toss.exe` from [GitHub Releases](https://github.com/vusiif/Toss/releases).
Keep the executable wherever convenient. No installation is required.

```powershell
.\toss.exe photo.png
.\toss.exe movie.mp4
.\toss.exe archive.7z
.\toss.exe .\Documents\
.\toss.exe report.docx
.\toss.exe report.doc
.\toss.exe paper.pdf
```

Explicit commands are available when you want to choose the action:

```powershell
.\toss.exe extract archive.zip
.\toss.exe pack .\Documents\
.\toss.exe view report.docx
.\toss.exe play music.flac
.\toss.exe info unknown.bin
.\toss.exe --help
```

Quote paths containing spaces. Use a path such as `.\-report.docx` for a filename
beginning with a hyphen. The advertised `hash` verb is not implemented yet.

## Word terminal reading

Terminal reading remains a core feature. It provides a lightweight view of a
document's content, rather than reproducing Word's paper layout.

| Key | Action in document reading mode |
|---|---|
| Up / Down | Scroll |
| Left / Right | Previous / next screen page |
| `<` / `>` | Select the previous / next picture |
| Enter | Open the selected supported original picture in the native viewer |
| `s` | Search |
| `n` / `p` | Next / previous search result |
| `h` / F1 | Help |
| `q` | Quit |
| F5 | On Windows full builds, convert the entire DOCX for PDF preview |

Close a picture or PDF window to return to terminal reading. F5 conversion is
on demand, runs locally in a subprocess of the same executable, and requires
no Office installation. It does not save a PDF beside your document.

**Legacy DOC does not support F5 PDF preview.** Save it as DOCX using Word/WPS
first if needed; simply changing the extension does not convert the file.

For plain text, redirect standard output:

```powershell
.\toss.exe report.docx > report.txt
.\toss.exe report.doc > report.txt
```

Redirected Word output contains text and image placeholders, without the
interactive interface or ANSI styling. This is not OCR, and does not extract
text drawn inside pictures. Structured Word/PDF JSON export is not implemented.

## Image and PDF windows

Images fit the window on opening and follow window resizing until manually
zoomed. Use the mouse wheel to zoom and drag to pan; `F` returns to fit mode.
Left / Right browse neighbouring supported images, or turn PDF pages. PDF
navigation stops at the first and last pages.

PDF pages use a fixed 144 dpi raster. Window zoom enlarges that raster, rather
than rendering new detail at every zoom level. Complex pages can take time to
render. Password entry for encrypted PDF files is not supported.

## Media controls

Space pauses/resumes, Left / Right seek by five seconds, Up / Down change
volume, `F` toggles video fullscreen, and Escape closes playback. Volume does
not apply to the silent HEVC software fallback.

## Limits and file handling

- Automatic classification currently uses recognised extensions first, then a
  small file-header probe for unknown extensions. It is not comprehensive
  content-based detection: a DOCX without its extension can be treated as ZIP,
  and a misleading supported extension can select the wrong handler.
- Word reading simplifies layout. Complex charts, drawings, floating objects,
  merged tables and legacy picture formats may be omitted or represented by
  placeholders. Terminal screen pages are not Word's physical paper pages.
- DOCX-to-PDF preview may simplify layout or omit chart details. It is a preview
  aid, not a fidelity guarantee or a public PDF export command.
- Picture decoding and document loading have format-specific byte/count/pixel
  budgets. PDF input is limited to 128 MiB, and each page to 8192 pixels per side
  and 16 megapixels at the fixed rendering scale. These are not hard limits on
  all parser/renderer internal memory allocations.
- Archive extraction checks member paths and skips links, devices and special
  files. Existing output destinations are refused. A failed operation can leave
  partial output; automatic archive extraction currently has no total expanded
  byte limit, so use sufficient free disk space.
- Unknown, encrypted, damaged or unsupported files can be refused with a
  diagnostic. Identifying an extension does not establish that a file is valid.

## Build from source

The manifest currently requires **Rust 1.92 or newer**. Use the checked-in
`Cargo.lock`. The default feature set is intentionally minimal; use `full` for
all current functionality:

```powershell
cargo build --release --locked --features full
cargo test --locked --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo fmt --check
```

The Windows executable is `target\release\toss.exe`. The repository configures
static CRT linking for the Windows MSVC target. Native builds require the MSVC
C/C++ build tools and CMake; the HEVC fallback additionally needs NASM and the
MSYS2 bash/make tools used by `build.rs`. First builds may take several minutes.
Cargo dependencies must be downloaded before using `--offline`.

Features include `archive`, `image`, `media`, `inspect`, `word`, `pdf` and
`word-pdf`. The `full` feature enables the complete current set. Feature names
alone do not promise every planned command is implemented.

## Development and verification

This is an evolving utility. Local development binaries are kept separately
under `dist/`; those binaries, private documents, downloaded corpora and
`.sandbox-test/` results are not source inputs or repository deliverables.
A source update does not itself publish a new release.

Current local checks include Windows headless unit/integration tests and DOC
CLI regression. Those results do not certify every GUI interaction, document
layout, codec, clean-machine setup or platform. Bugs found in everyday use are
valuable; report the Toss version, command, observed behaviour and a shareable
minimal sample. Avoid posting private documents.

## Dependencies and provenance

Toss uses libarchive/zlib/liblzma for archives, Windows imaging and Media
Foundation APIs, and a narrowly configured FFmpeg HEVC fallback. Word terminal
reading adapts **doxx 0.1.4** (MIT, Brandon Greenwell), using docx-rs, and uses
**rwml 0.1.4** for binary DOC. PDF rendering uses **Hayro 0.8.0**; DOCX PDF
preview uses **docxide-pdf 0.19.0**. Exact transitive versions are in Cargo.lock.

The local **xml-rs 0.8.29** adaptation adds a default-off
`toss-omit-namespace-maps` feature, enabled only for the audited Word reader.
It omits unused namespace snapshots while retaining qualified-name validation;
other consumers must not enable it without review. The upstream crate archive
SHA256 is `e450f9b2ed1dff33c94c12589a87338689467b9c4f5d8a5710bd09a847d2c8a7`.
The doxx 0.1.4 crate archive SHA256 is
`5eb8c750f34bb9a8756fea542618521e8cf0138681f50e9073abbd1a24f4ea6b`.
Vendored doxx contains Toss-specific loader, rendering, resource-budget and UI
adaptations; it is not an unmodified upstream distribution.

Original licence texts are retained with vendored sources, including
[the doxx licence](third_party/doxx/LICENSE),
[the xml-rs licence](third_party/xml-rs/LICENSE), and
[PDF asset licensing](third_party/pdf-assets/LICENSE.txt).
The three binary DOC test fixtures come from rwml 0.1.4's repository-owned
synthetic benchmark corpus, exported with LibreOffice's Word 97 filter;
[their MIT licence](tests/fixtures/word-legacy/LICENSE-rwml) is retained.
They contain no user documents. Their tests check selected body/cell text,
not complete floating-object layout or Word pagination.

Third-party components retain their respective licences. Binary release
packaging must provide the applicable notices and corresponding source; a
single-executable runtime does not remove those distribution obligations.

## Licence

Copyright (C) 2026 IceLolly.

Toss is licensed under **GNU GPL version 3 or, at your option, any later version**.
See [LICENSE](LICENSE). SPDX: `GPL-3.0-or-later`.
