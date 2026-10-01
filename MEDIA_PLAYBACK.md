# Toss Media Playback — Architecture and Decisions (Phase 7)

> This file records decisions that have to stay true for as long as media
> playback exists: the backend choice, what the registry measurement does and
> does not prove, the interaction contract, the audio/video split, the
> dependency and unsafe boundaries, and the milestone that proves the
> lifecycle before anything else is built.
>
> It is not a development log. Commit history carries progress; this carries
> the architecture facts a later agent has to honour without re-litigating
> them. Same standing as `ARCHIVE_BACKEND.md` and `IMAGE_VIEWER.md`.

---

## 1. Scope, and why the backend came first

`Toss_AGENTS.md` Phase 7 says *"Integrate the selected mature media
backend"* — and until 2026-10-01 no backend had been selected. Phase 7 was
therefore scheduled as **verification and decision before code**: media is
v0.1's largest architectural risk, and the decision had to rest on measured
capability on the Windows floor rather than on preference.

```text
play    pause    seek    volume    fullscreen
```

Those five are the minimum, from §18.4. "Do not build codec
implementations" and "keep raw FFI behind a safe backend wrapper" come from
the phase definition itself. **Do not turn v0.1 into a full media-player
project.**

---

## 2. D1: Media Foundation — the selected backend

Adjudicated 2026-10-01 (the full record of that decision):

```text
Phase 7 / D1
  Windows backend
    Media Foundation                    ✓ ACCEPT

  FFmpeg                                ✗ not selected
  system default player                 ✗ not a Toss backend

  Windows 10 19045 registry measurement
    required extensions                 8 / 8 registered
    actual codec playback               not proven by registry
                                        → corpus must verify

  windows-rs
    required MF leaf features           ✓ ACCEPT
    new crates                          0 expected
    Cargo.lock delta                    0 expected
    API > Windows 10 19045 floor        STOP / re-review

  raw MF
    behind safe backend wrapper         required
```

Why Media Foundation wins is the order in `Toss_AGENTS.md` §15 itself:

```text
Windows OS capability
        ↓
Media Foundation
        ↓
0 new crates · 0 sidecar · 0 bundled codec/runtime
single EXE preserved
        ↓
satisfies Phase 7's playback control
```

**FFmpeg is not entering the project now, and not early for Phase 8's sake
either.** "Might be useful later" is not a reason to take on complexity
today, and it collides with §44 (single EXE), §16 (size) and a licence
review (LGPL/GPL against Toss's MIT/Apache) all at once. A system default
player was refused for the same reason it is tempting: it hands `pause`,
`seek` and `volume` to somebody else's program, and Phase 7's five minimum
goals are Toss's to meet.

### The measurement, with its limit stated

What was actually measured, on the machine that *is* the floor:

```text
Windows 10 22H2 (19045) measurement:
the eight v0.1 media extensions have registered
Media Foundation ByteStream handlers.

Actual playback compatibility remains subject to
the codecs present in each media file and must be
validated with the Phase 7 playback corpus.
```

Registered handlers under
`HKLM\SOFTWARE\Microsoft\Windows Media Foundation\ByteStreamHandlers`:
`.mp4 .mkv .webm .avi .mov .mp3 .flac .wav` — 8 of 8, plus thirty-odd more
extensions the system also claims. **This evidence was enough to choose the
backend; it does not replace the playback corpus** (§8). A container is not
a codec: `.mkv` and `.mp4` carry whatever somebody muxed into them, and
"the OS can open the container" says nothing about the elementary streams
inside.

---

## 3. D2: the keyboard-only player

```text
Phase 7 / D2
  Space                                 play / pause
  Left / Right                          fixed-step seek
  Up / Down                             volume
  F                                     fullscreen (video)
  Esc / close                           stop + exit

  progress bar                          not v0.1
  volume control UI                     not v0.1
  playlist                              not v0.1
  subtitles UI                          not v0.1
  OSD                                   not v0.1
```

The same strategy Phase 6 used: prove Toss owns a reliable playback
lifecycle and a minimal interaction before building a media player. The
controls that are absent here are absent on purpose, not deferred
mysteriously — §40.

**Seek step: ±5 seconds, fixed.** Recorded here before implementation, as
required by that adjudication. No acceleration curve, no percentage-of-
duration jump, no key-repeat policy — a step a script and a person can both
predict.

---

## 4. One backend for audio and video; the console carries audio

Settle this before code, not during it:

- **Audio and video share the same Media Foundation backend/session
  abstraction.** No `AudioBackend`, no second pipeline for MP3/FLAC/WAV —
  a separate backend for files that differ only in which stream they carry
  is the kind of split `Toss_AGENTS.md` §42 warns against.
- **Presentation follows the content:**

  ```text
  video        → session → Toss video window → picture in the client area
  audio-only   → same session → no dummy black window
  ```

  An audio file gets **no meaningless window** — the console is the textual
  companion, exactly the judgement §12 reached for the image viewer: a
  graphic window exists only when the content itself needs one (§3).

- **The console summary looks like the image viewer's** (§12's format), and
  it stays honest by the same rule:

  ```text
  Toss — Media

  File      song.mp3
  Format    MP3
  Duration  03:42

  Playing

  Controls
    Space       Play / pause
    Left/Right  Seek
    Up/Down     Volume
    Esc         Stop and close
  ```

