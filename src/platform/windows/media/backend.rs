//! Media Foundation: startup, a session, its events, and teardown.
//!
//! This file is Phase 7's first milestone in full (`MEDIA_PLAYBACK.md` §9):
//! prove the *asynchronous lifetime* — MF ownership, COM lifetime, the event
//! pump and error mapping — before a window or a key is built on top of it.
//!
//! Deliberately absent: any window, any control, any picture. The topology
//! below connects the source to the system's **audio renderer** and nothing
//! else, which is enough to drive a session to its end on a real file and is
//! honest about what P7-A is — the video pipeline arrives with the video
//! milestone, not sideways into this one.

use std::path::Path;
use std::ptr;

use windows::Win32::Foundation::HWND;
use windows::Win32::Media::MediaFoundation::{
    IMFMediaSession, IMFMediaSource, IMFPresentationDescriptor, IMFSimpleAudioVolume,
    IMFStreamDescriptor, IMFTopology, MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS, MEEndOfPresentation,
    MESessionClosed, MF_OBJECT_TYPE, MF_PD_DURATION, MF_RESOLUTION_MEDIASOURCE,
    MF_TOPOLOGY_OUTPUT_NODE, MF_TOPOLOGY_SOURCESTREAM_NODE, MF_TOPONODE_NOSHUTDOWN_ON_REMOVE,
    MF_TOPONODE_PRESENTATION_DESCRIPTOR, MF_TOPONODE_SOURCE, MF_TOPONODE_STREAM_DESCRIPTOR,
    MF_VERSION, MFCreateAudioRendererActivate, MFCreateMediaSession, MFCreateSourceResolver,
    MFCreateTopology, MFCreateTopologyNode, MFCreateVideoRendererActivate, MFGetService,
    MFMediaType_Audio, MFMediaType_Video, MFSTARTUP_FULL, MFShutdown, MFStartup,
    MR_POLICY_VOLUME_SERVICE,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Variant::VT_I8;
use windows::core::{IUnknown, Interface, PCWSTR};

use crate::core::error::TossError;

/// Play `path` to its end, then close everything down in MF's own order.
///
/// The sequence is the milestone (`MEDIA_PLAYBACK.md` §9): platform up,
/// file open, session created, topology resolved, playback started, the
/// event pump run to the end of the presentation, then close — waiting for
/// the session to agree it is closed — and finally the releases.
pub(super) fn play(path: &Path) -> Result<(), TossError> {
    let _foundation = MediaFoundation::new(path)?;

    // Open before the session exists: a file that cannot be opened should
    // cost no session at all (§5's "the safest useful default" applies to
    // failures too — spend the least, report the most).
    let source = open_source(path)?;

    let (descriptor, planned, duration) = probe(path, &source)?;
    let duration = duration.min(i64::MAX as u64) as i64;
    let has_video = planned.iter().any(|stream| stream.video);

    let session = Session::new(path)?;

    if has_video {
        // The video path. The window exists because the EVR needs one —
        // `MFCreateVideoRendererActivate` takes an hwnd and there is no
        // video without that — and the event pump runs off the window's
        // timer, because video also needs a message pump to refresh at all.
        // Keyboard controls are P7-C; what stops this play in the meantime
        // is the end of the presentation or the title-bar cross.
        let window = super::player::Window::new(path)?;
        let topology = topology(path, &source, &descriptor, &planned, Some(window.handle()))?;
        start(&session, path, &topology)?;

        // Blocks until the window is gone — the presentation's end destroys
        // it, and so does the cross — and surfaces any failure the event
        // pump recorded on its way.
        super::player::run(window, session.inner.clone(), path, duration)?;

        session.close(path)?;
    } else {
        // Audio only: no window at all, and the blocking pump P7-A proved.
        // D2's "an audio file gets no dummy window" stays true by *not
        // taking the video path*, rather than by hiding one.
        let topology = topology(path, &source, &descriptor, &planned, None)?;
        start(&session, path, &topology)?;

        session.wait_for(path, MEEndOfPresentation.0 as u32, false)?;
        session.close(path)?;
    }

    Ok(())
}

/// Hand over the topology, then start from the beginning.
///
/// The documented pairing: `SetTopology` resolves, `Start` begins with a
/// null time format (MF's default) and an empty `PROPVARIANT` — VT_EMPTY,
/// "from the current position", which for a fresh session is the start. A
/// *null* position is refused outright (`E_POINTER`, measured in P7-A).
fn start(session: &Session, path: &Path, topology: &IMFTopology) -> Result<(), TossError> {
    // SAFETY: the session outlives this call and the topology is borrowed
    // only for the hand-over MF takes a reference to; both return `Result`.
    unsafe {
        session
            .inner
            .SetTopology(0, topology)
            .map_err(|err| super::failed(path, "prepare playback", err))?;

        let position = PROPVARIANT::default();
        session
            .inner
            .Start(ptr::null(), &position)
            .map_err(|err| super::failed(path, "start playback", err))?;
    }

    Ok(())
}

/// Media Foundation, started once and shut down when this goes out of scope.
///
/// The same shape as `decode.rs`'s `Apartment`: tied to a value rather than
/// to a code path, so an early `?` cannot leave the platform initialised —
/// and whatever way `play` ends, MF's own pairing runs.
struct MediaFoundation;

impl MediaFoundation {
    fn new(path: &Path) -> Result<Self, TossError> {
        // SAFETY: the documented argument pair for a full startup, on the
        // calling thread; the failure path returns before `Self` exists, so
        // `Drop` only ever balances a startup that happened.
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }
            .map_err(|err| super::failed(path, "start Media Foundation", err))?;

        Ok(Self)
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        // SAFETY: pairs the `MFStartup` above on this same thread, which is
        // MF's requirement. The result is ignored because drop cannot report
        // and the OS reclaims whatever a failed shutdown leaves; every path
        // out of `play` still gets here exactly once.
        unsafe {
            let _ = MFShutdown();
        }
    }
}

