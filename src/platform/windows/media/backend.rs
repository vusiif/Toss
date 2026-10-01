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

use windows::Win32::Media::MediaFoundation::{
    IMFMediaSession, IMFMediaSource, IMFStreamDescriptor, IMFTopology,
    MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS, MEEndOfPresentation, MESessionClosed, MF_OBJECT_TYPE,
    MF_RESOLUTION_MEDIASOURCE, MF_TOPOLOGY_OUTPUT_NODE, MF_TOPOLOGY_SOURCESTREAM_NODE,
    MF_TOPONODE_NOSHUTDOWN_ON_REMOVE, MF_TOPONODE_PRESENTATION_DESCRIPTOR, MF_TOPONODE_SOURCE,
    MF_TOPONODE_STREAM_DESCRIPTOR, MF_TOPONODE_STREAMID, MF_VERSION, MFCreateAudioRendererActivate,
    MFCreateMediaSession, MFCreateSourceResolver, MFCreateTopology, MFCreateTopologyNode,
    MFSTARTUP_FULL, MFShutdown, MFStartup,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
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

    let session = Session::new(path)?;
    let topology = topology(path, &source)?;

    // SAFETY: the session and topology are local values that outlive these
    // calls, and both methods are MF's documented pairing — hand over the
    // topology, then start from a null position, which means "from the
    // beginning" (§18.4's "play"; the rest of the five controls are later
    // milestones).
    unsafe {
        session
            .inner
            .SetTopology(0, &topology)
            .map_err(|err| super::failed(path, "prepare playback", err))?;

        // An empty PROPVARIANT — VT_EMPTY — is "from the beginning", and a
        // *null* position is refused outright (E_POINTER, measured); the
        // time format may be null: MF's own default applies.
        let position = PROPVARIANT::default();
        session
            .inner
            .Start(ptr::null(), &position)
            .map_err(|err| super::failed(path, "start playback", err))?;
    }

    session.wait_for(path, MEEndOfPresentation.0 as u32)?;
    session.close(path)?;

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
    fn wait_for(&self, path: &Path, kind: u32) -> Result<(), TossError> {
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

            if status.is_err() {
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

        self.wait_for(path, MESessionClosed.0 as u32)
    }
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

/// The smallest topology that can carry a session to its end: source stream
/// node, connected to the system's audio renderer.
///
/// Audio only, and deliberately so (§9): P7-A is proving lifetime, not
/// pictures, and a video stream with no video sink would make topology
/// resolution — and therefore this milestone's verdict — about rendering
/// rather than about ownership. The video sink arrives with the video
/// window, as its own milestone.
fn topology(path: &Path, source: &IMFMediaSource) -> Result<IMFTopology, TossError> {
    // SAFETY: everything built here is local — the topology, both nodes and
    // the activate outlive these calls — and `source` is borrowed only for
    // the attachments, which the topology keeps references of its own to.
    // Every call returns a `Result`, so the only thing that can go wrong is
    // reported through `failed` rather than left half-assembled: an error
    // anywhere unwinds this function with nothing published to MF.
    unsafe {
        let topology =
            MFCreateTopology().map_err(|err| super::failed(path, "create a topology", err))?;

        // The source node needs the presentation descriptor as much as it
        // needs the source itself: without it the node has nothing that says
        // *which* streams exist, and MF refuses the topology outright with
        // MF_E_TOPO_MISSING_PRESENTATION_DESCRIPTOR — measured, this is the
        // first thing P7-A got wrong.
        let descriptor = source
            .CreatePresentationDescriptor()
            .map_err(|err| super::failed(path, "read the presentation descriptor", err))?;
        // The first stream: P7-A's topology carries exactly one audio
        // stream, and *which* stream deserves to be selected for a file with
        // several is a later milestone's decision, not this one's.
        descriptor
            .SelectStream(0)
            .map_err(|err| super::failed(path, "select the first stream", err))?;

        // ...and its stream descriptor: the third thing a source node
        // cannot do without. MF counts them in order — source, presentation
        // descriptor, stream descriptor — and answers each missing one with
        // its own MF_E_TOPO_* (measured: 0xC00D5217, then 0xC00D5218).
        // The selection flag is written but never read: the stream was
        // selected a line above, and this milestone acts on exactly one.
        let mut selected = windows::core::BOOL(0);
        let mut stream: Option<IMFStreamDescriptor> = None;
        descriptor
            .GetStreamDescriptorByIndex(0, &mut selected, &mut stream)
            .map_err(|err| super::failed(path, "read the first stream descriptor", err))?;
        let stream = stream
            .ok_or_else(|| TossError::other("the first stream of this file has no descriptor"))?;

        let source_node = MFCreateTopologyNode(MF_TOPOLOGY_SOURCESTREAM_NODE)
            .map_err(|err| super::failed(path, "create a source node", err))?;
        source_node
            .SetUnknown(&MF_TOPONODE_SOURCE, source)
            .map_err(|err| super::failed(path, "attach the source to the topology", err))?;
        source_node
            .SetUnknown(&MF_TOPONODE_STREAM_DESCRIPTOR, &stream)
            .map_err(|err| super::failed(path, "attach the stream descriptor", err))?;
        source_node
            .SetUnknown(&MF_TOPONODE_PRESENTATION_DESCRIPTOR, &descriptor)
            .map_err(|err| super::failed(path, "attach the presentation descriptor", err))?;
        source_node
            .SetUINT32(&MF_TOPONODE_STREAMID, 0)
            .map_err(|err| super::failed(path, "name the source stream", err))?;
        topology
            .AddNode(&source_node)
            .map_err(|err| super::failed(path, "add the source to the topology", err))?;

        let activate = MFCreateAudioRendererActivate()
            .map_err(|err| super::failed(path, "reach the audio renderer", err))?;
        let output_node = MFCreateTopologyNode(MF_TOPOLOGY_OUTPUT_NODE)
            .map_err(|err| super::failed(path, "create an output node", err))?;
        output_node
            .SetObject(&activate)
            .map_err(|err| super::failed(path, "attach the audio renderer", err))?;
        output_node
            .SetUINT32(&MF_TOPONODE_STREAMID, 0)
            .map_err(|err| super::failed(path, "name the output stream", err))?;
        // The renderer is the system's, not this topology's to shut down
        // when a node is removed — MF's own requirement for shared sinks.
        output_node
            .SetUINT32(&MF_TOPONODE_NOSHUTDOWN_ON_REMOVE, 1)
            .map_err(|err| super::failed(path, "mark the renderer as shared", err))?;
        topology
            .AddNode(&output_node)
            .map_err(|err| super::failed(path, "add the renderer to the topology", err))?;

        source_node
            .ConnectOutput(0, &output_node, 0)
            .map_err(|err| super::failed(path, "connect the source to the renderer", err))?;

        Ok(topology)
    }
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
