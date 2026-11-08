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
| XLSX, XLSM, CSV, TSV | Read tables in the terminal; redirect output to first-sheet CSV |
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

The current literal case-insensitive search finds all non-overlapping matches,
using original UTF-8 byte positions. It uses Unicode simple case folding, without
locale-specific or normalization equivalence. Search retains up to 4096 results
and 16 MiB of shared source text, and reports a reached limit. Paragraph hits
navigate to their wrapped line; table hits navigate to their row. Per-cell table
highlighting is not implemented.

## Image and PDF windows

The native image decoder limits output to 32,768 pixels per side, 64 Mi pixels
and 256 MiB of BGRA storage, with recoverable buffer allocation errors. This
does not cap all allocations inside Windows image codecs.

Images fit the window on opening and follow window resizing until manually
zoomed. Use the mouse wheel to zoom and drag to pan; `F` returns to fit mode.
Left / Right browse neighbouring supported images, or turn PDF pages. PDF
navigation stops at the first and last pages.

PDF pages use a fixed 144 dpi raster. Window zoom enlarges that raster, rather
than rendering new detail at every zoom level. Complex pages can take time to
render. Password entry for encrypted PDF files is not supported.

## Media controls

For video, Space pauses/resumes, Left / Right seek by five seconds, Up / Down
change volume, `F` toggles fullscreen, and Escape closes playback. Volume does
not apply to the silent HEVC software fallback.

Pure audio uses the existing console, without opening a video window:

| Key | Audio action |
|---|---|
| Space | Pause / resume |
| Left / Right | Seek backward / forward five seconds |
| Up / Down | Change renderer volume by ten percentage points, clamped to 0–100% |
| Esc, `q`, Ctrl+C | Stop and exit |

A seek while paused returns to paused playback. Seeking to/past a known end
finishes playback. Volume is best-effort when the system renderer does not expose
its control service. Commands wait for asynchronous session acknowledgements;
held Space does not repeatedly toggle pause, and input processing is bounded.
Normal exit restores the previous console input mode. Playback status goes to
stderr, with no progress-bar TUI or playlist.

Keyboard input is enabled only when standard input is a real console. With a
pipe, file, null stdin or no console, audio plays to completion and does not read
stdin or allocate a replacement console. Source files are opened read-only.

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
  files. Trial defaults are 64 GiB total output, 32 GiB per file and 100,000
  members, counting directories and skipped entries too. Declared sizes are
  checked before extraction; actual output is checked again before each write.
- Extraction first writes to a unique temporary directory beside the destination
  and commits it without replacing any existing target. Ordinary failures clean
  up this operation's staging directory; interruption or cleanup errors can leave
  a temporary directory, whose path should be retained for diagnosis. Directory
  packing likewise stages a closed, nonempty archive beside its destination and
  publishes it without replacing an existing output. Ordinary failures remove
  its owned staging directory; abrupt termination can still leave temporary data.
- Space preflight uses declared logical sizes, filesystem allocation units and a
  provisional per-entry allowance. The additional reserve defaults to zero for
  this trial; an empirical safety-margin policy is still pending. Unknown sizes,
  query failures or insufficient estimated space require `y/N` confirmation.
  Noninteractive use refuses that risk unless explicitly accepted. This estimate
  does not guarantee sufficient space or bound the decoder's internal memory/time.
- Unknown, encrypted, damaged or unsupported files can be refused with a
  diagnostic. Identifying an extension does not establish that a file is valid.

Extraction limits can be adjusted explicitly (sizes are decimal bytes):

```powershell
.\toss.exe extract archive.zip --max-total-bytes 68719476736 --max-file-bytes 34359738368 --max-entries 100000
.\toss.exe extract archive.zip --reserve-bytes 1073741824
.\toss.exe extract archive.zip --accept-space-risk
```

`--accept-space-risk` bypasses only the space-risk prompt, never the hard output
limits. The first command shows trial defaults; the reserve example is a user
choice, not a recommended universal value. No fixed compression-ratio multiplier
is used. Native transactional extraction is implemented for Windows/Linux/macOS;
this change has been exercised on Windows only so far.

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

Features include `archive`, `image`, `media`, `inspect`, `word`, `pdf`,
`word-pdf` and `spreadsheet`. The `full` feature enables the complete current set. Feature names
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
preview uses **docxide-pdf 0.19.0**. The internal **toss-sheet** viewer selectively
adapts xleak 0.2.6, using the recorded Calamine 0.34.0 adaptation. Exact transitive
versions are in Cargo.lock.

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


### Excel / CSV tables