/// A playback session with the teardown order it owes MF written down once.
///
/// `MFCreateMediaSession` hands over a session that must be *closed* before
/// it is released — that is MF's rule, not a preference — so [`Self::close`]
/// is the orderly path and `Drop` is the safety net for the error paths that
/// never reach it.
struct Session {
    inner: IMFMediaSession,
}

impl Session {
    fn new(path: &Path) -> Result<Self, TossError> {
        // SAFETY: no configuration object means MF's defaults, which is what
        // a first milestone should be running against.
        let inner = unsafe { MFCreateMediaSession(None) }
            .map_err(|err| super::failed(path, "create a playback session", err))?;

        Ok(Self { inner })
    }

    /// Pump events until one of `kinds` arrives, reporting any event that
    /// carries a failure instead of waiting on.
    ///
    /// This is the whole of "receive / translate asynchronous events" for
    /// P7-A: `GetEvent` with a zero flag blocks — the documented way to wait
    /// — and MF guarantees a terminal event for every session it starts, so
    /// the loop ends at `MEEndOfPresentation` on success or on the first
    /// event whose *status* is a failure (which is how `MEError` carries its
    /// reason). Events that are merely progress — geometry updated, streams
    /// created, session started — pass through without being acted on.
    ///
    /// The failure mode this *cannot* have is a silent hang: an event pump
    /// that waits forever would need MF to break its own guarantee. The
    /// failure mode it *can* have is returning an error the caller maps
    /// through `from_hresult`, which is where §24's number comes from.
    fn wait_for(&self, path: &Path, kind: u32, closing: bool) -> Result<(), TossError> {
        loop {
            // SAFETY: the session is alive for the whole of `play`, and the
            // three calls only read its state and the event it hands back —
            // an event owned by this loop, released when it drops.
            let (arrived, status) = unsafe {
                let event = self
                    .inner
                    .GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))
                    .map_err(|err| super::failed(path, "wait for a media event", err))?;

                let arrived = event
                    .GetType()
                    .map_err(|err| super::failed(path, "read a media event", err))?;

                // `GetStatus` answers two different questions: whether the
                // call worked (the `Result`), and what the event itself
                // reports (the `HRESULT` inside). The second one is where
                // MEError hides its reason, and it is the only place a
                // playback failure surfaces.
                let status = event
                    .GetStatus()
                    .map_err(|err| super::failed(path, "read an event's status", err))?;

                (arrived, status)
            };

