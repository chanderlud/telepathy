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
use telepathy_core::native::NativeTelepathy;
use telepathy_core::types::{CodecConfig, Contact, NetworkConfig};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tuirealm::terminal::{CrosstermTerminalAdapter, TerminalBridge};
use tuirealm::{Application, EventListenerCfg, PollStrategy, Sub, SubClause, SubEventClause, Update};

use crate::components::{CoreEventBridgeComponent, PlaceholderComponent, placeholder_ids};
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
    #[error("core error: {0}")]
    Core(String),
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
    let state = Arc::new(Mutex::new(AppState::new(active_profile.clone())));

    let (tx, rx) = mpsc::channel::<CoreEvent>(256);

    let handle = Handle::current();
    let callbacks = build_callbacks(tx, state.clone(), handle.clone());
    let core = init_core_client(&config, &secret_store, &active_profile, callbacks).await?;

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

    app.mount(
        Id::CoreEventBridge,
        Box::new(CoreEventBridgeComponent),
        vec![Sub::new(
            SubEventClause::Discriminant(CoreEvent::LogLine(String::new())),
            SubClause::Always,
        )],
    )
    .map_err(|error| AppError::Application(error.to_string()))?;

    let terminal = TerminalBridge::init(
        CrosstermTerminalAdapter::new()
            .map_err(|error| AppError::Terminal(error.to_string()))?,
    )
    .map_err(|error| AppError::Terminal(error.to_string()))?;

    let mut model = Model::new(app, terminal, state, config, secret_store, handle, core);

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

async fn init_core_client(
    config: &config::AppConfig,
    secret_store: &SecretStore,
    active_profile: &ProfileMeta,
    callbacks: telepathy_core::native::NativeCallbacks,
) -> Result<Arc<NativeTelepathy>, AppError> {
    let preferences = &config.preferences;
    let network_config =
        match NetworkConfig::new(preferences.relay_address.clone(), preferences.relay_id.clone()) {
            Ok(config) => config,
            Err(error) => {
                log::warn!("invalid relay config, falling back to defaults: {error:?}");
                NetworkConfig::default()
            }
        };
    let codec_config = CodecConfig::new(
        preferences.codec_enabled,
        preferences.codec_vbr,
        preferences.codec_residual_bits,
    );

    let mut core = NativeTelepathy::new_default(&network_config, &codec_config, callbacks);
    core.set_output_volume(preferences.output_volume_db);
    core.set_input_volume(preferences.input_volume_db);
    core.set_rms_threshold(preferences.input_sensitivity_db);
    core.set_denoise(preferences.use_denoise);
    core.set_play_custom_ringtones(preferences.play_custom_ringtones);
    core.set_efficiency_mode(preferences.efficiency_mode);
    core.set_input_device(preferences.input_device_id.clone()).await;
    core.set_output_device(preferences.output_device_id.clone()).await;
    core.start_manager().await;

    if !active_profile.id.is_empty() {
        match secret_store.load_keypair(&active_profile.id).await {
            Ok(keypair) => {
                if let Err(error) = core.set_identity(keypair).await {
                    return Err(AppError::Core(format!("set identity failed: {error:?}")));
                }
            }
            Err(StorageError::SecretNotFound) => {
                log::warn!("no profile keypair found for active profile {}", active_profile.id);
            }
            Err(error) => return Err(AppError::Storage(error)),
        }

        for contact in &active_profile.contacts {
            let peer_id = match secret_store
                .load_contact_peer_id(&active_profile.id, &contact.id)
                .await
            {
                Ok(peer_id) => peer_id,
                Err(StorageError::SecretNotFound) => continue,
                Err(error) => {
                    log::error!("failed to load peer id for contact {}: {error}", contact.id);
                    continue;
                }
            };

            let core_contact =
                match Contact::from_parts(contact.id.clone(), contact.nickname.clone(), peer_id) {
                    Ok(contact) => contact,
                    Err(error) => {
                        log::error!("invalid peer id for contact {}: {error:?}", contact.id);
                        continue;
                    }
                };
            core.start_session(&core_contact).await;
        }
    }

    Ok(Arc::new(core))
}
