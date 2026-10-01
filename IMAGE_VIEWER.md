# Toss Image Viewer — Architecture and Decisions (Phase 6)

> This file records decisions that have to stay true for as long as the image
> viewer exists: the Windows baseline, the capability choice, the platform
> boundary, the dependency decision and its measured cost, and what is
> explicitly not built.
>
> It is not a development log. Commit history carries progress; this carries
> the architecture facts a later agent has to honour without re-litigating
> them.

---

## 1. Scope and Windows baseline

Phase 6 delivers the first Toss operation that needs a graphical window:

```text
toss image.png  →  classify as Image  →  one native window  →  show it
```

Minimum behaviour (`Toss_AGENTS.md` §18.3 and Phase 6):

```text
open    zoom    pan    previous / next
```

Formats: `.jpg` `.jpeg` `.png` `.bmp` `.gif` — decoded by WIC, which Windows
already ships. **No image codec dependency is added.**

**Windows compatibility floor: Windows 10 22H2 / build 19045 and above.**

- No fallback design for older Windows; no dynamic API probing; no
  `legacy_windows` backend; no compatibility shims.
- 19045 is the ceiling on what may be used, not a reason to hunt for new
  APIs. A mature older Win32/WIC/GDI call that solves the problem wins.
- Every API actually used must have its minimum supported client recorded in
  `src/platform/windows/image/raw.rs` next to its declaration, with the
  Microsoft Learn reference. Any API whose floor exceeds 19045 stops the
  milestone and forces a re-choice.
- Confirmed at decision time:

  | API | Minimum supported client | vs build 19045 |
  |---|---|---|
  | `SetProcessDpiAwarenessContext` | Windows 10, version 1703 (15063) | under |
  | `GetDpiForWindow` | Windows 10, version 1607 (14393) | under |
  | `IWICImagingFactory` (WIC) | Windows XP with SP2 / Vista | under |

  The remaining calls (window class, message pump, GDI, COM) are Windows
  2000/NT era and are verified per declaration as they are written, not
  asserted from memory.

**Explicitly out of scope for Phase 6:** image editing, format conversion,
metadata editing, thumbnail databases, favourites/galleries, cross-platform
GUI frameworks, a Linux/macOS viewer, legacy-Windows support, and anything
belonging to Phase 7 media playback.

---

## 2. Capability choice: WIC to decode, GDI to render

```text
image decode    → WIC   (Windows Imaging Component, built in)
window / input  → Win32 (user32)
rendering       → GDI   (StretchDIBits)
```

**Direct2D is deliberately not used in Phase 6.**

The requirement is to show decoded pixels reliably, not to build a rendering
framework. GDI satisfies every Phase 6 acceptance criterion; Direct2D does not
buy anything that Phase 6 measures. If a *measured* capability gap appears —
scaling quality, performance, DPI behaviour — Direct2D is re-evaluated then,
against the same dependency gate as everything else.

Until that day: **no D2D-shaped abstraction is written "in case".** When and
if the renderer changes, the platform boundary (§3) is what protects the
layers above; nothing above `platform::image` may need to change.

---

## 3. Platform boundary

```text
CLI / detection / dispatch
          │
          ▼
   handlers/image.rs          portable: picks the image set, builds the request
          │                   no cfg, no unsafe, no Windows types
          ▼
   platform/image.rs          portable capability facade
          │                   the only #[cfg(all(windows, feature = "image"))]
          ▼
   platform/windows/image/    raw.rs (declarations) + viewer.rs (window, decode, render)
```

Hard rules:

- `ViewRequest` and `platform::image::view` are the entire surface between the
  two halves. Their signature is `(&ViewRequest) -> Result<(), TossError>`.
- `HWND`, `HRESULT`, `GUID`, `IWIC*`, `ID2D*`, `WPARAM`, `LPARAM`, `WM_*` and
  any other Windows type or constant must not appear in `core/`, `handlers/`,
  `dispatch/`, `detection/`, or in `platform/image.rs`.