            if status.is_err() && !closing {
                return Err(super::from_hresult(path, "play the file", status));
            }

            if arrived == kind {
                return Ok(());
            }
        }
    }

    /// Orderly shutdown: ask, then wait until the session confirms.
    fn close(&self, path: &Path) -> Result<(), TossError> {
        // SAFETY: the session is alive, and closing it is the operation its
        // contract asks for before the release `Drop` will perform.
        unsafe {
            self.inner
                .Close()
                .map_err(|err| super::failed(path, "close the playback session", err))?;
        }

        self.wait_for(path, MESessionClosed.0 as u32, true)
    }
}

/// The audio renderer's master-volume interface, through MF's service
/// lookup.
///
/// `MR_POLICY_VOLUME_SERVICE` is the master-volume service an audio
/// renderer exposes (Microsoft Learn, "Service Interfaces"), and
/// `IMFSimpleAudioVolume` rather than the per-channel
/// `IMFAudioStreamVolume` because D2's volume keys are one up/down control,
/// not a mixer.
fn simple_audio_volume(
    path: &Path,
    session: &IMFMediaSession,
) -> Result<IMFSimpleAudioVolume, TossError> {
    let mut raw = std::ptr::null_mut();

    // SAFETY: `raw` is local out-storage; the IID is the interface being
    // asked for, so the pointer that comes back *is* that interface, and
    // `from_raw` takes over the single reference MF handed us — dropped,
    // and released, when the returned value goes out of scope.
    unsafe {
        MFGetService(
            session,
            &MR_POLICY_VOLUME_SERVICE,
            &IMFSimpleAudioVolume::IID,
            &mut raw,
        )
        .map_err(|err| super::failed(path, "reach the volume control", err))?;

        Ok(IMFSimpleAudioVolume::from_raw(raw))
    }
}

/// A `PROPVARIANT` carrying one absolute position (VT_I8, 100-ns units).
///
/// Built field by field because windows-rs exposes the C layout directly:
/// the variant type in the header, the value in the union behind it. The
/// zeroed default is VT_EMPTY — "no position", which is what `resume`
/// uses; this is the same structure with a position written into it.
fn position_at(target: i64) -> PROPVARIANT {
    let mut position = PROPVARIANT::default();

    // SAFETY: reading a union field is the unsafe step — the field behind
    // `Anonymous` is a `ManuallyDrop` wrapper, so reading it runs no
    // destructor and hands out a plain `&mut` the header can be filled
    // through. What is written afterwards (the variant type, then the value
    // union) needs no further ceremony: the reference above already made the
    // access exclusive.
    let header = unsafe { &mut *position.Anonymous.Anonymous };
    header.vt = VT_I8;
    header.Anonymous.hVal = target;

    position
}
impl Drop for Session {
    fn drop(&mut self) {
        // The safety net: an error path skips `close`, and MF requires a
        // closed session before the release that `Drop` then performs. Close
        // on an already-closed session is refused rather than harmful, so
        // the double-close an orderly run produces costs nothing.
        //
        // SAFETY: the session is alive until this call, and closing it is
        // exactly the operation its contract asks for before release.
        unsafe {
            let _ = self.inner.Close();
        }
    }
}

