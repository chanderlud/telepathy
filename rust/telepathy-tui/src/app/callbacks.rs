//! Construction of the [`NativeCallbacks`] surface required by
//! `telepathy-core`. Each callback marshals data into a [`CoreEvent`] and
//! pushes it through the mpsc channel feeding
//! [`CoreEventPort`](super::port::CoreEventPort).

use std::sync::{Arc, Mutex};

use telepathy_core::native::NativeCallbacks;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::events::CoreEvent;
use crate::state::{AppState, IncomingPromptState};

/// Build the `telepathy-core` callback surface backed by the provided channel
/// and shared application state.
///
/// `handle` is the tokio runtime handle used to spawn the cancel-watcher task
/// associated with each incoming-call prompt.
pub fn build_callbacks(
    sender: mpsc::Sender<CoreEvent>,
    state: Arc<Mutex<AppState>>,
    handle: Handle,
) -> NativeCallbacks {
    let accept_call = {
        let sender = sender.clone();
        let state = state.clone();
        let handle = handle.clone();
        move |request_id: String,
              ringtone: Option<Vec<u8>>,
              response_tx: tokio::sync::oneshot::Sender<bool>,
              cancel_rx: tokio::sync::watch::Receiver<bool>| {
            let mut guard = match state.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };

            if guard.incoming_prompt.is_some() {
                let _ = response_tx.send(false);
                return;
            }

            // Treat the request id as the contact id placeholder until contact
            // resolution lands in T5/T6.
            let contact_id = request_id.clone();

            guard.pending_accept_response = Some(response_tx);
            guard.pending_accept_cancel = Some(cancel_rx.clone());
            guard.incoming_prompt = Some(IncomingPromptState {
                request_id: request_id.clone(),
                contact_id: contact_id.clone(),
            });
            drop(guard);

            let incoming_sender = sender.clone();
            let incoming_request_id = request_id.clone();
            handle.spawn(async move {
                let _ = incoming_sender
                    .send(CoreEvent::IncomingCall {
                        request_id: incoming_request_id,
                        contact_id,
                        ringtone,
                    })
                    .await;
            });

            let cancel_sender = sender.clone();
            let mut cancel_rx = cancel_rx;
            handle.spawn(async move {
                while cancel_rx.changed().await.is_ok() {
                    if *cancel_rx.borrow() {
                        let _ = cancel_sender
                            .send(CoreEvent::IncomingCallCancelled {
                                request_id: request_id.clone(),
                            })
                            .await;
                        break;
                    }
                }
            });
        }
    };

    let get_contact = {
        move |_id: Vec<u8>| -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<telepathy_core::types::Contact>> + Send>,
        > { Box::pin(async move { None }) }
    };

    let call_state = {
        let sender = sender.clone();
        move |state: telepathy_core::types::CallState| {
            let sender = sender.clone();
            let fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
                Box::pin(async move {
                    let _ = sender
                        .send(CoreEvent::CallStateChanged(Arc::new(state)))
                        .await;
                });
            fut
        }
    };

    let session_status = {
        let sender = sender.clone();
        move |(peer, status): (String, telepathy_core::types::SessionStatus)| {
            let sender = sender.clone();
            let fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
                Box::pin(async move {
                    let _ = sender
                        .send(CoreEvent::SessionStatusChanged(peer, Arc::new(status)))
                        .await;
                });
            fut
        }
    };

    let get_contacts = {
        move |_: ()| -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Vec<telepathy_core::types::Contact>> + Send>,
        > { Box::pin(async move { Vec::new() }) }
    };

    let statistics = {
        let sender = sender.clone();
        move |stats: telepathy_core::types::Statistics| {
            let sender = sender.clone();
            let fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
                Box::pin(async move {
                    let _ = sender
                        .send(CoreEvent::StatisticsUpdated(Arc::new(stats)))
                        .await;
                });
            fut
        }
    };

    let message_received = {
        let sender = sender.clone();
        move |message: telepathy_core::types::ChatMessage| {
            let sender = sender.clone();
            let fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
                Box::pin(async move {
                    let peer_id = format_peer(&message);
                    let _ = sender
                        .send(CoreEvent::MessageReceived(peer_id, message.text))
                        .await;
                });
            fut
        }
    };

    let manager_active = {
        let sender = sender.clone();
        move |(active, restartable): (bool, bool)| {
            let sender = sender.clone();
            let fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
                Box::pin(async move {
                    let _ = sender
                        .send(CoreEvent::ManagerActiveChanged(active, restartable))
                        .await;
                });
            fut
        }
    };

    let screenshare_started = move |_: (telepathy_core::types::FrontendNotify, bool)| {
        let fut: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            Box::pin(async move {});
        fut
    };

    NativeCallbacks::new(
        accept_call,
        get_contact,
        call_state,
        session_status,
        get_contacts,
        statistics,
        message_received,
        manager_active,
        screenshare_started,
    )
}

/// Returns the chat peer id used to route incoming messages in the TUI model.
fn format_peer(message: &telepathy_core::types::ChatMessage) -> String {
    message.receiver.to_string()
}
