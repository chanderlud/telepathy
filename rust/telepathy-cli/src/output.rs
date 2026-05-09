use crate::events::Event;
use serde::Serialize;
use serde_json::Value;
use tokio::io::{self, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutputLine {
    Ack {
        id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Result {
        id: String,
        data: Value,
    },
    Event {
        #[serde(flatten)]
        event: Event,
    },
}

pub fn spawn_writer(mut rx: UnboundedReceiver<OutputLine>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut stdout = io::stdout();
        while let Some(line) = rx.recv().await {
            match serde_json::to_string(&line) {
                Ok(json) => {
                    let _ = stdout.write_all(json.as_bytes()).await;
                    let _ = stdout.write_all(b"\n").await;
                    let _ = stdout.flush().await;
                }
                Err(_) => {
                    let _ = stdout
                        .write_all(
                            br#"{"kind":"event","type":"error","id":null,"message":"failed to serialize output"}"#,
                        )
                        .await;
                    let _ = stdout.write_all(b"\n").await;
                    let _ = stdout.flush().await;
                }
            }
        }
    })
}