- **No metadata subsystem.** Duration (or anything else) is printed only if
  the session provides it as an ordinary part of doing its job. Toss does
  not grow a metadata parser so a console line can be longer — capabilities
  arrive from real needs, not from what a backend happens to expose.

---

## 5. Platform boundary

The Phase 6 shape, unchanged in kind:

```text
CLI / detection / dispatch
          │
          ▼
    handlers/media.rs         portable: picks the file, builds the request
          │                   no cfg, no unsafe, no Windows types
          ▼
    platform/media.rs         portable capability facade
          │                   the only #[cfg(all(windows, feature = "..."))]
          ▼
    platform/windows/media/   raw MF behind the safe wrapper
```

Same hard rules as `IMAGE_VIEWER.md` §3: no `HWND`, `HRESULT`, `IMF*`,
`WM_*` or any other Windows type above `platform/media.rs`, and
`tests/platform_boundary.rs` reads the sources to enforce it — because
`cfg`'d-out code never compiles on Linux, where the compiler could catch a
leak.

`Kind::Media` currently falls through to the info fallback with
`not_implemented` (exit 3). Wiring it to the media handler changes that on
Windows-with-the-feature only; everywhere else the answer stays exit 3 with
the same sentence (§8 of `IMAGE_VIEWER.md` records the equivalent red line
for images).

---

## 6. Dependency decision: windows-rs leaves for Media Foundation

Granted together with D1, under the rules Phase 6 established.

**The leaves P7-A actually needed — four, recorded as they landed:**

| leaf | why it is needed |
|---|---|
| `Win32_Media_MediaFoundation` | the capability itself |
| `Win32_System_Com_StructuredStorage` | `PROPVARIANT`, in `IMFMediaSession::Start` and `IMFMediaEvent::GetValue` |
| `Win32_System_Variant` | `IMFMediaSession::Start` is **not even emitted** without it — the method carries `#[cfg(all(StructuredStorage, Variant))]`, so its absence presents as "no such method", not as a missing type |
| `Win32_UI_Shell_PropertiesSystem` | `IPropertyStore`, in `IMFSourceResolver::CreateObjectFromURL`'s signature |

**Minimum supported client, verified from Microsoft Learn (2026-10-01), not
from memory** — the D1 condition, checked per declaration:

```text
MFStartup                                Windows Vista
IMFMediaSession::Start                   Windows Vista
IMFSourceResolver::CreateObjectFromURL   Windows Vista
MFCreateTopologyNode                     Windows Vista
MFCreateAudioRendererActivate            no separate Learn page (two candidate
                                         URLs 404) — the MF audio renderer is
                                         Vista-era; recorded as a gap rather
                                         than asserted
```

Every documented one is Vista — far under the 19045 floor — **so no API in
this milestone forced a re-choice.** The undocumentable one stays a noted
gap: if a later machine ever rejects it, the floor rule stops the milestone
rather than raising the floor.

Two facts Learn settled while the code was being written, both worth keeping
next to the calls they bless:

