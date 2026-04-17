use crate::flutter::{
    CallState, ChatMessage, Contact, FrontendNotify, FlutterCallbacks, FlutterStatisticsCallback,
    SessionStatus, Statistics, invoke, notify,
};
use crate::internal::callbacks::{CoreCallbacks, CoreStatisticsCallback};
use libp2p::PeerId;
use std::sync::Arc;
use tokio::spawn;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

impl CoreCallbacks<FlutterStatisticsCallback> for FlutterCallbacks {
    fn session_status(
        &self,
        status: SessionStatus,
        peer: PeerId,
    ) -> impl Future<Output = ()> + Send {
        notify(&self.session_status, (peer.to_string(), status))
    }

    fn call_state(&self, status: CallState) -> impl Future<Output = ()> + Send {
        invoke(&self.call_state, status)
    }

    fn get_contacts(&self) -> impl Future<Output = Vec<Contact>> + Send {
        invoke(&self.get_contacts, ())
    }

    fn manager_active(&self, active: bool, restartable: bool) -> impl Future<Output = ()> + Send {
        notify(&self.manager_active, (active, restartable))
    }

    fn screenshare_started(
        &self,
        stop: FrontendNotify,
        sender: bool,
    ) -> impl Future<Output = ()> + Send {
        notify(&self.screenshare_started, (stop, sender))
    }

    fn get_contact(&self, peer_id: Vec<u8>) -> impl Future<Output = Option<Contact>> + Send {
        invoke(&self.get_contact, peer_id)
    }

    fn get_accept_handle(
        &self,
        contact_id: &str,
        ringtone: Option<Vec<u8>>,
        cancel: &Arc<Notify>,
    ) -> JoinHandle<bool> {
        let accept_call = self.accept_call.clone();
        let contact_id = contact_id.to_owned();
        let dart_cancel = FrontendNotify::new(cancel);
        spawn(async move { invoke(&accept_call, (contact_id, ringtone, dart_cancel)).await })
    }

    fn message_received(&self, chat_message: ChatMessage) -> impl Future<Output = ()> + Send {
        invoke(&self.message_received, chat_message)
    }

    fn statistics_callback(&self) -> FlutterStatisticsCallback {
        FlutterStatisticsCallback {
            inner: Arc::clone(&self.statistics),
        }
    }
}

impl CoreStatisticsCallback for FlutterStatisticsCallback {
    fn post(&self, stats: Statistics) -> impl Future<Output = ()> + Send {
        invoke(&self.inner, stats)
    }
}
