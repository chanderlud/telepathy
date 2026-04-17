mod callbacks;

use crate::types::{CallState, ChatMessage, Contact, FrontendNotify, SessionStatus, Statistics};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::{oneshot, watch};

type NativeFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
type NativeVoid<A> = Arc<dyn Fn(A) -> NativeFuture<()> + Send + Sync + 'static>;
type NativeMethod<A, R> = Arc<dyn Fn(A) -> NativeFuture<R> + Send + Sync + 'static>;
type NativeAcceptCall = Arc<
    dyn Fn(String, Option<Vec<u8>>, oneshot::Sender<bool>, watch::Receiver<bool>) + Send + Sync,
>;

/// Statistics callback adapter for non-FRB clients.
#[derive(Clone)]
pub struct NativeStatisticsCallback {
    inner: NativeVoid<Statistics>,
}

/// Rust-native callback surface for `telepathy-core`.
///
/// This mirrors `FlutterCallbacks` but replaces FRB function wrappers with plain
/// Rust closures/futures so native consumers (like `telepathy-tui`) can depend on
/// `telepathy-core` without FRB runtime semantics.
pub struct NativeCallbacks {
    /// Prompts the user to accept a call.
    ///
    /// - `response_tx`: send `true` to accept or `false` to reject
    /// - `cancel_rx`: core toggles this to `true` to dismiss the pending prompt
    accept_call: NativeAcceptCall,
    get_contact: NativeMethod<Vec<u8>, Option<Contact>>,
    call_state: NativeVoid<CallState>,
    session_status: NativeVoid<(String, SessionStatus)>,
    get_contacts: NativeMethod<(), Vec<Contact>>,
    statistics: NativeVoid<Statistics>,
    message_received: NativeVoid<ChatMessage>,
    manager_active: NativeVoid<(bool, bool)>,
    screenshare_started: NativeVoid<(FrontendNotify, bool)>,
}

impl NativeCallbacks {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        accept_call: impl Fn(String, Option<Vec<u8>>, oneshot::Sender<bool>, watch::Receiver<bool>)
        + Send
        + Sync
        + 'static,
        get_contact: impl Fn(Vec<u8>) -> NativeFuture<Option<Contact>> + Send + Sync + 'static,
        call_state: impl Fn(CallState) -> NativeFuture<()> + Send + Sync + 'static,
        session_status: impl Fn((String, SessionStatus)) -> NativeFuture<()> + Send + Sync + 'static,
        get_contacts: impl Fn(()) -> NativeFuture<Vec<Contact>> + Send + Sync + 'static,
        statistics: impl Fn(Statistics) -> NativeFuture<()> + Send + Sync + 'static,
        message_received: impl Fn(ChatMessage) -> NativeFuture<()> + Send + Sync + 'static,
        manager_active: impl Fn((bool, bool)) -> NativeFuture<()> + Send + Sync + 'static,
        screenshare_started: impl Fn((FrontendNotify, bool)) -> NativeFuture<()> + Send + Sync + 'static,
    ) -> Self {
        Self {
            accept_call: Arc::new(accept_call),
            get_contact: Arc::new(get_contact),
            call_state: Arc::new(call_state),
            session_status: Arc::new(session_status),
            get_contacts: Arc::new(get_contacts),
            statistics: Arc::new(statistics),
            message_received: Arc::new(message_received),
            manager_active: Arc::new(manager_active),
            screenshare_started: Arc::new(screenshare_started),
        }
    }
}