- `IMFMediaSession::Start`: `pguidTimeFormat` **may be NULL**;
  `pvarStartPosition` is a `PROPVARIANT` where **VT_EMPTY** means "start from
  the current position" — a *null* position is refused (`E_POINTER`,
  measured). The code passes an empty `PROPVARIANT`, which is that clause.
- `CreateObjectFromURL`: `dwFlags` **must contain** `MF_RESOLUTION_MEDIASOURCE`
  or `MF_RESOLUTION_BYTESTREAM`. Zero worked for five containers and the
  documented example passes the former, so the flag is what the contract
  says, not what happened to work.

- **Only the leaf features actually called** to reach the MF APIs used.
  Nothing widened for convenience.
- **0 new crates expected, `Cargo.lock` delta 0 expected.** The `windows`
  crate is already in the closure; this is more of its surface, and that is
  exactly what "widening" means — which is why it was decided rather than
  edited.
- **Every API's minimum supported client is recorded next to its use.**
  Any API above Windows 10 build 19045 **stops the milestone and forces a
  re-choice** — the floor is not raised quietly to make an API fit.

---

## 7. unsafe and FFI policy

Phase 7's own instruction: *keep raw FFI behind a safe backend wrapper*.

```text
platform/windows/media/...    every unsafe block of Phase 7
platform/media.rs             cfg only, no unsafe
handlers/ core/ dispatch/     zero unsafe, zero Windows types
```

The layering is `windows-rs declarations → backend wrapper → capability
facade → handler` (§32 of `Toss_AGENTS.md`), so a raw pointer never reaches
a handler. The documentation lints that guard this
(`clippy::undocumented_unsafe_blocks`, `rust::unsafe_op_in_unsafe_fn`)
apply unchanged.

The dangerous parts of this phase are not the key handling. They are
**MF's asynchronous lifetime**: COM ownership across `GetEvent`, session
shutdown ordering, and an event pump that must not outlive what it reads.
That is what the first milestone exists to prove (§9).

---

## 8. The playback corpus

The registry measurement (§2) chose the backend. **The corpus proves
playback.** Committed bytes are the fixture — the same rule
`tests/corpus/image/generate.py` states: generation may use whatever tools
are at hand, `cargo test` uses nothing but the committed files.

Nine committed samples, one per format §18.4 lists plus the one that must
fail:

```text
tone.wav      176,478 B   44100 Hz 16-bit stereo PCM   audio only
tone.mp3       16,970 B   LAME                         audio only
tone.flac      23,834 B   FLAC                         audio only
tone.mp4       18,016 B   H.264 + AAC                  video
tone.mkv       17,302 B   H.264 + AAC                  video
tone.webm      20,453 B   VP9 + Opus                   video
tone.avi       70,530 B   MPEG-4 + MP3                 video
tone.mov       18,067 B   H.264 + AAC                  video
truncated.mp4   6,005 B   mp4 cut inside its header — must not decode
```

All synthesised from the same one-second 440 Hz tone and the same 160x120
test picture, so format is the only variable between samples. Regeneration
is `generate.sh` (needs ffmpeg, **generation only** — `cargo test` uses the
committed bytes and nothing else, the rule `image/generate.py` already
states); ffprobe confirms all eight parse and that `truncated.mp4` fails
with `moov atom not found`.

---


### First playback pass (P7-A, measured on the floor machine)

This is the second layer of evidence the D1 limitation asked for — the
registry said the handlers exist; this says what an actual session does:

```text
tone.wav    exit 0   played to MEEndOfPresentation
tone.mp3    exit 0   played
tone.flac   exit 0   played
tone.mp4    exit 0   played
tone.mov    exit 0   played
tone.mkv    exit 3   MF_E_INVALIDMEDIATYPE, in the event pump
tone.webm   exit 3   MF_E_INVALIDMEDIATYPE, in the event pump
tone.avi    exit 3   MF_E_INVALIDMEDIATYPE, in the event pump
truncated   exit 6   MF_E_INVALID_POSITION, while opening
```

