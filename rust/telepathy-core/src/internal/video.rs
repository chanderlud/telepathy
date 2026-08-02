pub mod platform;
pub mod transport;

use crate::internal::utils::JoinHandle;
use speedy::{Readable, Writable};
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

pub(crate) const VIDEO_PROTOCOL_REVISION: u8 = 1;
pub const VIDEO_CONTROL_MAX_FRAME_LENGTH: usize = 8 * 1024 * 1024;
pub const VIDEO_PREAMBLE_MAX_LENGTH: usize = 512;
pub const VIDEO_MEDIA_MAX_FRAME_LENGTH: usize = 64 * 1024;
pub(crate) const VIDEO_NEGOTIATION_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(15);
const MAX_VIDEO_DIMENSION: u32 = 16_384;

pub(crate) use crate::types::{
    VideoCodec, VideoLifecycleEvent, VideoMediaFormat, VideoPhase, VideoRole, VideoSessionId,
    VideoSessionIdentity, VideoSource, VideoTerminalReason,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LocalVideoGeneration(u64);

impl LocalVideoGeneration {
    pub(crate) const fn initial() -> Self {
        Self(0)
    }

    pub(crate) const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoWorkerStartup {
    Ready,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoAttempt {
    session_id: VideoSessionId,
    generation: LocalVideoGeneration,
}

impl VideoAttempt {
    pub(crate) const fn new(session_id: VideoSessionId, generation: LocalVideoGeneration) -> Self {
        Self {
            session_id,
            generation,
        }
    }

    pub(crate) fn accepts(self, control: VideoControl) -> bool {
        self.session_id == control.session_id()
    }

    pub const fn session_id(self) -> VideoSessionId {
        self.session_id
    }
}

struct VideoReservation {
    attempt: VideoAttempt,
    role: VideoRole,
    phase: VideoPhase,
    descriptor: VideoMediaDescriptor,
    cancellation: CancellationToken,
    worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for VideoReservation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VideoReservation")
            .field("attempt", &self.attempt)
            .field("role", &self.role)
            .field("phase", &self.phase)
            .field("descriptor", &self.descriptor)
            .field("cancellation", &self.cancellation)
            .field("worker_installed", &self.worker.is_some())
            .finish()
    }
}

#[derive(Debug)]
struct VideoSlotState {
    generation: LocalVideoGeneration,
    reservation: Option<VideoReservation>,
    pending_terminal: Option<(VideoAttempt, VideoTerminalReason)>,
}

impl Default for VideoSlotState {
    fn default() -> Self {
        Self {
            generation: LocalVideoGeneration::initial(),
            reservation: None,
            pending_terminal: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct VideoSlot {
    state: Mutex<VideoSlotState>,
    idle: Notify,
    terminal: Notify,
}

#[derive(Debug, Clone)]
pub struct VideoLaunch {
    attempt: VideoAttempt,
    role: VideoRole,
    descriptor: VideoMediaDescriptor,
    cancellation: CancellationToken,
}

impl VideoLaunch {
    pub const fn attempt(&self) -> VideoAttempt {
        self.attempt
    }

    pub const fn role(&self) -> VideoRole {
        self.role
    }

    pub const fn descriptor(&self) -> VideoMediaDescriptor {
        self.descriptor
    }

    pub const fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }
}

#[derive(Debug)]
pub struct VideoDisplacement {
    reservation: VideoReservation,
}

impl VideoDisplacement {
    fn new(reservation: VideoReservation) -> Self {
        reservation.cancellation.cancel();
        Self { reservation }
    }

    pub async fn cancel_and_join(
        mut self,
        peer_id: String,
        reason: VideoTerminalReason,
    ) -> VideoLifecycleEvent {
        if let Some(worker) = self.reservation.worker.take() {
            let _ = worker.await;
        }
        VideoLifecycleEvent {
            identity: VideoSessionIdentity {
                peer_id,
                session_id: self.reservation.attempt.session_id(),
            },
            role: self.reservation.role,
            source: self.reservation.descriptor.source(),
            phase: VideoPhase::Terminal,
            terminal_reason: Some(reason),
        }
    }
}

#[derive(Debug)]
pub enum VideoSlotEffect {
    Ignored,
    Send(VideoControl),
    Launch(VideoLaunch),
    SendAndLaunch(VideoControl, VideoLaunch),
    DisplaceAndSendAndLaunch(VideoDisplacement, VideoControl, VideoLaunch),
    Terminal(VideoAttempt, VideoTerminalReason),
}

impl VideoSlotEffect {
    #[cfg(test)]
    fn launch(self) -> Option<VideoLaunch> {
        match self {
            Self::Launch(launch)
            | Self::SendAndLaunch(_, launch)
            | Self::DisplaceAndSendAndLaunch(_, _, launch) => Some(launch),
            Self::Ignored | Self::Send(_) | Self::Terminal(_, _) => None,
        }
    }
}

impl VideoSlot {
    pub async fn start_local(&self, descriptor: VideoMediaDescriptor) -> Option<VideoControl> {
        let mut state = self.state.lock().await;
        if state.reservation.is_some() {
            return None;
        }
        state.generation = state.generation.next();
        let session_id = VideoSessionId::new();
        state.reservation = Some(VideoReservation {
            attempt: VideoAttempt::new(session_id, state.generation),
            role: VideoRole::Sender,
            phase: VideoPhase::WaitingReady,
            descriptor,
            cancellation: CancellationToken::new(),
            worker: None,
        });
        Some(VideoControl::offer(session_id, descriptor))
    }

    pub async fn receive(&self, control: VideoControl, local_offer_wins: bool) -> VideoSlotEffect {
        let mut state = self.state.lock().await;
        match control {
            VideoControl::Offer(offer) => match state.reservation.as_ref() {
                None => {
                    let (control, launch) = Self::accept_remote_offer(&mut state, offer);
                    VideoSlotEffect::SendAndLaunch(control, launch)
                }
                Some(current)
                    if current.attempt.accepts(control) && current.role == VideoRole::Receiver =>
                {
                    VideoSlotEffect::Send(VideoControl::ready(control.session_id()))
                }
                Some(current)
                    if current.role == VideoRole::Sender
                        && current.phase == VideoPhase::WaitingReady
                        && !local_offer_wins =>
                {
                    let displaced = VideoDisplacement::new(
                        state
                            .reservation
                            .take()
                            .expect("matching reservation checked"),
                    );
                    let (control, launch) = Self::accept_remote_offer(&mut state, offer);
                    VideoSlotEffect::DisplaceAndSendAndLaunch(displaced, control, launch)
                }
                Some(_) => VideoSlotEffect::Send(VideoControl::reject(
                    control.session_id(),
                    VideoRejectReason::SessionUnavailable,
                )),
            },
            VideoControl::Ready { .. } => match state.reservation.as_mut() {
                Some(current)
                    if current.role == VideoRole::Sender
                        && current.phase == VideoPhase::WaitingReady
                        && current.attempt.accepts(control) =>
                {
                    current.phase = VideoPhase::Starting;
                    VideoSlotEffect::Launch(Self::launch(current))
                }
                Some(current)
                    if current.role == VideoRole::Sender
                        && current.phase == VideoPhase::Starting
                        && current.attempt.accepts(control) =>
                {
                    VideoSlotEffect::Ignored
                }
                Some(_) | None => VideoSlotEffect::Ignored,
            },
            VideoControl::Reject { .. } => {
                Self::terminal_if_current(&mut state, control, VideoTerminalReason::Rejected)
            }
            VideoControl::Stop { reason, .. } => {
                Self::terminal_if_current(&mut state, control, reason)
            }
        }
    }

    pub async fn receive_offer(
        &self,
        offer: VideoOffer,
        local_offer_wins: bool,
        receive_formats: &[VideoMediaFormat],
    ) -> VideoSlotEffect {
        if !receive_formats.contains(&offer.descriptor.format) {
            return VideoSlotEffect::Send(VideoControl::reject(
                offer.session_id,
                VideoRejectReason::UnsupportedCodec,
            ));
        }
        self.receive(VideoControl::Offer(offer), local_offer_wins)
            .await
    }

    pub async fn install(&self, launch: &VideoLaunch, worker: JoinHandle<()>) -> bool {
        let mut state = self.state.lock().await;
        let matches = state.reservation.as_ref().is_some_and(|reservation| {
            reservation.attempt == launch.attempt
                && reservation.phase == VideoPhase::Starting
                && reservation.worker.is_none()
        });
        if matches {
            let reservation = state
                .reservation
                .as_mut()
                .expect("matching reservation checked above");
            reservation.worker = Some(worker);
            return true;
        }
        drop(state);
        launch.cancellation.cancel();
        let _ = worker.await;
        false
    }

    pub async fn complete_startup(
        &self,
        launch: &VideoLaunch,
        startup: VideoWorkerStartup,
        peer_id: String,
    ) -> Option<VideoLifecycleEvent> {
        if startup != VideoWorkerStartup::Ready {
            return None;
        }
        let mut state = self.state.lock().await;
        let reservation = state.reservation.as_mut()?;
        if reservation.attempt != launch.attempt
            || reservation.phase != VideoPhase::Starting
            || reservation.worker.is_none()
        {
            return None;
        }
        reservation.phase = VideoPhase::Active;
        Some(VideoLifecycleEvent {
            identity: VideoSessionIdentity {
                peer_id,
                session_id: reservation.attempt.session_id(),
            },
            role: reservation.role,
            source: reservation.descriptor.source(),
            phase: VideoPhase::Active,
            terminal_reason: None,
        })
    }

    pub(crate) async fn cancel_current_and_join(
        &self,
        reason: VideoTerminalReason,
    ) -> Option<VideoTerminalReason> {
        let attempt = self.state.lock().await.reservation.as_ref()?.attempt;
        self.cancel_and_join(attempt, reason).await
    }

    pub async fn cancel_and_join(
        &self,
        attempt: VideoAttempt,
        reason: VideoTerminalReason,
    ) -> Option<VideoTerminalReason> {
        loop {
            let idle = self.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            let worker = {
                let mut state = self.state.lock().await;
                let current = state.reservation.as_mut()?;
                if current.attempt != attempt {
                    return None;
                }
                if current.phase == VideoPhase::Stopping {
                    None
                } else {
                    current.phase = VideoPhase::Stopping;
                    current.cancellation.cancel();
                    Some(current.worker.take())
                }
            };
            let Some(worker) = worker else {
                idle.await;
                continue;
            };
            if let Some(worker) = worker {
                let _ = worker.await;
            }
            let mut state = self.state.lock().await;
            if state
                .reservation
                .as_ref()
                .is_some_and(|current| current.attempt == attempt)
            {
                state.reservation = None;
                state.pending_terminal = None;
                drop(state);
                self.idle.notify_waiters();
                return Some(reason);
            }
            return None;
        }
    }

    pub async fn current_event(
        &self,
        peer_id: String,
        phase: VideoPhase,
        terminal_reason: Option<VideoTerminalReason>,
    ) -> Option<VideoLifecycleEvent> {
        let state = self.state.lock().await;
        let current = state.reservation.as_ref()?;
        Some(VideoLifecycleEvent {
            identity: VideoSessionIdentity {
                peer_id,
                session_id: current.attempt.session_id(),
            },
            role: current.role,
            source: current.descriptor.source(),
            phase,
            terminal_reason,
        })
    }

    pub(crate) async fn report_terminal(&self, attempt: VideoAttempt, reason: VideoTerminalReason) {
        let mut state = self.state.lock().await;
        if state.pending_terminal.is_none()
            && state.reservation.as_ref().is_some_and(|current| {
                current.attempt == attempt && current.phase != VideoPhase::Stopping
            })
        {
            state.pending_terminal = Some((attempt, reason));
            drop(state);
            self.terminal.notify_one();
        }
    }

    pub(crate) async fn terminal_notified(&self) {
        self.terminal.notified().await;
    }

    pub(crate) async fn take_terminal(&self) -> Option<(VideoAttempt, VideoTerminalReason)> {
        self.state.lock().await.pending_terminal.take()
    }

    pub(crate) async fn expire_waiting_ready(&self) -> Option<(VideoAttempt, VideoTerminalReason)> {
        let state = self.state.lock().await;
        let current = state.reservation.as_ref()?;
        if current.role != VideoRole::Sender || current.phase != VideoPhase::WaitingReady {
            return None;
        }
        Some((current.attempt, VideoTerminalReason::Failed))
    }

    fn accept_remote_offer(
        state: &mut VideoSlotState,
        offer: VideoOffer,
    ) -> (VideoControl, VideoLaunch) {
        state.generation = state.generation.next();
        let session_id = VideoControl::Offer(offer).session_id();
        state.reservation = Some(VideoReservation {
            attempt: VideoAttempt::new(session_id, state.generation),
            role: VideoRole::Receiver,
            phase: VideoPhase::Starting,
            descriptor: offer.descriptor,
            cancellation: CancellationToken::new(),
            worker: None,
        });
        let launch = Self::launch(state.reservation.as_ref().expect("reservation inserted"));
        (VideoControl::ready(session_id), launch)
    }

    fn terminal_if_current(
        state: &mut VideoSlotState,
        control: VideoControl,
        reason: VideoTerminalReason,
    ) -> VideoSlotEffect {
        let Some(current) = state.reservation.as_ref() else {
            return VideoSlotEffect::Ignored;
        };
        if !current.attempt.accepts(control) {
            return VideoSlotEffect::Ignored;
        }
        VideoSlotEffect::Terminal(current.attempt, reason)
    }

    fn launch(reservation: &VideoReservation) -> VideoLaunch {
        VideoLaunch {
            attempt: reservation.attempt,
            role: reservation.role,
            descriptor: reservation.descriptor,
            cancellation: reservation.cancellation.clone(),
        }
    }
}

#[derive(Readable, Writable, Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoRejectReason {
    UnsupportedSource,
    UnsupportedCodec,
    InvalidDescriptor,
    SessionUnavailable,
}

#[derive(Readable, Writable, Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoMediaDescriptor {
    source: VideoSource,
    format: VideoMediaFormat,
    width: u32,
    height: u32,
}

impl VideoMediaDescriptor {
    #[cfg_attr(
        not(any(target_os = "windows", target_os = "macos", target_os = "linux")),
        expect(
            dead_code,
            reason = "only used by the desktop ffmpeg backend and tests"
        )
    )]
    pub const fn display(codec: VideoCodec, width: u32, height: u32) -> Self {
        Self {
            source: VideoSource::Display,
            format: VideoMediaFormat::MpegTs(codec),
            width,
            height,
        }
    }

    pub(crate) const fn source(self) -> VideoSource {
        self.source
    }

    #[cfg_attr(
        not(any(target_os = "windows", target_os = "macos", target_os = "linux")),
        expect(
            dead_code,
            reason = "only used by the desktop ffmpeg backend and tests"
        )
    )]
    pub(crate) const fn codec(self) -> VideoCodec {
        match self.format {
            VideoMediaFormat::MpegTs(codec) => codec,
        }
    }

    #[cfg_attr(
        not(any(target_os = "windows", target_os = "macos", target_os = "linux")),
        expect(
            dead_code,
            reason = "only used by the desktop ffmpeg backend and tests"
        )
    )]
    pub(crate) const fn dimensions(self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub(crate) const fn is_valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && self.width <= MAX_VIDEO_DIMENSION
            && self.height <= MAX_VIDEO_DIMENSION
    }
}