- This is enforced by `tests/platform_boundary.rs`, which reads the sources and
  fails the build. `cfg`'d-out code is never compiled on Linux, so the compiler
  alone cannot catch a leak — the guard test can.
- If Linux CI ever goes red because core depends on a Windows API, that is an
  architecture error to fix, not a reason to weaken the Linux leg.

`Cargo.toml` layout that makes this work (measured, see §6):

```toml
[target.'cfg(windows)'.dependencies]
windows = { version = "0.62", optional = true, features = [ ... ] }

[features]
image = ["dep:windows"]
```

`windows` does **not** compile on non-Windows targets, so a plain
`[dependencies]` entry breaks the Linux leg of CI. The target gate plus the
`cfg` in `platform/image.rs` is what keeps both platforms green.

---

## 4. Ownership: Toss owns rules, Windows owns interaction state

```text
Toss owns                                   Windows owns
──────────────────────────────────────      ─────────────────────────────────
the image set and its order                 HWND
prev/next, including the wrap rule          message loop
the zoom ladder and its limits              mouse and keyboard input
the legal bounds of panning                 the current interaction state
which image is open                         the current viewport / pan offset
product error semantics                     WM_* → calls into Toss's rules
                                            WIC decode
                                            GDI rendering
```

"Windows owns cursor/state" is **not** "Windows owns behaviour". A mouse-wheel
event belongs to Windows; *which zoom step comes next, and what the maximum
is*, belongs to Toss. That keeps the principle intact — **Toss owns behaviour;
the platform provides capabilities** — without inventing an
`ImageViewerDelegate` trait for a project that has exactly one implementation.

There is deliberately **no delegate/callback abstraction in Phase 6.** When a
second implementation exists and the need is real, the interaction state moves
up. A hypothetical future is not a reason to build it today (`Toss_AGENTS.md`
§42).

**The red line:** the Windows side holding interaction state must never become
a reason for `handlers/image.rs` to see `HWND`, `WPARAM`, `LPARAM`, WIC or GDI
types. The extension seam Phase 8 needs has to exist at the platform boundary
from the start, not be excavated later.

---

## 5. Navigation rule: previous / next wraps

```text
next(i, n)     = (i + 1) % n        previous(i, n) = (i + n - 1) % n
```

- Browsing past the last image continues at the first; backwards past the
  first continues at the last.
- One image → next and previous both stay on that image.
- Zero images is not a state that reaches navigation at all; it is refused
  before a window exists.

The rule is **Toss-owned**, expressed as a pure function at the capability
boundary, not as an emergent property of the Win32 message handler — so a
future Linux or macOS backend inherits the same behaviour for free.

Boundary cases **0 / 1 / N must have tests.**

Two implementation facts, both landed in M6 and both worth knowing before
touching this code:

- **The guard for `n = 0` is inside the rule, not at the call site.** The
  formula still reads `(i + n - 1) % n`, but the implementation steps an
  index that is first taken modulo `n`, which removes both the modulo-by-zero
  panic and any overflow in `i + n` without a branch at every caller — one
  forgotten `if` is all "guarded at the call site" needs to become a panic on
  something the user can influence (`Toss_AGENTS.md` §23).
- **A picture that will not decode blocks the direction it sits in.** `next`
  and `previous` are pure; *stepping onto* a picture means decoding it, and
  until M7 exists a failure leaves the viewer where it is. Walking right into
  `truncated.png` therefore stays put on every press — correct for now (no
  panic, no closed window, no good picture replaced by a broken one) and
  explicitly **not** the finished behaviour: skipping it or reporting it is
  part of M7's error handling, which is where the decision belongs.

---

## 6. Dependency decision: windows-rs

**Decision: use `windows-rs`, with only the features Phase 6 actually needs.**

The deciding factor was not size. It was ABI and COM correctness:

