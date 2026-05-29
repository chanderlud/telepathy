use crate::internal::utils::JoinHandle;
use crate::types::{
    CallState, ChatMessage, Contact, FrontendNotify, ManagerState, SessionStatus, Statistics,
};
#[cfg(feature = "integration-testing")]
use async_trait::async_trait;
use iroh::PublicKey;
#[cfg(feature = "integration-testing")]
use mockall::automock;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Notify;

#[cfg_attr(feature = "integration-testing", automock)]
#[cfg_attr(feature = "integration-testing", async_trait)]
pub trait CoreCallbacks<S: CoreStatisticsCallback> {
    fn session_status(
        &self,
        status: SessionStatus,
        peer: PublicKey,
    ) -> impl Future<Output = ()> + Send;

    fn call_state(&self, status: CallState) -> impl Future<Output = ()> + Send;

    fn get_contacts(&self) -> impl Future<Output = Vec<Contact>> + Send;

    fn manager_state(&self, state: ManagerState) -> impl Future<Output = ()> + Send;

    fn screenshare_started(
        &self,
        stop: FrontendNotify,
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

#[cfg_attr(feature = "integration-testing", automock)]
#[cfg_attr(feature = "integration-testing", async_trait)]
pub trait CoreStatisticsCallback {
    fn post(&self, stats: Statistics) -> impl Future<Output = ()> + Send;
}
