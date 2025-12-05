use crate::flutter::{
    CallState, ChatMessage, Contact, DartNotify, SessionStatus, Statistics, TelepathyCallbacks,
    TelepathyStatisticsCallback, invoke, notify,
};
use libp2p::PeerId;
use std::sync::Arc;
use tokio::spawn;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

pub(crate) trait Callbacks<S: StatisticsCallback> {
    fn session_status(
        &self,
        status: SessionStatus,
        peer: PeerId,
    ) -> impl Future<Output = ()> + Send;

    fn call_state(&self, status: CallState) -> impl Future<Output = ()> + Send;

    fn get_contacts(&self) -> impl Future<Output = Vec<Contact>> + Send;

    fn manager_active(&self, active: bool, restartable: bool) -> impl Future<Output = ()> + Send;

    fn screenshare_started(
        &self,
        stop: DartNotify,
        sender: bool,
    ) -> impl Future<Output = ()> + Send;

    fn get_contact(&self, peer_id: Vec<u8>) -> impl Future<Output = Option<Contact>> + Send;

    fn get_accept_handle(
        &self,
        contact_id: &str,
        ringtone: Option<Vec<u8>>,
        cancel: &Arc<Notify>,
    ) -> JoinHandle<bool>;

    fn message_received(&self, chat_message: ChatMessage) -> impl Future<Output = ()> + Send;

    fn statistics_callback(&self) -> S;
}

pub(crate) trait StatisticsCallback {
    fn post(&self, stats: Statistics) -> impl Future<Output = ()> + Send;
}

impl Callbacks<TelepathyStatisticsCallback> for TelepathyCallbacks {
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
        stop: DartNotify,
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
        let dart_cancel = DartNotify::new(cancel);
        spawn(async move { invoke(&accept_call, (contact_id, ringtone, dart_cancel)).await })
    }

    fn message_received(&self, chat_message: ChatMessage) -> impl Future<Output = ()> + Send {
        invoke(&self.message_received, chat_message)
    }

    fn statistics_callback(&self) -> TelepathyStatisticsCallback {
        TelepathyStatisticsCallback {
            inner: Arc::clone(&self.statistics),
        }
    }
}

impl StatisticsCallback for TelepathyStatisticsCallback {
    fn post(&self, stats: Statistics) -> impl Future<Output = ()> + Send {
        invoke(&self.inner, stats)
    }
}
