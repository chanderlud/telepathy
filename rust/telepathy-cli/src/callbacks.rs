use crate::events::Event;
use std::collections::HashMap;
use std::sync::Arc;
use telepathy_core::native::NativeCallbacks;
use telepathy_core::types::Contact;
use tokio::sync::{Mutex, RwLock, oneshot, watch};
use uuid::Uuid;

type PromptSlot = (oneshot::Sender<bool>, watch::Sender<bool>);

#[derive(Clone)]
pub struct Hub {
    pub event_tx: tokio::sync::mpsc::UnboundedSender<Event>,
    pub contacts: Arc<RwLock<HashMap<String, Contact>>>,
    pub pending_prompts: Arc<Mutex<HashMap<String, PromptSlot>>>,
}

impl Hub {
    pub fn new(event_tx: tokio::sync::mpsc::UnboundedSender<Event>) -> Self {
        Self {
            event_tx,
            contacts: Arc::new(RwLock::new(HashMap::new())),
            pending_prompts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn build_callbacks(&self) -> NativeCallbacks {
        let contacts_for_get_contact = Arc::clone(&self.contacts);
        let contacts_for_get_contacts = Arc::clone(&self.contacts);
        let prompts_for_accept = Arc::clone(&self.pending_prompts);
        let tx_for_accept = self.event_tx.clone();
        let tx_for_call_state = self.event_tx.clone();
        let tx_for_session_status = self.event_tx.clone();
        let tx_for_statistics = self.event_tx.clone();
        let tx_for_message = self.event_tx.clone();
        let tx_for_manager = self.event_tx.clone();
        let tx_for_screenshare = self.event_tx.clone();

        NativeCallbacks::new(
            move |contact_id, ringtone, response_tx, mut cancel_rx| {
                let request_id = Uuid::new_v4().to_string();
                let request_id_for_cancel = request_id.clone();
                let prompts = Arc::clone(&prompts_for_accept);
                let tx = tx_for_accept.clone();

                tokio::spawn(async move {
                    let (cancel_tx, _) = watch::channel(false);
                    {
                        let mut guard = prompts.lock().await;
                        guard.insert(request_id.clone(), (response_tx, cancel_tx));
                    }

                    let _ = tx.send(Event::AcceptCallPrompt {
                        request_id,
                        contact_id,
                        has_ringtone: ringtone.is_some(),
                    });

                    loop {
                        if cancel_rx.changed().await.is_err() {
                            break;
                        }
                        if *cancel_rx.borrow() {
                            let removed = {
                                let mut guard = prompts.lock().await;
                                guard.remove(&request_id_for_cancel)
                            };
                            if removed.is_some() {
                                let _ = tx.send(Event::AcceptCallCanceled {
                                    request_id: request_id_for_cancel.clone(),
                                });
                            }
                            break;
                        }
                    }
                });
            },
            move |peer_id| {
                let contacts = Arc::clone(&contacts_for_get_contact);
                Box::pin(async move {
                    let peer = peer_id;
                    let guard = contacts.read().await;
                    guard.values().find(|c| c.id_eq(peer.clone())).cloned()
                })
            },
            move |state| {
                let tx = tx_for_call_state.clone();
                Box::pin(async move {
                    let _ = tx.send(Event::CallState { state });
                })
            },
            move |(peer, status)| {
                let tx = tx_for_session_status.clone();
                Box::pin(async move {
                    let _ = tx.send(Event::SessionStatus { peer, status });
                })
            },
            move |_| {
                let contacts = Arc::clone(&contacts_for_get_contacts);
                Box::pin(async move {
                    let guard = contacts.read().await;
                    guard.values().cloned().collect()
                })
            },
            move |stats| {
                let tx = tx_for_statistics.clone();
                Box::pin(async move {
                    let _ = tx.send(Event::from(stats));
                })
            },
            move |message| {
                let tx = tx_for_message.clone();
                Box::pin(async move {
                    let _ = tx.send(Event::from(message));
                })
            },
            move |(active, restartable)| {
                let tx = tx_for_manager.clone();
                Box::pin(async move {
                    let _ = tx.send(Event::ManagerActive {
                        active,
                        restartable,
                    });
                })
            },
            move |(_notify, sender)| {
                let tx = tx_for_screenshare.clone();
                Box::pin(async move {
                    let _ = tx.send(Event::ScreenshareStarted { sender });
                })
            },
        )
    }
}