#[derive(Readable, Writable, Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoOffer {
    revision: u8,
    session_id: VideoSessionId,
    descriptor: VideoMediaDescriptor,
}

#[derive(Readable, Writable, Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoControl {
    Offer(VideoOffer),
    Ready {
        revision: u8,
        session_id: VideoSessionId,
    },
    Reject {
        revision: u8,
        session_id: VideoSessionId,
        reason: VideoRejectReason,
    },
    Stop {
        revision: u8,
        session_id: VideoSessionId,
        reason: VideoTerminalReason,
    },
}

impl VideoControl {
    pub const fn offer(session_id: VideoSessionId, descriptor: VideoMediaDescriptor) -> Self {
        Self::Offer(VideoOffer {
            revision: VIDEO_PROTOCOL_REVISION,
            session_id,
            descriptor,
        })
    }

    pub const fn ready(session_id: VideoSessionId) -> Self {
        Self::Ready {
            revision: VIDEO_PROTOCOL_REVISION,
            session_id,
        }
    }

    pub(crate) const fn reject(session_id: VideoSessionId, reason: VideoRejectReason) -> Self {
        Self::Reject {
            revision: VIDEO_PROTOCOL_REVISION,
            session_id,
            reason,
        }
    }

    pub const fn stop(session_id: VideoSessionId, reason: VideoTerminalReason) -> Self {
        Self::Stop {
            revision: VIDEO_PROTOCOL_REVISION,
            session_id,
            reason,
        }
    }