/// A media source for `path`, opened through MF's source resolver.
///
/// This is the step that decides whether the file *is* something this
/// machine can open — and, because dispatch already decided the file claims
/// to be media (§6), a refusal here says the bytes do not match the claim,
/// which is what `from_hresult` has to translate.
fn open_source(path: &Path) -> Result<IMFMediaSource, TossError> {
    let wide = wide_path(path)?;

    // SAFETY: a factory creation that takes no pointers at all; the resolver
    // is owned by this function and released when it drops.
    let resolver = unsafe { MFCreateSourceResolver() }
        .map_err(|err| super::failed(path, "create a media source resolver", err))?;

    let mut object_type = MF_OBJECT_TYPE(0);
    let mut object: Option<IUnknown> = None;

    // SAFETY: `wide` is a NUL-terminated buffer owned by this function; the
    // out-parameters point at local storage; no properties bag means "none",
    // which the resolver accepts.
    unsafe {
        // The flag the API *requires*: Microsoft Learn states dwFlags "must
        // contain either MF_RESOLUTION_MEDIASOURCE or
        // MF_RESOLUTION_BYTESTREAM", and the documented example passes the
        // former. Zero worked for some containers and not others — measured
        // against the corpus below — so this is not a formality.
        resolver.CreateObjectFromURL(
            PCWSTR(wide.as_ptr()),
            MF_RESOLUTION_MEDIASOURCE.0 as u32,
            None,
            &mut object_type,
            &mut object,
        )
    }
    .map_err(|err| super::failed(path, "open the media file", err))?;

    let object = object.ok_or_else(|| {
        TossError::other("media foundation opened the file but produced no source")
    })?;

    object
        .cast::<IMFMediaSource>()
        .map_err(|_| TossError::other("media foundation produced something that is not a source"))
}

/// One stream P7-B will carry, and whether it needs the video renderer.
///
/// Planned rather than performed: the caller decides where the video
/// renderer's window comes from before any node is built, because a file
/// with video in it is the whole reason a window exists (D2: an audio-only
/// file gets no dummy window).
pub(super) struct PlannedStream {
    pub(super) index: u32,
    pub(super) video: bool,
}

/// Decide which streams this file will play, and select exactly those.
///
/// Every stream starts deselected — including subtitle and data streams this
/// milestone will not connect — so the topology never has a selected stream
/// waiting for a sink nobody built. What is left selected is video and audio
/// only, which is what `topology` then connects.
///
/// Returns the presentation descriptor alongside the plan: the topology
/// needs it, and it belongs to the same decision.
pub(super) fn probe(
    path: &Path,
    source: &IMFMediaSource,
) -> Result<(IMFPresentationDescriptor, Vec<PlannedStream>, u64), TossError> {
    // SAFETY: `source` is a live object borrowed from the caller; every call
    // returns a `Result`; the descriptor handed back is a referenced
    // interface MF gives us ownership of.
    unsafe {
        let descriptor = source
            .CreatePresentationDescriptor()
            .map_err(|err| super::failed(path, "read the presentation descriptor", err))?;

        // How long this presentation is, for clamping a seek into it. A
        // source that does not answer gets u64::MAX, which is the same
        // as no clamp at all — the honest default when the length is
        // genuinely unknown, and every format in the corpus answers.
        let duration = descriptor.GetUINT64(&MF_PD_DURATION).unwrap_or(u64::MAX);

        let count = descriptor
            .GetStreamDescriptorCount()
            .map_err(|err| super::failed(path, "count the streams", err))?;

        for index in 0..count {
            descriptor
                .DeselectStream(index)
                .map_err(|err| super::failed(path, "clear a stream selection", err))?;
        }

        let mut planned: Vec<PlannedStream> = Vec::new();
        for index in 0..count {
            let stream = stream_descriptor(path, &descriptor, index)?;
            let handler = stream
                .GetMediaTypeHandler()
                .map_err(|err| super::failed(path, "read a stream's media types", err))?;
            let major = handler
                .GetMajorType()
                .map_err(|err| super::failed(path, "read a stream's kind", err))?;

            let video = if major == MFMediaType_Video {
                true
            } else if major == MFMediaType_Audio {
                false
            } else {
                // Subtitles, data, unknown kinds: not selected, not
                // connected — not this milestone's problem.
                continue;
            };

            descriptor
                .SelectStream(index)
                .map_err(|err| super::failed(path, "select a stream", err))?;
            planned.push(PlannedStream { index, video });
        }

        if planned.is_empty() {
            return Err(TossError::UnsupportedFormat(path.to_path_buf()));
        }

        Ok((descriptor, planned, duration))
    }
}

