//! Bootstrap for the telepathy-tui tuirealm `Application`.
//!
//! `run()` loads persistent config, constructs the shared [`AppState`],
//! wires up the [`NativeCallbacks`] surface used by `telepathy-core`,
//! configures the tuirealm event listener (crossterm input + the
//! [`CoreEventPort`]), mounts placeholder components for each [`Id`]
//! variant, and drives the standard tuirealm tick/redraw loop.

mod callbacks;
pub mod model;
mod port;

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use thiserror::Error;
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tuirealm::terminal::{CrosstermTerminalAdapter, TerminalBridge};
use tuirealm::{Application, EventListenerCfg, PollStrategy, Sub, SubClause, SubEventClause, Update};

use crate::components::{PlaceholderComponent, placeholder_ids};
use crate::events::{CoreEvent, Id, Msg};
use crate::state::AppState;
use crate::storage::config::{self, ProfileMeta};
use crate::storage::{SecretStore, StorageError};

use self::callbacks::build_callbacks;
use self::model::Model;
use self::port::CoreEventPort;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("tuirealm application error: {0}")]
    Application(String),
    #[error("terminal error: {0}")]
    Terminal(String),
}

/// Async entry point used by `main`.
pub async fn run() -> Result<(), AppError> {
    let config = config::load_config()?;
    let secret_store = SecretStore::from_config(&config)?;

    let active_profile = resolve_active_profile(&config);
    let state = Arc::new(Mutex::new(AppState::new(active_profile)));

    let (tx, rx) = mpsc::channel::<CoreEvent>(256);

    let handle = Handle::current();
    let _callbacks = build_callbacks(tx, state.clone(), handle.clone());

    let core_event_port = CoreEventPort::new(rx);

    let event_listener = EventListenerCfg::default()
        .with_handle(handle.clone())
        .async_crossterm_input_listener(Duration::from_millis(10), 3)
        .add_async_port(Box::new(core_event_port), Duration::ZERO, 16);

    let mut app: Application<Id, Msg, CoreEvent> = Application::init(event_listener);

    for id in placeholder_ids().iter().cloned() {
        app.mount(
            id,
            Box::new(PlaceholderComponent::default()),
            vec![Sub::new(SubEventClause::Tick, SubClause::Always)],
        )
        .map_err(|error| AppError::Application(error.to_string()))?;
    }

    let terminal = TerminalBridge::init(
        CrosstermTerminalAdapter::new()
            .map_err(|error| AppError::Terminal(error.to_string()))?,
    )
    .map_err(|error| AppError::Terminal(error.to_string()))?;

    let mut model = Model::new(app, terminal, state, config, secret_store, handle);

    let run_result = run_loop(&mut model);
    let restore_result = model
        .terminal
        .restore()
        .map_err(|error| AppError::Terminal(error.to_string()));

    run_result.and(restore_result)
}

fn run_loop(model: &mut Model<CrosstermTerminalAdapter>) -> Result<(), AppError> {
    while !model.quit {
        match model.app.tick(PollStrategy::Once) {
            Ok(messages) if !messages.is_empty() => {
                model.redraw = true;
                for msg in messages {
                    let mut current = Some(msg);
                    while current.is_some() {
                        current = model.update(current);
                    }
                }
            }
            Ok(_) => {}
            Err(error) => return Err(AppError::Application(error.to_string())),
        }

        if model.redraw {
            model.view();
            model.redraw = false;
        }
    }
    Ok(())
}

fn resolve_active_profile(config: &config::AppConfig) -> ProfileMeta {
    config
        .profiles
        .iter()
        .find(|p| p.id == config.active_profile_id)
        .cloned()
        .unwrap_or_else(placeholder_profile)
}

fn placeholder_profile() -> ProfileMeta {
    ProfileMeta {
        id: String::new(),
        nickname: "(no profile)".to_string(),
        contacts: Vec::new(),
        rooms: Vec::new(),
    }
}
