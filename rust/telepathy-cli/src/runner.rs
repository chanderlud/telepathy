use crate::callbacks::Hub;
use crate::commands::{Command, Envelope};
use crate::events::Event;
use crate::output::{OutputLine, spawn_writer};
use anyhow::{Context, Result};
use base64::Engine;
use serde_json::json;
use telepathy_core::native::NativeTelepathy;
use telepathy_core::types::{CodecConfig, Contact, NetworkConfig};
use tokio::io::{AsyncBufReadExt, BufReader};

const MAX_PARSE_LINE_LEN: usize = 240;

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub relay: String,
    pub relay_peer: String,
}

pub async fn run(opts: RunOptions) -> Result<()> {
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let (output_tx, output_rx) = tokio::sync::mpsc::unbounded_channel();
    let writer = spawn_writer(output_rx);
    let hub = Hub::new(event_tx);
    let callbacks = hub.build_callbacks();

    let network_config = match NetworkConfig::new(opts.relay, opts.relay_peer) {
        Ok(config) => config,
        Err(err) => {
            let message = err.message;
            send_event(
                &output_tx,
                Event::Error {
                    id: None,
                    message: message.clone(),
                },
            );
            drop(output_tx);
            let _ = writer.await;
            return Err(anyhow::anyhow!(message)).context("failed to build network config");
        }
    };
    let codec_config = CodecConfig::new(true, true, 5.0);
    let mut telepathy = NativeTelepathy::new(&network_config, &codec_config, callbacks);

    send_event(
        &output_tx,
        Event::Ready {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    );

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut should_exit = false;

    while !should_exit {
        tokio::select! {
            maybe_event = event_rx.recv() => {
                if let Some(event) = maybe_event {
                    send_event(&output_tx, event);
                }
            }
            sig = tokio::signal::ctrl_c() => {
                if sig.is_ok() {
                    telepathy.shutdown().await;
                }
                should_exit = true;
            }
            line_result = lines.next_line() => {
                match line_result {
                    Ok(Some(line)) => {
                        match serde_json::from_str::<Envelope>(&line) {
                            Ok(envelope) => {
                                let id = envelope.id.clone();
                                match handle_command(&mut telepathy, &hub, envelope).await {
                                    CommandOutcome::AckOk => send_ack_ok(&output_tx, id),
                                    CommandOutcome::AckErr(message) => send_ack_err(&output_tx, id, message),
                                    CommandOutcome::Result(data) => send_result(&output_tx, id, data),
                                    CommandOutcome::Shutdown => {
                                        telepathy.shutdown().await;
                                        send_ack_ok(&output_tx, id);
                                        should_exit = true;
                                    }
                                }
                            }
                            Err(err) => {
                                send_event(
                                    &output_tx,
                                    Event::Error {
                                        id: None,
                                        message: format!(
                                            "invalid command JSON: {}; line={}",
                                            err,
                                            truncate_line(&line)
                                        ),
                                    },
                                );
                            }
                        }
                    }
                    Ok(None) => {
                        telepathy.shutdown().await;
                        should_exit = true;
                    }
                    Err(err) => {
                        send_event(
                            &output_tx,
                            Event::Error {
                                id: None,
                                message: format!("stdin read error: {err}"),
                            },
                        );
                        telepathy.shutdown().await;
                        should_exit = true;
                    }
                }
            }
        }
    }

    while let Ok(event) = event_rx.try_recv() {
        send_event(&output_tx, event);
    }

    drop(telepathy);
    drop(hub);
    drop(output_tx);
    let _ = writer.await;
    Ok(())
}

enum CommandOutcome {
    AckOk,
    AckErr(String),
    Result(serde_json::Value),
    Shutdown,
}

async fn handle_command(
    telepathy: &mut NativeTelepathy,
    hub: &Hub,
    envelope: Envelope,
) -> CommandOutcome {
    match envelope.cmd {
        Command::SetIdentity { key_b64 } => {
            let decoded = match base64::engine::general_purpose::STANDARD.decode(key_b64) {
                Ok(value) => value,
                Err(err) => {
                    return CommandOutcome::AckErr(format!("invalid base64 identity: {err}"));
                }
            };

            match telepathy.set_identity(decoded).await {
                Ok(()) => CommandOutcome::AckOk,
                Err(err) => CommandOutcome::AckErr(err.message),
            }
        }
        Command::AddContact {
            id,
            nickname,
            peer_id,
        } => match Contact::from_parts(id.clone(), nickname, peer_id) {
            Ok(contact) => {
                hub.contacts.write().await.insert(id, contact);
                CommandOutcome::AckOk
            }
            Err(err) => CommandOutcome::AckErr(err.message),
        },
        Command::RemoveContact { id } => {
            hub.contacts.write().await.remove(&id);
            CommandOutcome::AckOk
        }
        Command::StartManager => {
            telepathy.start_manager().await;
            CommandOutcome::AckOk
        }
        Command::RestartManager => match telepathy.restart_manager().await {
            Ok(()) => CommandOutcome::AckOk,
            Err(err) => CommandOutcome::AckErr(err.message),
        },
        Command::Shutdown => CommandOutcome::Shutdown,
        Command::StartSession { contact_id } => match contact_by_id(hub, &contact_id).await {
            Ok(contact) => {
                telepathy.start_session(&contact).await;
                CommandOutcome::AckOk
            }
            Err(err) => CommandOutcome::AckErr(err),
        },
        Command::StopSession { contact_id } => match contact_by_id(hub, &contact_id).await {
            Ok(contact) => {
                telepathy.stop_session(&contact).await;
                CommandOutcome::AckOk
            }
            Err(err) => CommandOutcome::AckErr(err),
        },
        Command::StartCall { contact_id } => match contact_by_id(hub, &contact_id).await {
            Ok(contact) => match telepathy.start_call(&contact).await {
                Ok(()) => CommandOutcome::AckOk,
                Err(err) => CommandOutcome::AckErr(err.message),
            },
            Err(err) => CommandOutcome::AckErr(err),
        },
        Command::EndCall => {
            telepathy.end_call().await;
            CommandOutcome::AckOk
        }
        Command::AcceptCall { request_id, accept } => {
            let slot = { hub.pending_prompts.lock().await.remove(&request_id) };
            match slot {
                Some((response_tx, cancel_tx)) => {
                    let _ = cancel_tx.send(false);
                    match response_tx.send(accept) {
                        Ok(()) => CommandOutcome::AckOk,
                        Err(_) => {
                            CommandOutcome::AckErr("accept_call prompt already closed".to_string())
                        }
                    }
                }
                None => {
                    CommandOutcome::AckErr(format!("unknown accept_call request_id: {request_id}"))
                }
            }
        }
        Command::JoinRoom { members } => match telepathy.join_room(members).await {
            Ok(()) => CommandOutcome::AckOk,
            Err(err) => CommandOutcome::AckErr(err.message),
        },
        Command::SendChat {
            contact_id,
            text,
            attachments,
        } => match contact_by_id(hub, &contact_id).await {
            Ok(contact) => {
                let mut decoded = Vec::with_capacity(attachments.len());
                for attachment in attachments {
                    match base64::engine::general_purpose::STANDARD.decode(attachment.data_b64) {
                        Ok(data) => decoded.push((attachment.name, data)),
                        Err(err) => {
                            return CommandOutcome::AckErr(format!(
                                "invalid base64 attachment payload: {err}"
                            ));
                        }
                    }
                }

                let mut message = telepathy.build_chat(&contact, text, decoded);
                match telepathy.send_chat(&mut message).await {
                    Ok(()) => CommandOutcome::AckOk,
                    Err(err) => CommandOutcome::AckErr(err.message),
                }
            }
            Err(err) => CommandOutcome::AckErr(err),
        },
        Command::AudioTest => match telepathy.audio_test().await {
            Ok(()) => CommandOutcome::AckOk,
            Err(err) => CommandOutcome::AckErr(err.message),
        },
        Command::SetMuted { value } => {
            telepathy.set_muted(value);
            CommandOutcome::AckOk
        }
        Command::SetDeafened { value } => {
            telepathy.set_deafened(value);
            CommandOutcome::AckOk
        }
        Command::SetInputVolumeDb { value } => {
            telepathy.set_input_volume(value);
            CommandOutcome::AckOk
        }
        Command::SetOutputVolumeDb { value } => {
            telepathy.set_output_volume(value);
            CommandOutcome::AckOk
        }
        Command::SetRmsThresholdDb { value } => {
            telepathy.set_rms_threshold(value);
            CommandOutcome::AckOk
        }
        Command::SetDenoise { value } => {
            telepathy.set_denoise(value);
            CommandOutcome::AckOk
        }
        Command::SetEfficiencyMode { value } => {
            telepathy.set_efficiency_mode(value);
            CommandOutcome::AckOk
        }
        Command::SetPlayCustomRingtones { value } => {
            telepathy.set_play_custom_ringtones(value);
            CommandOutcome::AckOk
        }
        Command::SetInputDevice { id } => {
            telepathy.set_input_device(id).await;
            CommandOutcome::AckOk
        }
        Command::SetOutputDevice { id } => {
            telepathy.set_output_device(id).await;
            CommandOutcome::AckOk
        }
        Command::ListDevices => CommandOutcome::Result(json!({
            "supported": false,
            "reason": "NativeTelepathy does not currently expose list device APIs"
        })),
    }
}

async fn contact_by_id(hub: &Hub, contact_id: &str) -> std::result::Result<Contact, String> {
    let guard = hub.contacts.read().await;
    guard
        .get(contact_id)
        .cloned()
        .ok_or_else(|| format!("unknown contact_id: {contact_id}"))
}

fn send_ack_ok(tx: &tokio::sync::mpsc::UnboundedSender<OutputLine>, id: String) {
    let _ = tx.send(OutputLine::Ack {
        id,
        ok: true,
        error: None,
    });
}

fn send_ack_err(tx: &tokio::sync::mpsc::UnboundedSender<OutputLine>, id: String, error: String) {
    let _ = tx.send(OutputLine::Ack {
        id,
        ok: false,
        error: Some(error),
    });
}

fn send_result(
    tx: &tokio::sync::mpsc::UnboundedSender<OutputLine>,
    id: String,
    data: serde_json::Value,
) {
    let _ = tx.send(OutputLine::Result { id, data });
}

fn send_event(tx: &tokio::sync::mpsc::UnboundedSender<OutputLine>, event: Event) {
    let _ = tx.send(OutputLine::Event { event });
}

fn truncate_line(line: &str) -> String {
    if line.chars().count() <= MAX_PARSE_LINE_LEN {
        return line.to_string();
    }
    let truncated: String = line.chars().take(MAX_PARSE_LINE_LEN).collect();
    format!("{truncated}...")
}
