/// callback traits shared by FRB and native frontends
pub mod callbacks;
/// networking code for live audio streams
mod connections;
/// implementations for core telepathy functionality
pub mod core;
pub mod error;
/// helper methods used by telepathy core
mod helpers;
pub(crate) mod messages;
pub(crate) mod screenshare;
pub mod state;
mod utils;

use crate::AudioDevice;
use crate::direct_invitation;
use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use crate::internal::core::{RoomControllerStart, TelepathyCore};
use crate::internal::error::{Error, ErrorKind};
use crate::internal::messages::{Attachment, ProtocolMessage};
use crate::internal::state::{
    CallSlotAcquireResult, CallSlotState, EarlyCallState, PreparedIdentitySwitch,
    PreparedSwitchLease, RoomState, SessionState,
};
pub(crate) use crate::internal::utils::{JoinHandle, spawn_task};
use crate::overlay::Overlay;
use crate::types::{ChatMessage, CodecConfig, Contact, NetworkConfig, ScreenshareConfig};
use chrono::Local;
use iroh::SecretKey;
use speedy::{LittleEndian, Writable, Writer};
use std::collections::HashSet;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use telepathy_audio::RnnModel;
use telepathy_audio::devices::AudioHost;
use tokio::sync::mpsc::channel;
use tokio::sync::oneshot;
use tokio::sync::{Mutex, Notify};
#[cfg(not(target_family = "wasm"))]
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use tracing::{debug, error, info, info_span, warn};
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::timeout;

type Result<T> = std::result::Result<T, Error>;

/// Upper bound on how long `try_start_session` will wait for the manager to
/// apply the runtime before returning `RuntimeNotReady`. Bounds the damage of
/// a stalled or panicked manager task that never calls `mark_runtime_applied`.
const START_SESSION_RUNTIME_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on how long `TelepathyHandle::end_call` will wait for the call
/// slot to be released before returning. Long enough to absorb controller
/// teardown latency under load, short enough that a controller stuck on a
/// parked frontend callback cannot hang the public API.
const END_CALL_SLOT_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);

/// A timeout used when initializing the call
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// How often to keep-alive iroh session streams
const KEEP_ALIVE: Duration = Duration::from_secs(10);
/// the protocol identifier for Telepathy sessions
const ALPN: &[u8] = b"telepathy/session/1";
/// Maximum allowed size for a single length-delimited control/message frame on the session stream.
/// Attachments larger than this require a separately bounded or streamed transport.
pub(crate) const SESSION_MAX_FRAME_LENGTH: usize = 8 * 1024 * 1024;
/// Maximum encoded custom ringtone size accepted from disk or a remote peer.
pub(crate) const MAX_RINGTONE_LENGTH: usize = 4 * 1024 * 1024;

/// Counts a `speedy` encoding without allocating its payload and rejects writes
/// that would exceed the session frame limit.
struct BoundedSizeWriter {
    size: usize,
}

impl BoundedSizeWriter {
    fn new() -> Self {
        Self { size: 0 }
    }
}

impl Writer<LittleEndian> for BoundedSizeWriter {
    fn write_bytes(&mut self, bytes: &[u8]) -> std::result::Result<(), speedy::Error> {
        self.size = self
            .size
            .checked_add(bytes.len())
            .filter(|&size| size <= SESSION_MAX_FRAME_LENGTH)
            .ok_or_else(|| speedy::Error::custom("chat frame exceeds maximum size"))?;
        Ok(())
    }

    fn context(&self) -> &LittleEndian {
        static CONTEXT: LittleEndian = LittleEndian {};
        &CONTEXT
    }

    fn context_mut(&mut self) -> &mut LittleEndian {
        panic!("the bounded size writer does not mutate its serialization context")
    }
}

/// A borrowed representation of the `ProtocolMessage::Chat` wire format.
struct ChatFrame<'a> {
    text: &'a str,
    attachments: &'a [Attachment],
}

impl Writable<LittleEndian> for ChatFrame<'_> {
    fn write_to<T: ?Sized + Writer<LittleEndian>>(
        &self,
        writer: &mut T,
    ) -> std::result::Result<(), speedy::Error> {
        // `Chat` is the sixth `ProtocolMessage` variant and uses Speedy's
        // default u32 enum tag and collection-length encodings.
        writer.write_u32(5)?;
        write_sized_bytes(writer, self.text.as_bytes())?;
        let attachment_count = u32::try_from(self.attachments.len())
            .map_err(|_| speedy::Error::custom("too many chat attachments"))?;
        writer.write_u32(attachment_count)?;
        for attachment in self.attachments {
            write_sized_bytes(writer, attachment.name.as_bytes())?;
            write_sized_bytes(writer, &attachment.data)?;
        }
        Ok(())
    }
}

