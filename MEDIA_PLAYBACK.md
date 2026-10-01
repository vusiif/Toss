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