`IWICImagingFactory` has roughly thirty methods; with `IUnknown` that is about
thirty-three vtable slots. Writing those bindings by hand means placing
`CreateDecoderFromFilename` at slot 16 and `CreateFormatConverter` at slot 21
by counting. **An off-by-one compiles cleanly and crashes at run time** — the
one class of error Rust cannot help with. `windows-rs` generates those slots
from Microsoft's own metadata, so ABI correctness is not a thing this project
has to maintain by hand.

That matters because `Toss_AGENTS.md` §2 orders the trade as
**Reliability > Safety > Simplicity > startup > size > features > UI**, and
the requirement for the unsafe surface is that it stay *small, isolated and
auditable*. Forty hand-maintained declarations — thirty-three of them vtable
slots — is neither small nor auditable over time.

`+15 transitive crates` is recorded as a **cost**, not as a veto: Toss has
never been a zero-dependency project (it vendors libarchive, liblzma and
zlib). The rule is that a dependency proves its value, and here the value is
ABI correctness.

The constraint that comes with the decision: **only enable the `windows-rs`
features Phase 6 actually uses. Do not widen the Windows API surface for
convenience.** That makes this list part of the dependency closure (§11), so
each addition is a fresh decision rather than a line edit.

### Feature-set changes during Phase 6 — each one recorded

**M5 added `Win32_UI_Input_KeyboardAndMouse`, for `SetCapture` /
`ReleaseCapture`.** A drag must keep following the pointer once it leaves the
client area, and the button-up must arrive wherever it happens; polling
`MK_LBUTTON` instead would change behaviour at exactly that boundary, which
is a worse trade than the feature costs. Accepted with the evidence measured
at the time:

```text
new crates                 0
Cargo.lock delta           0
dependency closure         still 15 crates
build without `image`      +0 B   (206,848 B, unchanged)
--all-features             694,272 → 695,808 B  (+1,536 B for all of M5)
Windows floor              SetCapture/ReleaseCapture are Windows 2000/NT
                           era calls — far under the 19045 floor
capability bought          reliable drag with the pointer outside the window
```

The capture calls stay in the Windows backend; `platform::image::pan` is
given only numbers (origin, delta, content, viewport) and knows nothing about
a grab. A future backend uses its own pointer-grab mechanism to satisfy the
same rule. **Do not "clean up" this feature as unused-looking** — the only
callers are the `WM_LBUTTONDOWN` / `WM_LBUTTONUP` arms of the viewer, which
`cfg` hides from every build that has no viewer in it.


### Gates checked at decision time — all measured, none assumed

| Gate | Result |
|---|---|
| MSRV (`rust-version = 1.85`) | pass — every `windows*` crate declares `rust-version = 1.82` / 1.74 / 1.71 |
| License | pass — all eleven crates `MIT OR Apache-2.0`, same as Toss |
| Single EXE (§44) | pass — **no `build.rs`, no `links` key, package contains only `src/`**. Unlike libarchive there is no native configure step to cache |
| Runtime sidecar files | pass — links system DLLs only |
| Linux CI | **fails without the target gate** — `windows-future` does not compile on non-Windows. Passes with `[target.'cfg(windows)'.dependencies]` (see §3) |
| `Cargo.lock` | +15 entries (16 lines total including the package itself) |

### Measured cost

Two spikes doing identical work (`CoInitializeEx` + `GetSystemMetrics` +
`CoUninitialize`), same release profile (`strip`, `lto`, `panic = "abort"`,
`codegen-units = 1`):

| | transitive crates | cold `cargo build --release` | binary |
|---|---|---|---|
| hand-written FFI | 0 | 1.7 s | **109,056 B** |
| `windows` 0.62.2 (Phase 6 feature set) | 15 | 11.3 s | **113,152 B** |
| **delta** | **+15** | **+9.6 s** | **+4,096 B (+3.8%)** |

