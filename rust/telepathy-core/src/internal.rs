/// callback traits shared by FRB and native frontends
pub(crate) mod callbacks;
/// implementations for core telepathy functionality
mod core;
pub(crate) mod error;
/// helper methods used by telepathy core
mod helpers;
pub(crate) mod messages;
pub(crate) mod screenshare;
/// networking code for live audio streams
mod sockets;
mod state;
mod utils;

use crate::AudioDevice;
use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use crate::internal::core::TelepathyCore;
use crate::internal::error::Error;
use crate::internal::messages::{Attachment, ProtocolMessage};
use crate::internal::state::{EarlyCallState, RoomState, SessionState};
pub(crate) use crate::internal::utils::{JoinHandle, spawn_task};
use crate::overlay::Overlay;
use crate::types::{
    ChatMessage, CodecConfig, Contact, DartError, NetworkConfig, ScreenshareConfig,
};
use chrono::Local;
use libp2p::identity::Keypair;
use libp2p::{PeerId, StreamProtocol};
use std::mem;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Duration;
use telepathy_audio::devices::{AudioHost};
use telepathy_audio::internal::utils::db_to_multiplier;
use telepathy_audio::RnnModel;
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
/// the protocol identifier for Telepathy
const CHAT_PROTOCOL: StreamProtocol = StreamProtocol::new("/telepathy/0.0.1");
/// Maximum allowed size for a single length-delimited control/message frame on the session stream.
const SESSION_MAX_FRAME_LENGTH: usize = 1024 * 1024 * 1024;
/// How long to attempt direct connection upgrade before falling back to a relayed option
const DCUTR_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct TelepathyHandle<C, S, H>
where
    C: CoreCallbacks<S> + Send + Sync + 'static,
    S: CoreStatisticsCallback + Send + Sync + 'static,
    H: AudioHost + Send + Sync + Clone + 'static,
{
    inner: TelepathyCore<C, S, H>,

    /// contains handles to the manager thread & room managers
    handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

// TODO refactor all methods returning DartError to return Error
impl<C, S, H> TelepathyHandle<C, S, H>
where
    C: CoreCallbacks<S> + Send + Sync + 'static,
    S: CoreStatisticsCallback + Send + Sync + 'static,
    H: AudioHost + Send + Sync + Clone + 'static,
{
    /// Builds a new handle around a fresh `TelepathyCore`.
    pub(crate) fn new(
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
    pub async fn start_call(&self, contact: &Contact) -> std::result::Result<(), DartError> {
        if self.inner.is_call_active().await {
            return Err("Cannot start call while a call is already active"
                .to_string()
                .into());
        }

        if let Some(state) = self.inner.session_states.read().await.get(&contact.peer_id) {
            #[cfg(target_family = "wasm")]
            self.inner
                .init_web_audio()
                .await
                .map_err::<Error, _>(Error::into)?;

            state.start_call.notify_one();
            Ok(())
        } else {
            Err(String::from("No session found for contact").into())
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
        } else if let Some(session_state) = self
            .inner
            .session_states
            .read()
            .await
            .values()
            .find(|s| s.in_call.load(Relaxed))
        {
            debug!("ending call");
            session_state.end_call.notify_one();
        } else {
            warn!("end_call failed to end anything");
        }
    }

    /// The only entry point into participating in a room
    pub async fn join_room(
        &self,
        member_strings: Vec<String>,
    ) -> std::result::Result<(), DartError> {
        if self.inner.is_call_active().await {
            return Err("Cannot join room while a call is already active"
                .to_string()
                .into());
        }

        #[cfg(target_family = "wasm")]
        self.inner
            .init_web_audio()
            .await
            .map_err::<Error, _>(Error::into)?;

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
        let call_state = self.inner.setup_call(PeerId::random()).await?;
        // set room state
        let old_state_option = self.inner.room_state.write().await.replace(RoomState {
            peers: members.clone(),
            sender,
            cancel: cancel.clone(),
            end_call: end_call.clone(),
            early_state: call_state.clone(),
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
                    .room_controller(receiver, cancel, &stop_io, end_call)
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
    pub async fn restart_manager(&self) -> std::result::Result<(), DartError> {
        if self.inner.is_call_active().await {
            Err("Cannot restart manager while call is active"
                .to_string()
                .into())
        } else {
            // reset sessions so manager can clean up
            self.inner.reset_sessions().await;
            // restart the manager
            self.inner.restart_manager.notify_one();
            // wait for a new manager to start
            self.inner.core_state.manager_active.notified().await;
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
        for handle in self.handles.lock().await.drain(..) {
            handle.await.unwrap();
        }
        info!("shutdown complete");
    }

    /// Sets the signing key (called when the profile changes)
    pub async fn set_identity(&self, key: Vec<u8>) -> std::result::Result<(), DartError> {
        *self.inner.core_state.identity.write().await =
            Some(Keypair::from_protobuf_encoding(&key).map_err(Error::from)?);
        Ok(())
    }

    /// Stops a specific session (called when a contact is deleted)
    pub async fn stop_session(&self, contact: &Contact) {
        if let Some(state) = self
            .inner
            .session_states
            .write()
            .await
            .remove(&contact.peer_id)
        {
            state.stop_session.cancel();
        }
    }

    /// Blocks while an audio test is running
    pub async fn audio_test(&self) -> std::result::Result<(), DartError> {
        if self.inner.is_call_active().await {
            return Err("Cannot start test while call is active".to_string().into());
        }

        // update state right away to handle the test being ended quickly
        let end_call = Arc::new(Notify::new());
        self.inner.core_state.in_call.store(true, Relaxed);
        *self.inner.core_state.end_audio_test.lock().await = Some(end_call.clone());

        #[cfg(target_family = "wasm")]
        if let Err(error) = self.inner.init_web_audio().await {
            // clean up state before propagating error
            self.inner.core_state.end_audio_test.lock().await.take();
            self.inner.core_state.in_call.store(false, Relaxed);
            return Err(Error::into(error));
        }

        let result = match self.inner.setup_call(PeerId::random()).await {
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
                    .await
                    .map_err(Into::into);
                stop_io.cancel();
                result
            }
            Err(error) => Err(Error::into(error)),
        };

        self.inner.core_state.end_audio_test.lock().await.take();
        self.inner.core_state.in_call.store(false, Relaxed);
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
    pub async fn send_chat(&self, message: &mut ChatMessage) -> std::result::Result<(), DartError> {
        if message
            .attachments
            .iter()
            .map(|a| a.data.len())
            .sum::<usize>()
            > SESSION_MAX_FRAME_LENGTH
        {
            return Err("attachments too large".to_string().into());
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
            .map_err(|_| "channel closed".to_string())?;
        Ok(())
    }

    pub async fn start_screenshare(&self, contact: &Contact) {
        self.inner
            .send_start_screenshare(contact.peer_id, None)
            .await;
    }

    pub fn set_rms_threshold(&self, decimal: f32) {
        let threshold = db_to_multiplier(decimal);
        self.inner
            .core_state
            .rms_threshold
            .store(threshold, Relaxed);
    }

    pub fn set_input_volume(&self, decibel: f32) {
        let multiplier = db_to_multiplier(decibel);
        self.inner
            .core_state
            .input_volume
            .store(multiplier, Relaxed);
    }

    pub fn set_output_volume(&self, decibel: f32) {
        let multiplier = db_to_multiplier(decibel);
        self.inner
            .core_state
            .output_volume
            .store(multiplier, Relaxed);
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
    pub fn list_devices(
        &self,
    ) -> std::result::Result<(Vec<AudioDevice>, Vec<AudioDevice>), DartError> {
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

    pub async fn set_model(&self, model: Option<Vec<u8>>) -> std::result::Result<(), DartError> {
        let model = if let Some(mode_bytes) = model {
            RnnModel::from_bytes(&mode_bytes).ok_or(String::from("invalid model"))?
        } else {
            RnnModel::default()
        };

        *self.inner.core_state.denoise_model.write().await = model;
        Ok(())
    }
}