Full builds open `.xlsx`, `.xlsm`, `.csv` and `.tsv` directly, including via
`toss view data.xlsx`. In a terminal, arrow keys navigate, Tab switches sheets,
`/` then Enter searches, Esc cancels a search, `?` shows help and `q` quits.
The first occupied row is used as the header. No configuration file is required.

```powershell
.\toss.exe data.xlsx
.\toss.exe view data.csv
.\toss.exe data.xlsx > first-sheet.csv
```

Redirected stdout exports only the first sheet as UTF-8 CSV, without opening the
interactive viewer. Numeric-looking CSV fields remain strings. Formulas use
existing cached values and can show expressions; Toss does not recalculate them.
A missing cache displays as empty. XLSM macros are not executed. Charts and Office
page layout are not reproduced. CSV supports UTF-8 (with optional BOM); legacy
`.xls`, `.xlsb` and `.ods` are not yet routed to the table viewer.
General numeric display retains the parsed value's precision without forced
two-decimal rounding or automatic grouping. The interactive XLSX viewer supports
common percentage, decimal-place, grouping, currency, zero-padding and scientific
formats, including simple negative/zero sections. Unsupported conditions, fractions
and complex accounting formats fall back to raw values; dates retain the existing
normalized date/time display. Search follows displayed values; clipboard and
CSV/JSON retain raw typed values.

Interactive tables start with white text on a black background. Press `T` to toggle
original Excel colors (including default black text on white), or back to black/white.
Colors and borders are prepared on the first switch to original-color mode, keeping
the default information view lighter. Large styled workbooks may briefly pause on
that first switch; subsequent toggles reuse the prepared styles.
Viewer borders and titles stay on a dark background in both modes. Gray row/column
grid lines are reading guides. Explicit XLSX cell borders use the stored edge positions
and colors only in original-color mode. Black/white mode ignores source borders
and uses uniform gray thin grid lines solely for reading.
Thin, thick, double, dotted and dashed lines have terminal approximations; merged-cell
outlines, diagonal borders and exact Excel junction geometry are not reproduced.
Original-color viewing renders cell text colors and solid fills from XLSX styles
and themes (direct RGB/ARGB, indexed palettes, theme colors, and HLS luminance tint).
Header colors follow the source header row when present. Search matches and cursor
selection take visual precedence over cell colors. Excluded features (conditional formatting, pattern/gradient
fills, rich-text run colors, row/column default styles) fall back to the active mode's default colors.
Test scratch output is configurable via `TOSS_SHEET_TEST_TMPDIR` (defaulting to
`.sandbox-test/spreadsheet-unit`).

Trial limits include 64 MiB input, 1 MiB per cell, four million rectangular XLSX
cells, and 250,000 CSV records / two million rectangular CSV cells. XLSX also has
ZIP member and expanded-byte limits. These are admission/display safeguards,
not universal parser memory/CPU guarantees. Small-file startup and first-readable
content are the primary performance targets. A minimal build can enable the
`spreadsheet` Cargo feature separately; no standalone viewer or sidecar is needed.


Table options are available after `view` and require exactly one input file:

Interactive viewing defaults to column-letter labels and keeps the first occupied
row as ordinary selectable/searchable data, preserving source cell coordinates.
Use `--header` to explicitly promote that row to a fixed header. Redirected output
retains the existing first-row-as-header convention for script compatibility;
`--no-header` explicitly exports the first row as data with generated column labels.

```powershell
.\toss.exe view data.xlsx --sheet "Sheet2" --format json > sheet.json
.\toss.exe view data.csv --no-header --delimiter ";"
.\toss.exe view data.xlsx --header
.\toss.exe view data.xlsx --sheet "Sheet2" --format csv > sheet.csv
```

`--sheet` selects an exact worksheet name. `--no-header` treats the first occupied
row as data and generates column-letter headers; exported CSV still includes those
headers. `--delimiter` accepts one ASCII byte and applies only to CSV/TSV. Omit it
for ordinary comma CSV or tab-separated TSV. Explicit `--format csv` or
`--format json` always writes to stdout and exits, including in a terminal.
Without an explicit format, the usual interactive/redirected behavior remains.

JSON contains `sheet`, `rows`, `columns`, `headers` and a two-dimensional `data`
array. Numeric cells and booleans keep their JSON types; CSV text stays text,
including leading zeros. Dates are strings, absent values/caches are `null`, and
explicit empty-string caches remain `""`. Duplicate header names do not overwrite
data cells. This export contains cached values, not formula recalculation or a
complete Excel formatting/chart model. Multiple-file JSON concatenation is not
supported; process one file per command.
