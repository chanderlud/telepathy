use std::sync::Arc;
use libp2p::PeerId;
use tokio::spawn;
use tokio::sync::{oneshot, watch, Notify};
use tokio::task::JoinHandle;
use crate::flutter::{CallState, ChatMessage, Contact, FrontendNotify, SessionStatus, Statistics};
use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use crate::native::{NativeCallbacks, NativeStatisticsCallback};

impl CoreCallbacks<NativeStatisticsCallback> for NativeCallbacks {
    async fn session_status(&self, status: SessionStatus, peer: PeerId) {
        (self.session_status)((peer.to_string(), status)).await
    }

    async fn call_state(&self, status: CallState) {
        (self.call_state)(status).await
    }

    async fn get_contacts(&self) -> Vec<Contact> {
        (self.get_contacts)(()).await
    }

    async fn manager_active(&self, active: bool, restartable: bool) {
        (self.manager_active)((active, restartable)).await
    }

    async fn screenshare_started(&self, stop: FrontendNotify, sender: bool) {
        (self.screenshare_started)((stop, sender)).await
    }

    async fn get_contact(&self, peer_id: Vec<u8>) -> Option<Contact> {
        (self.get_contact)(peer_id).await
    }

    fn get_accept_handle(
        &self,
        contact_id: &str,
        ringtone: Option<Vec<u8>>,
        cancel: &Arc<Notify>,
    ) -> JoinHandle<bool> {
        let accept_call = Arc::clone(&self.accept_call);
        let contact_id = contact_id.to_string();
        let cancel_signal = Arc::clone(cancel);
        spawn(async move {
            let (response_tx, response_rx) = oneshot::channel();
            let (cancel_tx, cancel_rx) = watch::channel(false);

            accept_call(contact_id, ringtone, response_tx, cancel_rx);

            tokio::select! {
                _ = cancel_signal.notified() => {
                    let _ = cancel_tx.send(true);
                    false
                }
                response = response_rx => response.unwrap_or(false),
            }
        })
    }

    async fn message_received(&self, chat_message: ChatMessage) {
        (self.message_received)(chat_message).await
    }

    fn statistics_callback(&self) -> NativeStatisticsCallback {
        NativeStatisticsCallback {
            inner: Arc::clone(&self.statistics),
        }
    }
}

impl CoreStatisticsCallback for NativeStatisticsCallback {
    async fn post(&self, stats: Statistics) {
        (self.inner)(stats).await
    }
}