**8 of 8 containers open** — the bytestream handlers the registry listed
really do create sources. **5 of 8 play to the end on P7-A's topology**,
which is deliberately audio-only (§9): the three that fail do so *after* a
session exists and playback has started, with a media type the audio
renderer will not take — the video-sink boundary this milestone said it
would not cross. Those three are the video milestone's input, not a defect
in the lifecycle. Whether a container-specific splitter detail also
participates (mp4 and mov are video files too, and they pass) is exactly
what the next milestone measures rather than assumes.

## 9. Milestones

**P7-A — the lifecycle, and nothing else.** Deliberately not "the player":
before a window, before a key, the phase must prove the part that is
actually dangerous.

```text
Media Foundation startup/shutdown
        ↓
safe MediaBackend lifetime
        ↓
open a real corpus media file
        ↓
establish a playback session
        ↓
receive / translate asynchronous events
        ↓
clean teardown
```

MF ownership, COM lifetime, the async event pump and error mapping — proven
correct first. Video window, keyboard controls and the console companion
come in later milestones, and **P7-A does not grow into them sideways**.

**P7-A complete (2026-10-01).** Each of the six steps, measured on the
floor machine:

```text
MF startup / shutdown           RAII pair, balanced on every path out
safe MediaBackend lifetime      MediaFoundation + Session, Drop as the net
open a real corpus file         tone.wav through the source resolver
playback session                MFCreateMediaSession → SetTopology → Start
asynchronous events             GetEvent loop to MEEndOfPresentation,
                                then to MESessionClosed after Close
clean teardown                  exit 0 in ~1.1 s for a 1.0 s file
```

The error mapping was pinned by running the failures through it — the
three numbers §24 tables, from the real binary:

```text
toss <truncated.mp4>      exit 6   corrupt, while opening
toss play Cargo.toml      exit 3   unsupported, MF found no format it knows
toss play <directory>     exit 2   not a media file, before any media code
toss <tone.wav>           exit 0
```

Three wrong turns are recorded because each taught something the code now
says out loud: a source node missing its presentation descriptor
(`MF_E_TOPO_MISSING_PRESENTATION_DESCRIPTOR`), then missing its stream
descriptor (`..._STREAM_DESCRIPTOR`), then a null start position
(`E_POINTER`). The fix for each is a comment at the call it fixed.

An `#[ignore]`d test — `plays_a_tone_all_the_way_to_its_end` — carries the
lifecycle into `cargo test -- --ignored` on any machine with a sound card;
CI runs the four failure paths instead, because a runner has no audio
endpoint and a red build there would say more about the runner than about
Toss.

---


**P7-B complete (2026-10-01).** The video path, measured on the floor
machine:

```text
8 of 8 corpus formats play to the end, exit 0    (wav mp3 flac mp4 mkv webm avi mov)
truncated.mp4                                    exit 6, while opening
video window appears, EVR paints                  6/6 sampled pixels non-black
audio-only files                                  still windowless (D2) — P7-A's
                                                  blocking pump, unchanged
```

### The lesson: three numbering domains, one attribute name

P7-B spent most of its experiments on a bug that presented as *two*
independent failures — `MF_E_STREAMSINKS_FIXED` on one container,
`MF_E_TOPO_CODEC_NOT_FOUND` on another — and was neither:

```text
MF_TOPONODE_STREAMID on a SOURCE node
    = which stream of the source this node stands for (a PD index)
MF_TOPONODE_STREAMID on an OUTPUT node
    = which stream sink of the *renderer* — default 0 when omitted
source stream identifier (IMFStreamDescriptor::GetStreamIdentifier)
    = something else again, and belongs to nobody's node
```

The code set the **PD index on both node kinds**. Renderers here (EVR, the
audio renderer) are fixed-sink devices with **sink 0 only**, so any branch
whose stream happened to sit at PD index 1 — which for MP4 on this machine
means the *video* stream, because MF enumerates MP4 as audio-first while
ffprobe reports container order — asked for a sink that does not exist. The
two "modes" were the two containers' different stream orders hitting the same
mistake.

Verified against Microsoft's own sample
(`Windows-classic-samples/.../protectedplayback/Player.cpp`) and the
"Creating Source Nodes" / "Creating Output Nodes" pages: the official
source node sets **exactly three** attributes (SOURCE, PD, SD), and the
official output node **never writes a stream id at all** — its
`GetStreamIdentifier` call exists *"just for debugging"*, and the omitted
attribute means sink 0. After deleting both writes, every format above went
green at once.