fn write_sized_bytes<T: ?Sized + Writer<LittleEndian>>(
    writer: &mut T,
    bytes: &[u8],
) -> std::result::Result<(), speedy::Error> {
    let length = u32::try_from(bytes.len())
        .map_err(|_| speedy::Error::custom("chat field exceeds wire length"))?;
    writer.write_u32(length)?;
    writer.write_bytes(bytes)
}

fn chat_message_fits_frame(text: &str, attachments: &[Attachment]) -> bool {
    ChatFrame { text, attachments }
        .write_to(&mut BoundedSizeWriter::new())
        .is_ok()
}

pub struct TelepathyHandle<C, S, H, I, O>
where
    C: CoreCallbacks<S> + Send + Sync + 'static,
    S: CoreStatisticsCallback + Send + Sync + 'static,
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    pub inner: TelepathyCore<C, S, H, I, O>,

    /// contains handles to the manager thread & room managers
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

/// Tears down one room generation's controller and awaits its completion.
///
/// `cancel` is that generation's `end_sessions` token, so every post-spawn
/// cancellation branch in `join_room_with_operation` can route through this single
/// helper to signal the exact generation and wait for its slot/room_state release.
async fn abort_room_generation(
    cancel: &CancellationToken,
    end_call: &Notify,
    completion: &mut oneshot::Receiver<()>,
) {
    cancel.cancel();
    end_call.notify_one();
    let _ = completion.await;
}