/// One stream's descriptor, out of the presentation descriptor.
///
/// Its own function because every source node needs one and MF counts the
/// misses separately (`MF_E_TOPO_MISSING_STREAM_DESCRIPTOR`, measured in
/// P7-A) — one place to get it right.
pub(super) fn stream_descriptor(
    path: &Path,
    descriptor: &IMFPresentationDescriptor,
    index: u32,
) -> Result<IMFStreamDescriptor, TossError> {
    let mut selected = windows::core::BOOL(0);
    let mut stream: Option<IMFStreamDescriptor> = None;

    // SAFETY: the descriptor is live for this call, `selected` is local
    // storage the call fills in, and the interface comes back inside a
    // `Result`.
    unsafe {
        descriptor
            .GetStreamDescriptorByIndex(index, &mut selected, &mut stream)
            .map_err(|err| super::failed(path, "read a stream descriptor", err))?;
    }

    stream.ok_or_else(|| TossError::other("a selected stream has no descriptor"))
}

/// The topology for the planned streams: one source node and one sink per
/// stream, each pair connected on its own.
///
/// `video_window` is the EVR's target — `MFCreateVideoRendererActivate`
/// takes an hwnd, because there is no video without a window somewhere — and
/// the caller supplies it only for a file that actually has video, which is
/// how an audio-only play stays windowless (D2).
///
/// Each source node represents *one* stream: that is MF's model, not a
/// choice — a node carries a single stream descriptor and a single stream
/// id, so N streams mean N nodes, all pointing at the same media source and
/// the same presentation descriptor.
pub(super) fn topology(
    path: &Path,
    source: &IMFMediaSource,
    descriptor: &IMFPresentationDescriptor,
    planned: &[PlannedStream],
    video_window: Option<HWND>,
) -> Result<IMFTopology, TossError> {
    // SAFETY: everything built here is local — the topology and every node
    // outlive these calls — while `source`, `descriptor` and each stream
    // descriptor are borrowed only for attachments the topology keeps its
    // own references to. Every call returns a `Result`, so an error anywhere
    // unwinds with nothing published to MF.
    unsafe {
        let topology =
            MFCreateTopology().map_err(|err| super::failed(path, "create a topology", err))?;

        for stream in planned {
            let stream_descriptor = stream_descriptor(path, descriptor, stream.index)?;

            // The stream's own first media type, offered at both ends of the
            // connection. P7-A's single-stream topology resolved without it;
            // a multi-track file did not (MF_E_TOPO_CODEC_NOT_FOUND, measured
            // against tone.mp4 and probe-both.mp4) — with two streams in the
            // source, resolution wants a preference per port pair rather than
            // an open question at each one.
            let handler = stream_descriptor
                .GetMediaTypeHandler()
                .map_err(|err| super::failed(path, "read a stream's media types", err))?;
            let media_type = handler
                .GetMediaTypeByIndex(0)
                .map_err(|err| super::failed(path, "read a stream's media type", err))?;

            let source_node = MFCreateTopologyNode(MF_TOPOLOGY_SOURCESTREAM_NODE)
                .map_err(|err| super::failed(path, "create a source node", err))?;
            source_node
                .SetUnknown(&MF_TOPONODE_SOURCE, source)
                .map_err(|err| super::failed(path, "attach the source", err))?;
            source_node
                .SetUnknown(&MF_TOPONODE_PRESENTATION_DESCRIPTOR, descriptor)
                .map_err(|err| super::failed(path, "attach the presentation descriptor", err))?;
            source_node
                .SetUnknown(&MF_TOPONODE_STREAM_DESCRIPTOR, &stream_descriptor)
                .map_err(|err| super::failed(path, "attach the stream descriptor", err))?;
            // No MF_TOPONODE_STREAMID here, and that is the point: the
            // official source-node set is exactly three attributes — SOURCE,
            // PRESENTATION_DESCRIPTOR, STREAM_DESCRIPTOR ("Creating Source
            // Nodes", Microsoft Learn) — and which stream this node stands
            // for is already carried by the *stream descriptor*. The PD index
            // that used to sit here was a fourth attribute nobody asked for.
            topology
                .AddNode(&source_node)
                .map_err(|err| super::failed(path, "add the source to the topology", err))?;

            source_node
                .SetOutputPrefType(0, &media_type)
                .map_err(|err| super::failed(path, "offer the stream's media type", err))?;

            let activate = if stream.video {
                let window = video_window.ok_or_else(|| {
                    TossError::other("a video stream reached a topology with no window for it")
                })?;
                MFCreateVideoRendererActivate(window)
                    .map_err(|err| super::failed(path, "reach the video renderer", err))?
            } else {
                MFCreateAudioRendererActivate()
                    .map_err(|err| super::failed(path, "reach the audio renderer", err))?
            };

            let output_node = MFCreateTopologyNode(MF_TOPOLOGY_OUTPUT_NODE)
                .map_err(|err| super::failed(path, "create an output node", err))?;
            output_node
                .SetObject(&activate)
                .map_err(|err| super::failed(path, "attach a renderer", err))?;
            // No MF_TOPONODE_STREAMID on the output node either — the bug
            // this milestone spent its experiments on. On an *output* node
            // that attribute names the *stream sink's* identifier, not the
            // source stream's, and omitting it means "stream sink 0"
            // ("Creating Output Nodes", Microsoft Learn). Every renderer
            // here — EVR and the audio renderer alike — is a fixed-sink
            // device with sink 0 only: handing it a PD index of 1 asked for
            // a sink that does not exist, which surfaced as
            // MF_E_STREAMSINKS_FIXED or MF_E_TOPO_CODEC_NOT_FOUND depending
            // on the container. The experiment matrix's two "independent
            // failure modes" were one mistake: PD index, source stream id
            // and sink id are three numbering domains sharing one attribute
            // name, and only the middle one belongs to a source node.
            // The renderer belongs to the system (the video one to the
            // window), not to this topology — MF's requirement for shared
            // sinks, as in P7-A.
            output_node
                .SetUINT32(&MF_TOPONODE_NOSHUTDOWN_ON_REMOVE, 1)
                .map_err(|err| super::failed(path, "mark the renderer as shared", err))?;
            topology
                .AddNode(&output_node)
                .map_err(|err| super::failed(path, "add the renderer to the topology", err))?;

            output_node
                .SetInputPrefType(0, &media_type)
                .map_err(|err| super::failed(path, "state the renderer's input type", err))?;

            // Port 0 on both ends: this source node *is* one stream, so its
            // only output is that stream, and the sink's only input is the
            // one it was built for.
            source_node
                .ConnectOutput(0, &output_node, 0)
                .map_err(|err| super::failed(path, "connect a stream to its renderer", err))?;
        }

        Ok(topology)
    }
}