Also worth keeping: **`MF_E_STREAMSINKS_FIXED` (0xC00D4A3B)** is the
sink-side complaint ("these stream sinks are fixed, do not add or remove
them") — when a topology asks a fixed sink for a stream it does not have,
this is what it says.


### Unresolved compatibility case: `E:\Movie\教室别恋...mkv` (6.92 GB)

Kept as the user's real-world sample and **not** worked around
(`MEDIA_PLAYBACK.md` is also where the "no more MF special cases" ruling
lives):

```text
the file is HEVC (ffprobe: hevc, level 4.1, yuv420p) + DTS/AC3 audio
both the full topology and the video-only retry answer
    MF_E_TOPO_CODEC_NOT_FOUND  ->  exit 3 "unsupported format"
C:\Windows\System32 ships no *hevc* decoder file
a video-only remux (-c copy, no re-encode) fails identically
    -> the container, the multi-track layout and the retry are exonerated
```

So the verdict is what §2's limitation already says: **the machine has no
software HEVC decoder**, and Toss reports that as §24's 3 rather than as a
damaged file. What would change it — a bundled software decoder, hardware
MFTs — is exactly the Phase 7 capability investigation, and is not a reason
to add a branch here.
### Measured data (release, `--locked`, both feature sets)

| milestone | `--all-features` | without features | change |
|---|---|---|---|
| end of Phase 6 | 700,928 | 206,848 | — |
| P7-A, the lifecycle (`e6010eb`) | 724,480 | 206,848 | +23,952 / 0 |
| P7-B, video path (`6e79763`) | 741,376 | 206,848 | +16,896 / 0 |

`Cargo.lock` untouched across Phase 7 so far — the four console leaves
accepted with D1 are still the whole dependency change, and a build that
never asked for `media` still does not carry any of it (206,848 B, byte for
byte Phase 6's).
### Choices inside P7-B, stated once

- **The event pump is a timer + `GetEvent(MF_EVENT_FLAG_NO_WAIT)`** on the
  window's thread, not the `BeginGetEvent` callback the Microsoft sample
  uses: video needs a message pump to repaint at all, and a pump that blocks
  on MF events cannot repaint. One thread, no cross-thread marshalling, and
  every event still consumed — at worst 50 ms late, which no one can see.
- **The window is 640x480 and the frame is letterboxed into it.** Reading
  `MF_MT_FRAME_SIZE` is a packed two-value attribute this milestone chose not
  to risk; fitting the window to the frame (and fullscreen) is P7-C/D work.
  Observed: the client rect came back 412x282 on one run rather than the
  requested size — recorded here rather than explained, to be looked at when
  window sizing becomes a real concern.
- **Audio stays on P7-A's blocking pump with no window** — D2 by
  *construction*, not by hiding a window that exists.
---

**P7-C complete (2026-10-01).** D2's five keys, on the real machine — an
8-check probe, four of the five key behaviours automatable (volume's *effect*
is a person's ears; what is asserted for it is that the keys disturb
nothing, while the level clamp itself lives in the backend):

```text
Space              pauses a 1s file (it is still there 1.5s later), and
                   resumes to a clean exit 0
Left / Right       seeks by the fixed 5s step; past the end of a 1s file it
                   ends rather than hangs (see the boundary below)
Up / Down          volume 10% a step, clamped 0.0..=1.0 in the backend;
                   playback undisturbed, clean exit afterwards
F                  fullscreen covers the screen exactly (1707x960 measured
                   against GetSystemMetrics), F restores the saved style and
                   rectangle, Esc stops and exits 0
```

Two things this milestone had to fix to be honest:

- **Closing mid-play used to report a corrupt file.** `IMFMediaSession`
  emits events *during* shutdown whose status is a failure — measured,
  `MF_E_CANNOT_CREATE_SINK` — and the close-wait read them as playback
  errors, so pressing Esc on a healthy file answered exit 6. The wait now
  distinguishes "waiting for playback" (failures are the report) from
  "waiting for the close itself" (failures are the noise of closing), which
  is a semantic difference rather than a swallowed error.
- **Seeking past the end used to hang.** The session accepted a position
  beyond the presentation and then had nothing to play, so the window just
  sat there. The presentation's length is read once from the presentation
  descriptor (`MF_PD_DURATION`) and every seek is clamped into it: asking
  for the end lands on the end.

**Known boundary, recorded rather than papered over:** on a file whose
length *is* the step (every corpus sample is one second long), a seek to the
very end can surface as exit 6 — "corrupt" is the wrong word for "you asked
for the last frame". Real files are minutes long and never meet the edge
this way; when they do, the right answer is a quiet stop at the end, which
is a small change to `seek_to` and belongs with whoever next touches
seeking.

**No new windows-rs feature was needed for P7-C**: the virtual keys live in
`Win32_UI_Input_KeyboardAndMouse` (accepted with M5), fullscreen uses
`GetWindowLongPtrW`/`SetWindowPos`/`GetSystemMetrics` (already present), and
the volume interface sits inside `MediaFoundation` itself — the Audio-module
`IAudioStreamVolume` was never required, because MF exposes
`MR_POLICY_VOLUME_SERVICE` as a service.
---

### P7-B closure round (2026-10-01)

The follow-up after the three-bug fix landed, driven by the real-machine
report and finished before any codec investigation began:

```text
temp event logging removed          the diagnostics earned their keep and left
8-format regression                 wav mp3 flac mp4 mkv webm avi mov = exit 0,
                                    truncated.mp4 = exit 6
p7c smoke (three runs)              8/8: pause/resume, fullscreen, seek-past-end,
                                    volume keys, Esc
audio-only files                    no window for the whole playback (D2), exit 0
F = fullscreen                      window covers the monitor exactly
                                    (2560x1440 measured), restores its previous
                                    geometry on the second press, Esc exits 0
fmt + clippy (both) + tests         Windows and Linux, both feature sets, green
```

**Full screen, done the way the API prescribes.** `SetFullscreen` switches
the *renderer* into D3D exclusive mode; the application, by the same
document's words, must "resize the video window to cover the entire area of
the monitor", make it topmost and give it the focus — restoring the geometry
on the way out. That split is now what the code does: the backend owns the
switch, the player owns the geometry, and **the window style is never
touched** — driving it to `WS_POPUP` was what made the first attempt's edges
look like another operating system's. The renderer's transition is
asynchronous (measured: settled between 0.3s and 1.2s), so anything reading
the rectangle has to wait for it.

**Volume is best-effort.** `MR_POLICY_VOLUME_SERVICE` answered
`E_NOINTERFACE` in one measured state, and a level Toss cannot adjust is not
a reason to stop someone's playback: the key now does nothing rather than
ending the session. The console companion will have a place to say so when
it exists.

**One observation, recorded not fixed:** after a fullscreen round trip
(EVR in and out of exclusive mode), a *subsequent* instance's clean close
took longer than the usual ~1.3s — within a 12-second budget, but past the
8-second one a probe first allowed. Slow, not stuck; worth noticing if a
future probe starts timing closes.

**EVR is legacy, and Microsoft says so.** Every page touched in this work
carries the banner recommending `IMFMediaEngine` or `MediaPlayer` for new
code. The Media Session + EVR path Toss is on remains functional and is what
D1 chose, but the recommendation belongs on the record before Phase 8 or a
renderer change reopens the question — it is a *documented* migration path,
not a surprise for later.
## 10. Non-goals

Repeated because they are what gets added "while we are in here":

- No FFmpeg, no bundled codecs, no sidecar runtime (§44).
- No progress bar, volume slider, playlist, OSD, subtitle UI, media library.
- No `AudioBackend`; no second pipeline for audio files.
- No metadata subsystem.
- No mouse-driven seeking in v0.1.
- No Phase 8 portability work pulled forward "while MF is open".

---

## 11. What forces a re-decision

Stop and re-approve before continuing if a milestone would change any of:

```text
the dependency closure      (a new crate, or a lockfile change)
the single-binary guarantee (any sidecar file, any bundled runtime)
the platform boundary       (a Windows type above platform/media.rs)
the unsafe model            (unsafe outside the media backend wrapper)
the Windows 19045 floor     (an API whose minimum supported client is higher)
```

Implementation detail inside those lines does not need another round of
discussion.