    pub const fn session_id(self) -> VideoSessionId {
        match self {
            Self::Offer(offer) => offer.session_id,
            Self::Ready { session_id, .. }
            | Self::Reject { session_id, .. }
            | Self::Stop { session_id, .. } => session_id,
        }
    }

    pub(crate) const fn validate(self) -> Result<(), VideoProtocolError> {
        let revision = match self {
            Self::Offer(offer) => {
                if !offer.descriptor.is_valid() {
                    return Err(VideoProtocolError::InvalidDimensions);
                }
                offer.revision
            }
            Self::Ready { revision, .. }
            | Self::Reject { revision, .. }
            | Self::Stop { revision, .. } => revision,
        };
        if revision == VIDEO_PROTOCOL_REVISION {
            Ok(())
        } else {
            Err(VideoProtocolError::UnsupportedRevision(revision))
        }
    }
}

#[derive(Readable, Writable, Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoPreamble {
    revision: u8,
    session_id: VideoSessionId,
    descriptor: VideoMediaDescriptor,
}

impl VideoPreamble {
    pub const fn new(session_id: VideoSessionId, descriptor: VideoMediaDescriptor) -> Self {
        Self::with_revision(VIDEO_PROTOCOL_REVISION, session_id, descriptor)
    }

    pub(crate) const fn with_revision(
        revision: u8,
        session_id: VideoSessionId,
        descriptor: VideoMediaDescriptor,
    ) -> Self {
        Self {
            revision,
            session_id,
            descriptor,
        }
    }