/// Begin or resume playing from wherever the clock is.
///
/// An empty `PROPVARIANT` means "the current position" — for a paused
/// session that is the pause point, which is what makes this the other
/// half of D2's play/pause (§18.4).
pub(super) fn resume(session: &IMFMediaSession, path: &Path) -> Result<(), TossError> {
    // SAFETY: a null time format is GUID_NULL (presentation time, which
    // every source supports) and the position is an owned local.
    unsafe {
        let position = PROPVARIANT::default();
        session
            .Start(ptr::null(), &position)
            .map_err(|err| super::failed(path, "resume playback", err))
    }
}

/// Stop handing out samples until the next `resume`.
pub(super) fn pause(session: &IMFMediaSession, path: &Path) -> Result<(), TossError> {
    // SAFETY: the session is alive and owned by this wrapper.
    unsafe { session.Pause() }.map_err(|err| super::failed(path, "pause playback", err))
}

/// Where the media clock is right now, in 100-nanosecond units.
///
/// `GetCorrelatedTime` rather than a wall clock: seeking speaks the same
/// units, so a step is arithmetic on this number.
pub(super) fn current_time(session: &IMFMediaSession, path: &Path) -> Result<i64, TossError> {
    let mut now = 0_i64;
    let mut system = 0_i64;

    // SAFETY: the session is alive and owned by this wrapper; both
    // out-parameters point at local storage; the reserved flag is zero
    // as the contract requires.
    unsafe {
        let clock = session
            .GetClock()
            .map_err(|err| super::failed(path, "read the media clock", err))?;
        clock
            .GetCorrelatedTime(0, &mut now, &mut system)
            .map_err(|err| super::failed(path, "read the media clock", err))?;
    }

    Ok(now)
}