Feature gate means a build without `image` is byte-identical to the
hand-written baseline:

```text
Windows + --features image  → 113,152 B
Windows without the feature → 109,056 B   (windows-rs contributes nothing)
Linux + --features image    → cargo check passes
Linux without the feature   → cargo check passes
```

**This +4,096 B is a floor, not the Phase 6 delta.** The real viewer
instantiates more of the crate. `Toss_AGENTS.md` §16 requires the number to be
re-measured and recorded when it moves; §9 below holds the measurement.

---

## 7. unsafe and FFI policy

There is no `raw.rs`, and there will not be one. `windows-rs` **is** the raw
declaration layer — that is most of what the §6 decision bought.

```text
platform/windows/image/viewer.rs   every unsafe block in Phase 6
platform/windows/image/mod.rs      no unsafe; only the call into the viewer
platform/image.rs                  cfg only, no unsafe
handlers/ core/ dispatch/ detection/   zero unsafe, zero Windows types
```

- Layering is `windows-rs declarations → viewer → capability facade → handler`,
  so a raw pointer never reaches a handler (§32 of `Toss_AGENTS.md`).
- Two existing lints enforce the documentation half:
  `clippy::undocumented_unsafe_blocks` — M1 tripped it twice, and both were
  fixed rather than allowed out — and `rust::unsafe_op_in_unsafe_fn`.
- What remains unsafe is lifetime-bearing work: a window handle that must be
  live when it is used, a struct whose fields the system reads across the
  call. That is the part a human has to review; the ABI itself is generated.
- Handles the system owns — the module handle, the shared cursor, the stock
  brush — are never freed here. The viewer does not pretend to own what it
  does not own.

---

## 8. Verification