impl<C, S, H, I, O> TelepathyHandle<C, S, H, I, O>
where
    C: CoreCallbacks<S> + Send + Sync + 'static,
    S: CoreStatisticsCallback + Send + Sync + 'static,
    H: AudioHost<InputStream = I, OutputStream = O> + Send + Sync + Clone + 'static,
    I: Send + Sync + 'static,
    O: Send + Sync + 'static,
{
    /// Builds a new handle around a fresh `TelepathyCore`.
    pub fn new(
        host: H,
        network_config: &NetworkConfig,
        screenshare_config: &ScreenshareConfig,
        overlay: &Overlay,
        codec_config: &CodecConfig,
        callbacks: C,
    ) -> Self {
        Self {
            inner: TelepathyCore::new(
                host,
                network_config,
                screenshare_config,
                overlay,
                codec_config,
                callbacks,
            ),
            handles: Default::default(),
        }
    }

    pub async fn start_manager(&mut self) {
        if self
            .inner
            .core_state
            .desired_runtime()
            .is_ok_and(|runtime| runtime.identity.is_none())
            && let Some(identity) = self.inner.core_state.identity.read().await.clone()
        {
            self.inner
                .core_state
                .replace_desired_identity_infallible(identity);
        }
        if let Some(handle) = self.inner.start_manager().await {
            self.handles.lock().await.push(handle);
        }
    }

    /// Spawns the manager (if needed) and blocks until the runtime revision is
    /// applied. Production bindings call the non-blocking `start_manager` and
    /// observe `ManagerState::Active` via callbacks; this is retained for
    /// callers that need the synchronous readiness guarantee.
    pub async fn start_manager_and_wait(&mut self) -> Result<()> {
        self.start_manager().await;
        let revision = self.inner.core_state.desired_runtime()?.revision;
        self.inner
            .core_state
            .wait_for_runtime_applied(revision)
            .await
    }

    /// Tries to start a session for a contact
    pub async fn start_session(&self, contact: &Contact) {
        if let Err(error) = self.try_start_session(contact).await {
            warn!(event = "start_session_identity_switch_blocked", error = %error);
        }
    }

    pub async fn try_start_session(&self, contact: &Contact) -> Result<()> {
        let _session_gate = self.inner.core_state.identity_session_gate.lock().await;
        self.inner
            .core_state
            .add_desired_contact_infallible(contact.clone());
        self.await_runtime_applied(&CancellationToken::new())
            .await?;
        let attempt_id = self.inner.begin_direct_attempt(contact.peer_id);
        self.dial_session(contact, attempt_id).await?;
        Ok(())
    }

    async fn dial_session(&self, contact: &Contact, attempt_id: u64) -> Result<()> {
        if let Some(ref sender) = self.inner.start_session
            && sender.send((contact.peer_id, attempt_id)).await.is_err()
        {
            error!("start_session channel is closed");
            self.inner
                .complete_direct_attempt(contact.peer_id, attempt_id);
            return Err(ErrorKind::RuntimeNotReady.into());
        }
        if self.inner.start_session.is_none() {
            self.inner
                .complete_direct_attempt(contact.peer_id, attempt_id);
            return Err(ErrorKind::RuntimeNotReady.into());
        }
        Ok(())
    }

    /// Waits for the manager to apply the current runtime revision, bounded by
    /// `START_SESSION_RUNTIME_TIMEOUT`. Returns `RuntimeNotReady` immediately
    /// if no manager has been started, or on timeout. Selects against
    /// `operation` so a cancelled start_call / join_room does not block on a
    /// stalled manager.
    async fn await_runtime_applied(&self, operation: &CancellationToken) -> Result<()> {
        if self.inner.start_session.is_none() {
            return Err(ErrorKind::RuntimeNotReady.into());
        }
        let revision = self.inner.core_state.desired_runtime()?.revision;
        let wait = async {
            tokio::select! {
                biased;
                _ = operation.cancelled() => Ok(()),
                result = self.inner.core_state.wait_for_runtime_applied(revision) => result,
            }
        };
        timeout(START_SESSION_RUNTIME_TIMEOUT, wait)
            .await
            .unwrap_or_else(|_| Err(ErrorKind::RuntimeNotReady.into()))
    }

    /// Attempts to start a call through an existing session.
    ///
    /// [operation] is scoped to this request: a cancellation observed before
    /// the session task is notified releases its pending slot without affecting
    /// a later call attempt.
    pub async fn start_call(&self, contact: &Contact) -> Result<()> {
        self.start_call_with_operation(contact, &CancellationToken::new())
            .await
    }

    /// Attempts to start a call and observes cancellation for this operation.
    pub async fn start_call_with_operation(
        &self,
        contact: &Contact,
        operation: &CancellationToken,
    ) -> Result<()> {
        if operation.is_cancelled() {
            return Ok(());
        }
        self.await_runtime_applied(operation).await?;
        if operation.is_cancelled() {
            return Ok(());
        }

        loop {
            let availability = self.inner.session_availability_snapshot(contact.peer_id);
            if self
                .inner
                .session_states
                .read()
                .await
                .contains_key(&contact.peer_id)
            {
                break;
            }
            if !availability.waiting_for_session {
                if operation.is_cancelled() {
                    return Ok(());
                }
                return Err(ErrorKind::NoSessionForContact.into());
            }
            tokio::select! {
                biased;
                _ = operation.cancelled() => return Ok(()),
                _ = self.inner.wait_for_session_availability_change(contact.peer_id, availability.generation) => {}
            }
        }

        // Acquire the slot and capture its ownership snapshot atomically: both
        // happen under a single mutex hold inside `try_acquire_or_match_with_owner`,
        // so the snapshot reflects exactly the ownership this call established.
        // Deriving the snapshot from a later `snapshot()` call would re-acquire the
        // slot mutex and could observe a state that was transitioned or replaced
        // after acquisition (e.g. a concurrent handshake moving the slot to
        // `ActiveDirect`); releasing against that later snapshot would then release
        // a call this operation never owned.
        //
        // `Matched*` results are explicitly non-owning: the original acquirer owns
        // the slot and is responsible for its lifecycle, so `direct_owner` is `Some`
        // only on `Acquired`.
        let (slot_result, direct_owner) = {
            let state_lock = self.inner.session_states.read().await;
            if state_lock.get(&contact.peer_id).is_none() {
                return Err(ErrorKind::NoSessionForContact.into());
            }
            self.inner
                .core_state
                .call_slot
                .try_acquire_or_match_with_owner(CallSlotState::PendingOutgoing, contact.peer_id)?
        };

        if slot_result == CallSlotAcquireResult::Failed {
            return Err(ErrorKind::CallAlreadyActive.into());
        }

        if operation.is_cancelled() {
            if let Some(owner) = direct_owner {
                let _ = self.inner.core_state.call_slot.release_if_match(owner)?;
            }
            return Ok(());
        }

        // The slot is already `PendingOutgoing` for this peer, meaning the session task
        // has already consumed the original `notify_one` and is currently negotiating the
        // outgoing call. No additional notification is needed — the negotiation is already
        // in progress.
        if matches!(slot_result, CallSlotAcquireResult::MatchedPendingOutgoing) {
            return Ok(());
        }

        #[cfg(target_family = "wasm")]
        {
            if let Err(error) = self.inner.init_web_audio().await {
                if let Some(owner) = direct_owner {
                    let _ = self.inner.core_state.call_slot.release_if_match(owner)?;
                }
                return Err(error);
            }
            if operation.is_cancelled() {
                if let Some(owner) = direct_owner {
                    let _ = self.inner.core_state.call_slot.release_if_match(owner)?;
                }
                return Ok(());
            }
        }

        let state_lock = self.inner.session_states.read().await;
        if operation.is_cancelled() {
            if let Some(owner) = direct_owner {
                let _ = self.inner.core_state.call_slot.release_if_match(owner)?;
            }
            return Ok(());
        }
        if let Some(state) = state_lock.get(&contact.peer_id) {
            if operation.is_cancelled() {
                if let Some(owner) = direct_owner {
                    let _ = self.inner.core_state.call_slot.release_if_match(owner)?;
                }
                return Ok(());
            }
            state.start_call.notify_one();
            Ok(())
        } else {
            warn!(
                event = "start_call_no_current_session_releasing_slot",
                peer.id = %contact.peer_id,
            );
            if let Some(owner) = direct_owner {
                let _ = self.inner.core_state.call_slot.release_if_match(owner)?;
            }
            Err(ErrorKind::NoSessionForContact.into())
        }
    }

    /// Ends the current audio test, room, or call in that order
    pub async fn end_call(&self) {
        let owner = match self.inner.core_state.call_slot.snapshot() {
            Ok(owner) if owner.state != CallSlotState::Idle => owner,
            Ok(_) => {
                warn!("end_call failed to end anything");
                return;
            }
            Err(error) => {
                error!("end_call could not snapshot call slot: {error}");
                return;
            }
        };

        if let Some(end_audio_test) = self.inner.core_state.end_audio_test.lock().await.as_ref() {
            debug!("ending audio test");
            end_audio_test.notify_one();
        } else if let Some(room_state) = self.inner.room_state.read().await.as_ref() {
            debug!("ending room");
            room_state.end_call.notify_one();
        } else if let Some(peer) = owner.direct_peer
            && let Some(session_state) = self.inner.session_states.read().await.get(&peer)
        {
            debug!("ending call");
            session_state.end_call.notify_one();
        } else {
            warn!("end_call failed to end anything");
            return;
        }

        // Bounded fallback: a controller parked on a stalled frontend callback
        // (or any other path that cannot observe `end_call`/`stop_session`)
        // would otherwise keep the slot non-idle forever. `wait_for_release`
        // itself is generation-safe — it returns once the slot is Idle OR a
        // newer generation replaces `owner` — so a timeout elapsed here means
        // the slot is genuinely stuck on the same owner. Return anyway so the
        // public API stays responsive; the controller will finish teardown
        // asynchronously once its callback is abandoned.
        match timeout(
            END_CALL_SLOT_RELEASE_TIMEOUT,
            self.inner.core_state.call_slot.wait_for_release(owner),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                error!("end_call could not confirm call slot release: {error}");
            }
            Err(_elapsed) => {
                let current = self.inner.core_state.call_slot.snapshot();
                error!(
                    event = "end_call_slot_release_timeout",
                    owner = ?owner,
                    current = ?current,
                    "end_call timed out waiting for call slot release; \
                     returning without confirmation",
                );
            }
        }
    }

    /// The only entry point into participating in a room.
    pub async fn join_room(&self, member_strings: Vec<String>) -> Result<()> {
        self.join_room_with_operation(member_strings, &CancellationToken::new())
            .await
    }

    /// Joins a room and observes cancellation for this operation.
    pub async fn join_room_with_operation(
        &self,
        member_strings: Vec<String>,
        operation: &CancellationToken,
    ) -> Result<()> {
        if operation.is_cancelled() {
            return Ok(());
        }
        self.await_runtime_applied(operation).await?;

        // Parse the exact authorized member set before claiming RoomCall ownership.
        let members: Vec<_> = member_strings
            .into_iter()
            .filter_map(|member| member.parse().ok())
            .collect();

        let Some(room_owner) = self
            .inner
            .core_state
            .call_slot
            .try_acquire_with_snapshot(CallSlotState::RoomCall)?
        else {
            return Err(ErrorKind::CallAlreadyActive.into());
        };
        // No await/yield is allowed between slot acquisition and admission publication.
        let mut pending_admission = Some(
            self.inner
                .install_pending_room_admission(room_owner, &members),
        );

        if operation.is_cancelled() {
            let _ = self
                .inner
                .core_state
                .call_slot
                .release_if_match(room_owner)?;
            return Ok(());
        }

        #[cfg(target_family = "wasm")]
        {
            if let Err(error) = self.inner.init_web_audio().await {
                let _ = self
                    .inner
                    .core_state
                    .call_slot
                    .release_if_match(room_owner)?;
                return Err(error);
            }
            if operation.is_cancelled() {
                let _ = self
                    .inner
                    .core_state
                    .call_slot
                    .release_if_match(room_owner)?;
                return Ok(());
            }
        }

        // delivers messages from each session to the room controller
        let (sender, receiver) = channel(32);
        // cancels all processing threads
        let cancel = CancellationToken::new();
        // gracefully ends the room call
        let end_call = Arc::new(Notify::new());
        // the same early call state is used throughout the room, the real peer ids are set later
        // Pre-spawn cancellation branches release the slot directly (no controller yet);
        // post-spawn branches route through `abort_room_generation` to await this exact
        // generation's controller teardown.
        let call_state = {
            let result = tokio::select! {
                state = self.inner.setup_call(SecretKey::generate().public()) => state,
                _ = operation.cancelled() => {
                    let _ = self
                        .inner
                        .core_state
                        .call_slot
                        .release_if_match(room_owner)?;
                    return Ok(());
                }
            };
            match result {
                Ok(state) => state,
                Err(error) => {
                    let _ = self
                        .inner
                        .core_state
                        .call_slot
                        .release_if_match(room_owner)?;
                    return Err(error);
                }
            }
        };
        // acquire fresh generation for the new state
        let room_generation = self
            .inner
            .core_state
            .next_room_generation
            .fetch_add(1, Relaxed)
            .saturating_add(1);
        let (ready_sender, ready_receiver) = oneshot::channel();
        let (publication_sender, publication_receiver) = oneshot::channel();
        let (controller_completion_sender, mut controller_completion_receiver) = oneshot::channel();
        let self_clone = self.inner.clone();
        let controller_cancel = cancel.clone();
        let controller_end_call = Arc::clone(&end_call);
        let controller_operation = operation.clone();
        {
            let mut handles = tokio::select! {
                guard = self.handles.lock() => guard,
                _ = operation.cancelled() => {
                    let _ = self
                        .inner
                        .core_state
                        .call_slot
                        .release_if_match(room_owner)?;
                    return Ok(());
                }
            };
            handles.push(spawn_task(
                async move {
                    let stop_io = Default::default();
                    let outcome = self_clone
                        .room_controller(
                            receiver,
                            &stop_io,
                            RoomControllerStart {
                                end_sessions: controller_cancel,
                                end_call: controller_end_call,
                                operation: controller_operation,
                                room_owner,
                                room_generation,
                                ready_sender,
                                publication_receiver,
                            },
                        )
                        .await;
                    let _ = controller_completion_sender.send(());
                    if let Some(message) = outcome.into_message() {
                        self_clone
                            .callbacks
                            .call_state(crate::types::CallState::CallEnded(message, false))
                            .await;
                    }
                    stop_io.cancel();
                }
                .in_current_span(),
            ));
        }

        let setup_has_stream_error = tokio::select! {
            ready = ready_receiver => match ready {
                Ok(value) => value,
                Err(_) => return Ok(()),
            },
            _ = operation.cancelled() => {
                drop(pending_admission.take());
                abort_room_generation(&cancel, &end_call, &mut controller_completion_receiver)
                    .await;
                return Ok(());
            }
        };

        // Publish this generation before the controller can process stream errors.
        let mut room_guard = tokio::select! {
            guard = self.inner.room_state.write() => guard,
            _ = operation.cancelled() => {
                drop(pending_admission.take());
                abort_room_generation(&cancel, &end_call, &mut controller_completion_receiver)
                    .await;
                return Ok(());
            }
        };
        if operation.is_cancelled() {
            drop(room_guard);
            drop(pending_admission.take());
            abort_room_generation(&cancel, &end_call, &mut controller_completion_receiver).await;
            return Ok(());
        }
        let old_state_option = room_guard.replace(RoomState {
            peers: members.clone(),
            sender,
            cancel: cancel.clone(),
            end_call: end_call.clone(),
            early_state: call_state.clone(),
            generation: room_generation,
        });
        drop(room_guard);
        if let Some(pending_admission) = pending_admission.take() {
            pending_admission.publish();
        }
        self.inner.request_room_reconcile();
        if operation.is_cancelled() {
            drop(publication_sender);
            abort_room_generation(&cancel, &end_call, &mut controller_completion_receiver).await;
            return Ok(());
        }
        if publication_sender.send(()).is_err() || setup_has_stream_error {
            return Ok(());
        }

        // clean up old state
        if operation.is_cancelled() {
            abort_room_generation(&cancel, &end_call, &mut controller_completion_receiver).await;
            return Ok(());
        }
        if let Some(old_state) = old_state_option {
            old_state.cancel.cancel();
            old_state.end_call.notify_one();
        }
        for member in members {
            let read_guard = tokio::select! {
                guard = self.inner.session_states.read() => guard,
                _ = operation.cancelled() => {
                    abort_room_generation(
                        &cancel,
                        &end_call,
                        &mut controller_completion_receiver,
                    )
                    .await;
                    return Ok(());
                }
            };
            if let Some(state) = read_guard.get(&member) {
                state.start_call.notify_one();
            }
        }
        Ok(())
    }

    /// Restarts the session manager
    pub async fn restart_manager(&self) -> Result<()> {
        if self.inner.core_state.call_slot.current() != CallSlotState::Idle {
            return Err(ErrorKind::ManagerRestartDuringCall.into());
        }
        let revision = self.inner.core_state.restart_desired_runtime_infallible();
        self.inner
            .core_state
            .wait_for_runtime_applied(revision)
            .await?;
        Ok(())
    }

    pub async fn prepare_identity_switch(
        &self,
        target_key: [u8; 32],
        target_contacts: Vec<Contact>,
    ) -> Result<PreparedIdentitySwitch> {
        let target_identity = SecretKey::from_bytes(&target_key);
        validate_identity_switch_contacts(&target_contacts)?;
        let session_gate = self
            .inner
            .core_state
            .identity_session_gate
            .clone()
            .lock_owned()
            .await;
        let lease = PreparedSwitchLease::acquire(&self.inner.core_state.call_slot)?;
        Ok(PreparedIdentitySwitch::new(
            target_identity,
            target_contacts,
            self.inner.core_state.clone(),
            session_gate,
            lease,
        ))
    }

    /// shuts down the entire rust backend
    pub async fn shutdown(&self) {
        // stops sessions & manager
        self.inner.shutdown().await;
        // wait for manager & any room controllers to join
        let handles: Vec<_> = self.handles.lock().await.drain(..).collect();
        for handle in handles {
            if let Err(error) = handle.await {
                error!(event = "shutdown_task_join_failed", error = %error);
            }
        }
        info!("shutdown complete");
    }

    /// Sets the signing key (called when the profile changes)
    pub async fn set_identity(&self, key: &[u8; 32]) -> Result<()> {
        let identity = SecretKey::from_bytes(key);
        *self.inner.core_state.identity.write().await = Some(identity.clone());
        self.inner
            .core_state
            .replace_desired_identity_infallible(identity);
        Ok(())
    }

    /// Stops a specific session (called when a contact is deleted)
    pub async fn stop_session(&self, contact: &Contact) {
        self.inner
            .core_state
            .remove_desired_contact_infallible(contact.peer_id);
        // clear volume cache entry for contact
        if let Err(error) = self
            .inner
            .core_state
            .reset_peer_output_volume(&contact.peer_id)
        {
            error!("reset_peer_output_volume failed: {}", error);
        }
        // remove the session entry from the map under the write lock before releasing
        // the call slot, so a replacement session that has already entered the map
        // cannot be clobbered by the slot release.
        let removed_state = self
            .inner
            .session_states
            .write()
            .await
            .remove(&contact.peer_id);
        if let Err(error) = self
            .inner
            .core_state
            .call_slot
            .release_if_pending_for_peer(contact.peer_id)
        {
            error!("release_if_pending_for_peer failed: {}", error);
        }
        if let Some(state) = removed_state {
            state.stop_session.cancel();
        }
        self.inner.request_room_reconcile();
    }

    /// Blocks while an audio test is running
    pub async fn audio_test(&self) -> Result<()> {
        self.await_runtime_applied(&CancellationToken::new())
            .await?;
        if !self
            .inner
            .core_state
            .call_slot
            .try_acquire(CallSlotState::AudioTest, None)?
        {
            return Err(ErrorKind::CallAlreadyActive.into());
        }

        // update state right away to handle the test being ended quickly
        let end_call = Arc::new(Notify::new());
        *self.inner.core_state.end_audio_test.lock().await = Some(end_call.clone());

        #[cfg(target_family = "wasm")]
        if let Err(error) = self.inner.init_web_audio().await {
            // clean up state before propagating error
            self.inner.core_state.end_audio_test.lock().await.take();
            self.inner.core_state.call_slot.release()?;
            return Err(error);
        }

        let peer_id = SecretKey::generate().public();
        let result = match self.inner.setup_call(peer_id).await {
            Ok(mut audio_config) => {
                audio_config.remote_configuration = audio_config.local_configuration.clone();
                let stop_io = CancellationToken::new();
                let call_span = info_span!(
                    "call.run",
                    call.kind = "audio_test",
                    peer.id = %audio_config.peer,
                    codec.enabled = audio_config.codec_config().0,
                    sample_rate = audio_config.remote_configuration.sample_rate
                );
                let result = self
                    .inner
                    .call(&stop_io, audio_config, &end_call, None)
                    .instrument(call_span)
                    .await;
                stop_io.cancel();
                result.map(|_| ())
            }
            Err(error) => Err(error),
        };

        self.inner.core_state.reset_peer_output_volume(&peer_id)?;
        self.inner.core_state.end_audio_test.lock().await.take();
        self.inner.core_state.call_slot.release()?;
        result
    }

    pub fn build_chat(
        &self,
        contact: &Contact,
        text: String,
        attachments: Vec<(String, Vec<u8>)>,
    ) -> ChatMessage {
        ChatMessage {
            text,
            receiver: contact.peer_id,
            timestamp: Local::now(),
            attachments: attachments
                .into_iter()
                .map(|(name, data)| Attachment { name, data })
                .collect(),
        }
    }

    /// Sends a chat message
    pub async fn send_chat(&self, message: &mut ChatMessage) -> Result<()> {
        if !chat_message_fits_frame(&message.text, &message.attachments) {
            return Err(ErrorKind::AttachmentsTooLarge.into());
        }

        let Some(state) = self
            .inner
            .session_states
            .read()
            .await
            .get(&message.receiver)
            .cloned()
        else {
            warn!(
                event = "edge_case",
                case = "send_chat_without_session",
                peer.id = %message.receiver
            );
            return Ok(());
        };

        // take the data out of each attachment. the frontend doesn't need it
        let attachments = message
            .attachments
            .iter_mut()
            .map(|attachment| Attachment {
                name: attachment.name.clone(),
                data: mem::take(&mut attachment.data),
            })
            .collect();

        let message = ProtocolMessage::Chat {
            text: message.text.clone(),
            attachments,
        };

        state
            .message_sender
            .send(message)
            .await
            .map_err(|_| Error::from(ErrorKind::MpscSend))?;
        Ok(())
    }

    pub async fn start_screenshare(&self, contact: &Contact) {
        if let Some(state) = self.inner.session_states.read().await.get(&contact.peer_id) {
            state.start_screenshare.notify_one();
        }
    }

    pub fn set_rms_threshold(&self, decimal: f32) {
        self.inner.core_state.set_rms_threshold(decimal);
    }

    pub fn set_input_volume(&self, decibel: f32) {
        self.inner.core_state.set_input_volume(decibel)
    }

    pub fn set_output_volume(&self, decibel: f32) -> Result<()> {
        self.inner.core_state.set_output_volume(decibel)
    }

    pub fn set_contact_output_volume(&self, contact: &Contact) -> Result<()> {
        self.inner.core_state.set_peer_output_volume(contact)
    }

    pub fn set_deafened(&self, deafened: bool) {
        self.inner.core_state.deafened.store(deafened, Relaxed);
    }

    pub fn set_muted(&self, muted: bool) {
        self.inner.core_state.muted.store(muted, Relaxed);
    }

    /// Changing the denoise flag will not affect the current call
    pub fn set_denoise(&self, denoise: bool) {
        self.inner.core_state.denoise.store(denoise, Relaxed);
    }

    pub fn set_play_custom_ringtones(&self, play: bool) {
        self.inner
            .core_state
            .play_custom_ringtones
            .store(play, Relaxed);
    }

    pub fn set_send_custom_ringtone(&self, send: bool) {
        self.inner
            .core_state
            .send_custom_ringtone
            .store(send, Relaxed);
    }

    pub fn set_efficiency_mode(&self, enabled: bool) {
        self.inner
            .core_state
            .efficiency_mode
            .store(enabled, Relaxed);
    }

    pub fn pause_statistics(&self) {
        self.inner.core_state.statistics_paused.store(true, Relaxed);
    }

    pub fn resume_statistics(&self) {
        self.inner
            .core_state
            .statistics_paused
            .store(false, Relaxed);
    }

    pub async fn set_input_device(&self, device_id: Option<String>) {
        *self.inner.core_state.input_device.lock().await = device_id;
    }

    pub async fn set_output_device(&self, device_id: Option<String>) {
        *self.inner.core_state.output_device.lock().await = device_id;
    }

    /// Lists the input and output devices
    pub fn list_devices(&self) -> Result<(Vec<AudioDevice>, Vec<AudioDevice>)> {
        let device_list = self.inner.host.list_all_devices().map_err(Error::from)?;
        Ok((
            device_list
                .input_devices
                .into_iter()
                .map(AudioDevice::from)
                .collect(),
            device_list
                .output_devices
                .into_iter()
                .map(AudioDevice::from)
                .collect(),
        ))
    }

    pub async fn set_model(&self, model: Option<Vec<u8>>) -> Result<()> {
        let model = if let Some(mode_bytes) = model {
            RnnModel::from_bytes(&mode_bytes).ok_or_else(|| Error::from(ErrorKind::InvalidModel))?
        } else {
            RnnModel::default()
        };

        *self.inner.core_state.denoise_model.write().await = model;
        Ok(())
    }

    /// Returns the local endpoint's opaque direct invitation. Returns `None`
    /// if the session manager is not active or has no bound addresses.
    pub async fn node_addr(&self) -> Option<String> {
        direct_invitation::encode(
            self.inner.peer_id().await,
            self.inner.core_state.get_endpoint_addrs(),
        )
    }
}

