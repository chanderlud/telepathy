//! Async tuirealm port that drains [`CoreEvent`]s produced by
//! `telepathy-core` callbacks and surfaces them to the event loop as
//! `Event::User(CoreEvent)`.

use std::time::Duration;

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tuirealm::Event;
use tuirealm::listener::{ListenerError, ListenerResult, PollAsync};

use crate::events::CoreEvent;

/// Bridges a tokio mpsc channel into a tuirealm async port.
pub struct CoreEventPort {
    receiver: mpsc::Receiver<CoreEvent>,
}

impl CoreEventPort {
    pub fn new(receiver: mpsc::Receiver<CoreEvent>) -> Self {
        Self { receiver }
    }
}

#[tuirealm::async_trait]
impl PollAsync<CoreEvent> for CoreEventPort {
    async fn poll(&mut self) -> ListenerResult<Option<Event<CoreEvent>>> {
        match self.receiver.try_recv() {
            Ok(event) => Ok(Some(Event::User(event))),
            Err(TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(1)).await;
                Ok(None)
            }
            Err(TryRecvError::Disconnected) => Err(ListenerError::ListenerDied),
        }
    }
}
