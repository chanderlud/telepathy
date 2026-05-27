//! telepathy-cli protocol (NDJSON over stdio):
//!
//! Input (stdin, one JSON object per line):
//! `{"id":"<request-id>","cmd":"<command>","args":{...}}`
//!
//! Output (stdout, one JSON object per line):
//! - `{"kind":"ack","id":"<request-id>","ok":true}`
//! - `{"kind":"ack","id":"<request-id>","ok":false,"error":"..."}`
//! - `{"kind":"result","id":"<request-id>","data":{...}}`
//! - `{"kind":"event","type":"...","...":...}`
//!
//! All diagnostics are written to stderr so stdout remains machine-parseable.

mod callbacks;
mod commands;
mod events;
mod output;
mod runner;

use anyhow::{Result, anyhow};
use runner::RunOptions;
use serde_json::json;
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("telepathy_cli=info,telepathy_core=info")),
        )
        .init();

    let args = parse_args(std::env::args().collect())?;
    runner::run(args).await
}

fn parse_args(args: Vec<String>) -> Result<RunOptions> {
    let mut relay: Option<String> = std::env::var("TELEPATHY_RELAY_ADDR").ok();
    let mut relay_peer: Option<String> = std::env::var("TELEPATHY_RELAY_PEER").ok();

    let mut idx = 1usize;
    while idx < args.len() {
        match args[idx].as_str() {
            "--relay" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .cloned()
                    .ok_or_else(|| anyhow!("missing value for --relay"))?;
                relay = Some(value);
            }
            "--relay-peer" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .cloned()
                    .ok_or_else(|| anyhow!("missing value for --relay-peer"))?;
                relay_peer = Some(value);
            }
            other => {
                print_startup_error(&format!("unknown argument: {other}"));
                return Err(anyhow!("unknown argument: {other}"));
            }
        }
        idx += 1;
    }

    let relay = match relay {
        Some(value) => value,
        None => {
            print_startup_error("missing relay address: provide --relay or TELEPATHY_RELAY_ADDR");
            return Err(anyhow!("missing relay address"));
        }
    };

    let relay_peer = match relay_peer {
        Some(value) => value,
        None => {
            print_startup_error(
                "missing relay peer id: provide --relay-peer or TELEPATHY_RELAY_PEER",
            );
            return Err(anyhow!("missing relay peer id"));
        }
    };

    Ok(RunOptions { relay, relay_peer })
}

fn print_startup_error(message: &str) {
    println!(
        "{}",
        json!({
            "kind": "event",
            "type": "error",
            "id": null,
            "message": message
        })
    );
}