Every milestone is verified on **both platforms** before it is called done:

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings          (with and without features)
cargo test                                         (with and without features)
```

**Windows CI** covers compile, lint, unit tests, the release build, the
automated binary-size record, and the exit-code smoke test — all of which
already exist in `.github/workflows/ci.yml`.

The viewer is split so that CI can test the part a headless runner can
exercise:

- `decode(path) -> pixels + dimensions` — **no window**, runs against
  `tests/corpus/image/` in CI and proves WIC is wired correctly;
- the window itself — see below.

**The window is measured, not eyeballed.** A separate process that has set
its own DPI awareness can find the window by class and title, read its client
size, read pixels back out of its device context with `GetPixel`, and post
`WM_CLOSE` — so "is the picture actually on screen, in the right place, with
the right colours" is an assertion rather than an opinion. M3's smoke checked
three pixels of a gradient against the values the fixture was generated from,
and the client rectangle against the image's own dimensions.

Two traps that smoke run ran into, both worth remembering:

- a **DPI-unaware** probe reads a virtualised rectangle — a 320x200 client
  came back as 213x133 on a 150% display, which looks exactly like a sizing
  bug and is not one. The probe must set DPI awareness before it asks;
- `WM_CLOSE` posted from outside walks the same chain as the title-bar cross,
  so the exit code it produces is the one a person would get.

M5's drag smoke added two more, and they are the ones a later probe will hit
first:

- **`FindWindowW` returned NULL for a window `EnumWindows` could see** —
  same class, same session, visible, with `MainWindowHandle` populated. The
  probe now enumerates and matches on **pid + class name**, which also stops
  an older viewer still on screen from answering for this one;
- **`GetPixel` lives in `gdi32.dll`, not `user32.dll`** — declaring it
  against user32 compiles and then fails at run time with
  `EntryPointNotFoundException`.

M5's assertions, in the order they ran: the client rectangle is the image's
own size; the picture reads back at 1:1; **a drag at 1:1 moves nothing** (no
room to pan, so the rule refuses it); the anchor survives two zooms (M4
unchanged); a drag inside the range moves the picture — reading back
`(9, 8, 128)`, the gradient value `panel.png` has at the source pixel that
origin lands on; a drag far past the edge and a second one past it land in
the **same** place, which is the clamp; zooming out brings the picture home;
exit 0.

M6's walk, printed before *and* after each key so a divergence shows where it
happened rather than only at the end: open on `panel.png` (320x200), walk
backwards through `odd.png`, `odd.bmp`, `block.jpg`, **wrap off the front to
the last image**, wrap forward off the back to `block.jpg`, and forward all
the way home to an exact 320x200 again; then into `small.gif` and twice into
`truncated.png`, where the viewer holds still (§5 records why, and what M7
changes); then a key Toss does not own, which changes nothing; exit 0. The
small pictures come back at the system's minimum window size — 180 px wide
here, not 5 — so only `panel.png` is checked for an exact rectangle: that is
the OS enforcing `SM_CXMINTRACK`, not Toss getting a size wrong.

A real desktop session is still what M8 signs off: it catches what no probe
does — a window that opens behind others, a cursor that never changes, a
picture that is present but wrong in a way nobody thought to sample.

**Linux CI** is the portability guard. It must stay green, and
`toss <image>` there must be *defined* behaviour — exit 3,
`not implemented yet: image viewing` — never a panic. Linux is not required to
view images (Phase 6); it is required to keep compiling and to prove the core
stayed clean.

**MSRV CI** runs `cargo check --all-features` on Ubuntu, where the target gate
hides `windows` entirely.

---

## 9. Measured data

Release `cargo build --release --locked`, both feature sets, measured at each
milestone rather than accumulated:

| Milestone | `--all-features` | without features | change |
|---|---|---|---|
| before Phase 6 (`973b910`) | 664,064 | 189,952 | — |
| M0, the boundary (`471cab1`) | 677,376 | 206,848 | +13,312 / +16,896 |
| M1, the window (`0d78668`) | 685,056 | 206,848 | +7,680 / 0 |
| M2, WIC decode (`0ea8620`) | 691,712 | 206,848 | +6,656 / 0 |
| M3, first render | 693,248 | 206,848 | +1,536 / 0 |
| M4, zoom | 694,272 | 206,848 | +1,024 / 0 |
| M5, pan | 695,808 | 206,848 | +1,536 / 0 |
| M6, previous / next | 698,880 | 206,848 | +3,072 / 0 |

Plus, from the §6 spike: `windows-rs` floor **+4,096 B**, transitive crates
**+15**.

Two things this table is honest about:

- **M0 grew the small build more than the large one** (+16,896 against
  +13,312). The totals are measured; *why* the two moved by different amounts
  is not, and it is not decomposed here. `ARCHIVE_BACKEND.md` §56.4 set the
  standard: a composition that is reasoned rather than measured says so.
- **M1's window code costs 0 bytes without the feature**, which is what the
  target-gated optional dependency is for: a build that never asked for the
  viewer does not carry it.

Milestones M2 onward append rows here as they land, and the Phase 6 completion
report closes the table.

---

## 10. Non-goals

Repeated here because they are the things most likely to be added "while we
are in here":

- No editing, no conversion, no metadata editing, no thumbnails, no gallery.
- No homepage, no launcher, no tool picker (`Toss_AGENTS.md` §43).
- No second image backend, no Linux/macOS viewer, no legacy-Windows layer.
- No Direct2D until a measured gap demands it (§2).
- No delegate trait until a second implementation demands it (§4).
- No Phase 7 media work (§14 of the Phase 6 guide).

---

## 11. What forces a re-decision

Stop and re-approve before continuing if a milestone would change any of:

```text
the dependency closure      (adding or widening a crate)
the single-binary guarantee (any sidecar file, any build script, any native step)
the platform boundary       (a Windows type above platform/image.rs)
the unsafe model            (unsafe outside viewer.rs)
the Windows 19045 floor     (an API whose minimum supported client is higher)
```

Implementation detail inside those lines does not need another round of
discussion.