/// Jump to an absolute position in the presentation (100-ns units).
///
/// Negative targets are clamped to zero here: an arrow key held at the
/// start of a file must not ask for a position before it exists.
/// Running past the end is left to MF — its answer (stop there, or
/// refuse) is something to measure on a real machine, not to guess.
pub(super) fn seek_to(
    session: &IMFMediaSession,
    path: &Path,
    target: i64,
) -> Result<(), TossError> {
    // SAFETY: a null time format is GUID_NULL, and the position is an
    // owned local this call reads.
    unsafe {
        let position = position_at(target.max(0));
        session
            .Start(ptr::null(), &position)
            .map_err(|err| super::failed(path, "seek", err))
    }
}

/// The renderer's master volume, 0.0..=1.0.
pub(super) fn volume(session: &IMFMediaSession, path: &Path) -> Result<f32, TossError> {
    let volume = simple_audio_volume(path, session)?;

    // SAFETY: the interface was just created for this call and drops
    // after it.
    unsafe { volume.GetMasterVolume() }.map_err(|err| super::failed(path, "read the volume", err))
}

/// Set the renderer's master volume, clamped into 0.0..=1.0 — a held
/// arrow key must not be able to ask for a level the system rejects.
pub(super) fn set_volume(
    session: &IMFMediaSession,
    path: &Path,
    level: f32,
) -> Result<(), TossError> {
    let volume = simple_audio_volume(path, session)?;

    // SAFETY: as `volume` above; the level is clamped to the documented
    // range before it crosses the FFI boundary.
    unsafe { volume.SetMasterVolume(level.clamp(0.0, 1.0)) }
        .map_err(|err| super::failed(path, "set the volume", err))
}

/// The path as MF wants it: a NUL-terminated UTF-16 buffer.
///
/// Same rule as `decode.rs` widened paths (§22): a Windows path already *is*
/// UTF-16, so this widens rather than converts, and an interior NUL is
/// refused rather than truncated at the first one (§23).
fn wide_path(path: &Path) -> Result<Vec<u16>, TossError> {
    use std::os::windows::ffi::OsStrExt;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();

    if wide.contains(&0) {
        return Err(TossError::invalid_arguments(format!(
            "path cannot be opened: {}",
            path.display()
        )));
    }
    wide.push(0);

    Ok(wide)
}