fn validate_identity_switch_contacts(contacts: &[Contact]) -> Result<()> {
    let mut peers = HashSet::with_capacity(contacts.len());
    if contacts
        .iter()
        .any(|contact| contact.is_room_only || !peers.insert(contact.peer_id))
    {
        return Err(ErrorKind::InvalidContactFormat.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Attachment, BoundedSizeWriter, ProtocolMessage, SESSION_MAX_FRAME_LENGTH, Writer,
        chat_message_fits_frame,
    };
    use bytes::BytesMut;
    use speedy::Writable;
    use tokio_util::codec::{Decoder, LengthDelimitedCodec};

    #[test]
    fn session_codec_rejects_oversized_length_prefix_before_payload_allocation() {
        let mut codec = LengthDelimitedCodec::builder()
            .max_frame_length(SESSION_MAX_FRAME_LENGTH)
            .length_field_type::<u64>()
            .new_codec();
        let mut input = BytesMut::from(&(SESSION_MAX_FRAME_LENGTH as u64 + 1).to_be_bytes()[..]);

        assert!(codec.decode(&mut input).is_err());
    }

    #[test]
    fn chat_frame_accepts_exact_limit_attachment_metadata() {
        // Enum tag, text length, attachment count, attachment name length, and
        // attachment data length are each encoded as u32.
        const METADATA_LENGTH: usize = 5 * size_of::<u32>();
        let attachments = [Attachment {
            name: "m".repeat(SESSION_MAX_FRAME_LENGTH - METADATA_LENGTH),
            data: Vec::new(),
        }];

        assert!(chat_message_fits_frame("", &attachments));
        assert_eq!(
            ProtocolMessage::Chat {
                text: String::new(),
                attachments: attachments.into(),
            }
            .write_to_vec()
            .unwrap()
            .len(),
            SESSION_MAX_FRAME_LENGTH
        );
    }

    #[test]
    fn chat_frame_rejects_oversized_text() {
        let text = "x".repeat(SESSION_MAX_FRAME_LENGTH - 11);

        assert!(!chat_message_fits_frame(&text, &[]));
    }

    #[test]
    fn chat_frame_rejects_oversized_attachment_name() {
        let attachment = Attachment {
            name: "n".repeat(SESSION_MAX_FRAME_LENGTH),
            data: Vec::new(),
        };

        assert!(!chat_message_fits_frame("", &[attachment]));
    }

    #[test]
    fn chat_frame_accounts_for_all_attachments() {
        let attachment = || Attachment {
            name: "quarter.bin".to_string(),
            data: vec![7; SESSION_MAX_FRAME_LENGTH / 2],
        };

        assert!(!chat_message_fits_frame("", &[attachment(), attachment()]));
    }

    #[test]
    fn chat_frame_handles_overflow_safe_accumulation() {
        let mut writer = BoundedSizeWriter { size: usize::MAX };

        assert!(writer.write_bytes(&[0]).is_err());
        assert_eq!(writer.size, usize::MAX);
    }

    #[test]
    fn chat_frame_rejects_very_large_payload_without_copying_it() {
        let data = vec![0xA5; SESSION_MAX_FRAME_LENGTH * 8];
        let attachments = [Attachment {
            name: "archive.bin".to_string(),
            data,
        }];
        let original_data = attachments[0].data.as_ptr();

        assert!(!chat_message_fits_frame("", &attachments));
        assert_eq!(attachments[0].data.as_ptr(), original_data);
        assert_eq!(attachments[0].data.len(), SESSION_MAX_FRAME_LENGTH * 8);
    }
}
