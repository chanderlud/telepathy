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
use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use crate::internal::core::TelepathyCore;
use crate::internal::error::{Error, ErrorKind};
use crate::internal::messages::{Attachment, ProtocolMessage};
use crate::internal::state::{
    CallSlotAcquireResult, CallSlotState, EarlyCallState, RoomState, SessionState,
};
pub(crate) use crate::internal::utils::{JoinHandle, spawn_task};
use crate::overlay::Overlay;
use crate::types::{ChatMessage, CodecConfig, Contact, NetworkConfig, ScreenshareConfig};
use chrono::Local;
use iroh::SecretKey;
use std::mem;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use telepathy_audio::RnnModel;
use telepathy_audio::devices::AudioHost;
use tokio::sync::mpsc::channel;
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;
use tracing::{debug, error, info, info_span, warn};
#[cfg(target_family = "wasm")]
use wasmtimer::tokio::timeout;

type Result<T> = std::result::Result<T, Error>;

/// A timeout used when initializing the call
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// How often to keep-alive libp2p streams
const KEEP_ALIVE: Duration = Duration::from_secs(10);
/// the protocol identifier for Telepathy sessions
const ALPN: &[u8] = b"telepathy/session/0";
/// Maximum allowed size for a single length-delimited control/message frame on the session stream.
const SESSION_MAX_FRAME_LENGTH: usize = 1024 * 1024 * 1024;

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
        if let Some(handle) = self.inner.start_manager().await {
            self.handles.lock().await.push(handle);
        }
    }

    /// Tries to start a session for a contact
    pub async fn start_session(&self, contact: &Contact) {
        debug!("start_session called for {}", contact.peer_id);

        if let Some(ref sender) = self.inner.start_session
            && sender.send(contact.peer_id).await.is_err()
        {
            error!("start_session channel is closed");
        }
    }

    /// Attempts to start a call through an existing session
    pub async fn start_call(&self, contact: &Contact) -> Result<()> {
        // The session presence check and the pending-slot acquisition are
        // atomic: both happen under the same `session_states` read lock
        // guard, so the slot can only be acquired for a session that is
        // currently in the map.
        // The subsequent `notify_one` is a separate, best-effort operation:
        // if the session has been removed in the meantime (after the guard
        // is released), the acquired slot is released and `NoSessionForContact`
        // is returned to avoid leaking the slot.
        let slot_result = {
            let state_lock = self.inner.session_states.read().await;
            if state_lock.get(&contact.peer_id).is_none() {
                return Err(ErrorKind::NoSessionForContact.into());
            }
            self.inner
                .core_state
                .call_slot
                .try_acquire_or_match(CallSlotState::PendingOutgoing, contact.peer_id)?
        };

        if slot_result == CallSlotAcquireResult::Failed {
            return Err(ErrorKind::CallAlreadyActive.into());
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
                self.inner
                    .core_state
                    .call_slot
                    .release_if_pending_for_peer(contact.peer_id)?;
                return Err(error);
            }
        }

        let state_lock = self.inner.session_states.read().await;
        if let Some(state) = state_lock.get(&contact.peer_id) {
            state.start_call.notify_one();
            Ok(())
        } else {
            warn!(
                event = "start_call_no_current_session_releasing_slot",
                peer.id = %contact.peer_id,
            );
            self.inner
                .core_state
                .call_slot
                .release_if_pending_for_peer(contact.peer_id)?;
            Err(ErrorKind::NoSessionForContact.into())
        }
    }

    /// Ends the current audio test, room, or call in that order
    pub async fn end_call(&self) {
        if let Some(end_audio_test) = self.inner.core_state.end_audio_test.lock().await.as_ref() {
            debug!("ending audio test");
            end_audio_test.notify_one();
        } else if let Some(room_state) = self.inner.room_state.read().await.as_ref() {
            debug!("ending room");
            room_state.end_call.notify_one();
        } else if let Ok(Some(peer)) = self
            .inner
            .core_state
            .call_slot
            .snapshot()
            .map(|s| s.direct_peer)
            && let Some(session_state) = self.inner.session_states.read().await.get(&peer)
        {
            debug!("ending call");
            session_state.end_call.notify_one();
        } else {
            warn!("end_call failed to end anything");
        }
    }

    /// The only entry point into participating in a room
    pub async fn join_room(&self, member_strings: Vec<String>) -> Result<()> {
        if !self
            .inner
            .core_state
            .call_slot
            .try_acquire(CallSlotState::RoomCall, None)?
        {
            return Err(ErrorKind::CallAlreadyActive.into());
        }

        #[cfg(target_family = "wasm")]
        if let Err(error) = self.inner.init_web_audio().await {
            self.inner.core_state.call_slot.release()?;
            return Err(error);
        }

        // capture the exact ownership snapshot this room acquired so the room controller's
        // teardown can release the slot against the same generation we own, even if the slot
        // was released and re-acquired (e.g. a newer room) while the controller was running.
        let room_owner = match self.inner.core_state.call_slot.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.inner.core_state.call_slot.release()?;
                return Err(error);
            }
        };

        // parse members
        let members: Vec<_> = member_strings
            .into_iter()
            .filter_map(|m| m.parse().ok())
            .collect();
        // delivers messages from each session to the room controller
        let (sender, receiver) = channel(32);
        // cancels all processing threads
        let cancel = CancellationToken::new();
        // gracefully ends the room call
        let end_call = Arc::new(Notify::new());
        // the same early call state is used throughout the room, the real peer ids are set later
        let call_state = match self.inner.setup_call(SecretKey::generate().public()).await {
            Ok(state) => state,
            Err(error) => {
                self.inner.core_state.call_slot.release()?;
                return Err(error);
            }
        };
        // acquire fresh generation for the new state
        let room_generation = self
            .inner
            .core_state
            .next_room_generation
            .fetch_add(1, Relaxed)
            .saturating_add(1);
        // set room state
        let old_state_option = self.inner.room_state.write().await.replace(RoomState {
            peers: members.clone(),
            sender,
            cancel: cancel.clone(),
            end_call: end_call.clone(),
            early_state: call_state.clone(),
            generation: room_generation,
        });
        // clean up old state
        if let Some(old_state) = old_state_option {
            old_state.cancel.cancel();
            old_state.end_call.notify_one();
        }
        // tries to connect to every member of the room using existing sessions, or new ones if needed
        // note: sending your own identity to start_session is safe
        for member in members {
            if let Some(state) = self.inner.session_states.read().await.get(&member) {
                state.start_call.notify_one();
            } else if let Some(ref sender) = self.inner.start_session {
                // when the session opens, start_call will be notified
                _ = sender.send(member).await;
            }
        }
        // spawn room controller
        let self_clone = self.inner.clone();
        self.handles.lock().await.push(spawn_task(
            async move {
                let stop_io = Default::default();
                if let Err(error) = self_clone
                    .room_controller(
                        receiver,
                        cancel,
                        &stop_io,
                        end_call,
                        room_owner,
                        room_generation,
                    )
                    .await
                {
                    error!("error in room controller: {:?}", error);
                }
                stop_io.cancel();
            }
            .in_current_span(),
        ));

        Ok(())
    }

    /// Restarts the session manager
    pub async fn restart_manager(&self) -> Result<()> {
        if self.inner.core_state.call_slot.current() != CallSlotState::Idle {
            Err(ErrorKind::ManagerRestartDuringCall.into())
        } else {
            // reset sessions so manager can clean up
            self.inner.reset_sessions().await;
            // restart the manager
            self.inner.restart_manager.notify_one();
            // wait for a new manager to start
            self.inner.core_state.manager_active.notified().await;
            // ensure volume cache resets fully
            self.inner.core_state.reset_peer_output_volumes()?;
            // start a session for all contacts
            for contact in self.inner.callbacks.get_contacts().await {
                self.start_session(&contact).await;
            }
            Ok(())
        }
    }

    /// shuts down the entire rust backend
    pub async fn shutdown(&self) {
        // stops sessions & manager
        self.inner.shutdown().await;
        // wait for manager & any room controllers to join
        let handles: Vec<_> = self.handles.lock().await.drain(..).collect();
        for handle in handles {
            handle.await.unwrap();
        }
        info!("shutdown complete");
    }

    /// Sets the signing key (called when the profile changes)
    pub async fn set_identity(&self, key: &[u8; 32]) -> Result<()> {
        *self.inner.core_state.identity.write().await = Some(SecretKey::from_bytes(key));
        Ok(())
    }

    /// Stops a specific session (called when a contact is deleted)
    pub async fn stop_session(&self, contact: &Contact) {
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
    }

    /// Blocks while an audio test is running
    pub async fn audio_test(&self) -> Result<()> {
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
                result
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
        if message
            .attachments
            .iter()
            .map(|a| a.data.len())
            .sum::<usize>()
            > SESSION_MAX_FRAME_LENGTH
        {
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
}