    const fn validate(self) -> Result<(), VideoProtocolError> {
        if self.revision != VIDEO_PROTOCOL_REVISION {
            return Err(VideoProtocolError::UnsupportedRevision(self.revision));
        }
        if !self.descriptor.is_valid() {
            return Err(VideoProtocolError::InvalidDimensions);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoProtocolError {
    FrameTooLarge,
    Malformed,
    UnsupportedRevision(u8),
    InvalidDimensions,
}

pub(crate) fn encode_preamble(preamble: &VideoPreamble) -> Result<Vec<u8>, VideoProtocolError> {
    preamble.validate()?;
    let encoded = preamble
        .write_to_vec()
        .map_err(|_| VideoProtocolError::Malformed)?;
    if encoded.len() > VIDEO_PREAMBLE_MAX_LENGTH {
        return Err(VideoProtocolError::FrameTooLarge);
    }
    Ok(encoded)
}

pub(crate) fn decode_preamble(bytes: &[u8]) -> Result<VideoPreamble, VideoProtocolError> {
    if bytes.len() > VIDEO_PREAMBLE_MAX_LENGTH {
        return Err(VideoProtocolError::FrameTooLarge);
    }
    let preamble =
        VideoPreamble::read_from_buffer(bytes).map_err(|_| VideoProtocolError::Malformed)?;
    preamble.validate()?;
    Ok(preamble)
}

#[cfg(test)]
mod tests {
    use super::{
        LocalVideoGeneration, VideoAttempt, VideoCodec, VideoControl, VideoMediaDescriptor,
        VideoPreamble, VideoProtocolError, VideoSessionId, decode_preamble, encode_preamble,
    };
    use crate::internal::state::SessionState;
    use speedy::Writable;

    #[test]
    fn preamble_round_trip_preserves_identity_and_descriptor() {
        let session_id = VideoSessionId::new();
        let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1920, 1080);
        let preamble = VideoPreamble::new(session_id, descriptor);

        let encoded = encode_preamble(&preamble).expect("valid preamble encodes");
        let decoded = decode_preamble(&encoded).expect("encoded preamble decodes");

        assert_eq!(decoded, preamble);
    }

    #[test]
    fn preamble_rejects_unknown_revision_and_malformed_dimensions() {
        let session_id = VideoSessionId::new();
        let unsupported_revision = VideoPreamble::with_revision(
            99,
            session_id,
            VideoMediaDescriptor::display(VideoCodec::H264, 1920, 1080),
        );
        let malformed_dimensions = VideoPreamble::new(
            session_id,
            VideoMediaDescriptor::display(VideoCodec::H264, 0, 1080),
        );

        assert_eq!(
            decode_preamble(&unsupported_revision.write_to_vec().expect("encodes")),
            Err(VideoProtocolError::UnsupportedRevision(99))
        );
        assert_eq!(
            decode_preamble(&malformed_dimensions.write_to_vec().expect("encodes")),
            Err(VideoProtocolError::InvalidDimensions)
        );
    }

    #[test]
    fn preamble_rejects_unknown_codec_and_oversize_before_decode() {
        let preamble = VideoPreamble::new(
            VideoSessionId::new(),
            VideoMediaDescriptor::display(VideoCodec::H264, 1920, 1080),
        );
        let mut unknown_codec = encode_preamble(&preamble).expect("encodes");
        unknown_codec[25..29].copy_from_slice(&99_u32.to_le_bytes());

        assert_eq!(
            decode_preamble(&unknown_codec),
            Err(VideoProtocolError::Malformed)
        );
        assert_eq!(
            decode_preamble(&vec![0; super::VIDEO_PREAMBLE_MAX_LENGTH + 1]),
            Err(VideoProtocolError::FrameTooLarge)
        );
    }

    #[test]
    fn controls_preserve_identity_and_distinguish_sessions() {
        let session_id = VideoSessionId::new();
        let offer = VideoControl::offer(
            session_id,
            VideoMediaDescriptor::display(VideoCodec::H264, 1920, 1080),
        );

        assert_eq!(offer.session_id(), session_id);
        assert_ne!(
            offer,
            VideoControl::offer(
                VideoSessionId::new(),
                VideoMediaDescriptor::display(VideoCodec::H264, 1920, 1080)
            )
        );
    }

    #[test]
    fn local_generation_advances() {
        let generation = LocalVideoGeneration::initial();

        assert_ne!(generation, generation.next());
    }

    #[test]
    fn attempt_accepts_duplicate_control_but_not_replaced_identity() {
        let session_id = VideoSessionId::new();
        let generation = LocalVideoGeneration::initial();
        let attempt = VideoAttempt::new(session_id, generation);
        let duplicate = VideoControl::ready(session_id);
        let replacement = VideoControl::ready(VideoSessionId::new());

        assert!(attempt.accepts(duplicate));
        assert!(!attempt.accepts(replacement));
    }

    #[tokio::test]
    async fn slot_remains_reserved_until_matching_worker_joins() {
        let slot = std::sync::Arc::new(super::VideoSlot::default());
        let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1920, 1080);
        let offer = slot
            .start_local(descriptor)
            .await
            .expect("first offer reserves slot");
        let launch = slot
            .receive(VideoControl::ready(offer.session_id()), true)
            .await
            .launch()
            .expect("ready starts matching sender");
        let worker_exited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_exited_clone = std::sync::Arc::clone(&worker_exited);
        let cancellation = launch.cancellation().clone();
        let worker = tokio::spawn(async move {
            cancellation.cancelled().await;
            tokio::task::yield_now().await;
            worker_exited_clone.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        slot.install(&launch, worker).await;

        let cleanup = tokio::spawn({
            let slot = std::sync::Arc::clone(&slot);
            async move {
                slot.cancel_and_join(launch.attempt(), super::VideoTerminalReason::Stopped)
                    .await
            }
        });

        assert_eq!(
            cleanup.await.expect("cleanup joins"),
            Some(super::VideoTerminalReason::Stopped)
        );
        assert!(worker_exited.load(std::sync::atomic::Ordering::Relaxed));
        assert!(
            slot.current_event("peer".to_string(), super::VideoPhase::Terminal, None)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn stale_worker_installation_cancels_and_joins_without_clearing_replacement() {
        let slot = super::VideoSlot::default();
        let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1920, 1080);
        let first = slot.start_local(descriptor).await.expect("first offer");
        let first_launch = slot
            .receive(VideoControl::ready(first.session_id()), true)
            .await
            .launch()
            .expect("first launch");
        slot.cancel_and_join(first_launch.attempt(), super::VideoTerminalReason::Stopped)
            .await;
        let second = slot
            .start_local(descriptor)
            .await
            .expect("replacement offer");
        let stale_joined = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stale_joined_clone = std::sync::Arc::clone(&stale_joined);
        let cancellation = first_launch.cancellation().clone();
        let stale_worker = tokio::spawn(async move {
            cancellation.cancelled().await;
            stale_joined_clone.store(true, std::sync::atomic::Ordering::Relaxed);
        });

        slot.install(&first_launch, stale_worker).await;

        assert!(stale_joined.load(std::sync::atomic::Ordering::Relaxed));
        assert!(
            slot.current_event("peer".to_string(), super::VideoPhase::WaitingReady, None)
                .await
                .is_some()
        );
        assert_ne!(first.session_id(), second.session_id());
    }

    #[tokio::test]
    async fn concurrent_terminal_claims_wait_for_one_join_and_emit_once() {
        let slot = std::sync::Arc::new(super::VideoSlot::default());
        let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1920, 1080);
        let offer = slot
            .start_local(descriptor)
            .await
            .expect("offer reserves slot");
        let launch = slot
            .receive(VideoControl::ready(offer.session_id()), true)
            .await
            .launch()
            .expect("ready launches sender");
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let (worker_cancelled, cancelled) = tokio::sync::oneshot::channel();
        let worker_release = std::sync::Arc::clone(&release);
        let cancellation = launch.cancellation().clone();
        let worker = tokio::spawn(async move {
            cancellation.cancelled().await;
            let _ = worker_cancelled.send(());
            worker_release.notified().await;
        });
        slot.install(&launch, worker).await;
        let attempt = launch.attempt();
        let first = tokio::spawn({
            let slot = std::sync::Arc::clone(&slot);
            async move {
                slot.cancel_and_join(attempt, super::VideoTerminalReason::Stopped)
                    .await
            }
        });
        let second = tokio::spawn({
            let slot = std::sync::Arc::clone(&slot);
            async move {
                slot.cancel_and_join(attempt, super::VideoTerminalReason::Teardown)
                    .await
            }
        });
        cancelled.await.expect("worker observes cancellation");
        tokio::task::yield_now().await;
        assert!(!first.is_finished());
        assert!(!second.is_finished());

        release.notify_one();

        let outcomes = [
            first.await.expect("first cleanup joins"),
            second.await.expect("second cleanup joins"),
        ];
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_some()).count(),
            1
        );
        assert!(
            slot.current_event("peer".to_string(), super::VideoPhase::Terminal, None)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn session_teardown_waits_for_installed_video_worker() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let state = std::sync::Arc::new(SessionState::new(&sender));
        let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1920, 1080);
        let offer = state
            .video_slot
            .start_local(descriptor)
            .await
            .expect("offer reserves slot");
        let launch = state
            .video_slot
            .receive(VideoControl::ready(offer.session_id()), true)
            .await
            .launch()
            .expect("ready launches sender");
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let (worker_cancelled, cancelled) = tokio::sync::oneshot::channel();
        let worker_release = std::sync::Arc::clone(&release);
        let cancellation = launch.cancellation().clone();
        let worker = tokio::spawn(async move {
            cancellation.cancelled().await;
            let _ = worker_cancelled.send(());
            worker_release.notified().await;
        });
        state.video_slot.install(&launch, worker).await;
        let teardown = tokio::spawn({
            let state = std::sync::Arc::clone(&state);
            async move { state.teardown().await }
        });
        cancelled.await.expect("worker observes cancellation");
        assert!(!teardown.is_finished());

        release.notify_one();

        teardown.await.expect("session teardown joins");
        assert!(
            state
                .video_slot
                .current_event("peer".to_string(), super::VideoPhase::Terminal, None)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn slot_resolves_crossed_offers_and_expires_only_waiting_sender() {
        let slot = super::VideoSlot::default();
        let descriptor = VideoMediaDescriptor::display(VideoCodec::H264, 1920, 1080);
        let local_offer = slot
            .start_local(descriptor)
            .await
            .expect("local offer reserves slot");
        let remote_id = VideoSessionId::new();

        assert_eq!(slot.start_local(descriptor).await, None);
        let remote_launch = slot
            .receive(VideoControl::offer(remote_id, descriptor), false)
            .await
            .launch()
            .expect("winning remote offer starts receiver");
        assert_eq!(remote_launch.role(), super::VideoRole::Receiver);
        assert_eq!(slot.expire_waiting_ready().await, None);
        assert!(matches!(
            slot.receive(
                VideoControl::stop(
                    local_offer.session_id(),
                    super::VideoTerminalReason::Stopped
                ),
                false
            )
            .await,
            super::VideoSlotEffect::Ignored
        ));
        let terminal = slot
            .receive(
                VideoControl::stop(remote_id, super::VideoTerminalReason::Stopped),
                false,
            )
            .await;
        assert!(matches!(
            terminal,
            super::VideoSlotEffect::Terminal(_, super::VideoTerminalReason::Stopped)
        ));
        slot.cancel_and_join(remote_launch.attempt(), super::VideoTerminalReason::Stopped)
            .await;

        let offer = slot
            .start_local(descriptor)
            .await
            .expect("second local offer reserves slot");
        let expired = slot
            .expire_waiting_ready()
            .await
            .expect("waiting offer expires");
        assert_eq!(expired.1, super::VideoTerminalReason::Failed);
        slot.cancel_and_join(expired.0, expired.1).await;
        assert!(matches!(
            slot.receive(VideoControl::ready(offer.session_id()), true)
                .await,
            super::VideoSlotEffect::Ignored
        ));
    }
}
