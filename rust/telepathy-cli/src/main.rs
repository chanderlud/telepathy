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
            EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                EnvFilter::new("telepathy_cli=info,telepathy_core=info,iroh=info")
            }),
        )
        .init();

    let args = parse_args(std::env::args().collect())?;
    runner::run(args).await
}

fn parse_args(args: Vec<String>) -> Result<RunOptions> {
    let mut listen_port = listen_port_from_env()?;
    let mut bind_addresses: Vec<String> = std::env::var("TELEPATHY_BIND_ADDRESSES")
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut relay_url = std::env::var("TELEPATHY_RELAY_URL").ok();
    let mut dns_endpoint = std::env::var("TELEPATHY_DNS_ENDPOINT").ok();
    let mut pkarr_relay = std::env::var("TELEPATHY_PKARR_RELAY").ok();

    let mut idx = 1usize;
    while idx < args.len() {
        match args[idx].as_str() {
            "--listen-port" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .cloned()
                    .ok_or_else(|| startup_failure("missing value for --listen-port"))?;
                listen_port = parse_listen_port(&value, "--listen-port")?;
            }
            "--bind-address" => {
                idx += 1;
                let value = args
                    .get(idx)
                    .cloned()
                    .ok_or_else(|| startup_failure("missing value for --bind-address"))?;
                bind_addresses.push(value);
            }
            "--relay-url" => {
                idx += 1;
                relay_url = Some(
                    args.get(idx)
                        .cloned()
                        .ok_or_else(|| startup_failure("missing value for --relay-url"))?,
                );
            }
            "--dns-endpoint" => {
                idx += 1;
                dns_endpoint = Some(
                    args.get(idx)
                        .cloned()
                        .ok_or_else(|| startup_failure("missing value for --dns-endpoint"))?,
                );
            }
            "--pkarr-relay" => {
                idx += 1;
                pkarr_relay = Some(
                    args.get(idx)
                        .cloned()
                        .ok_or_else(|| startup_failure("missing value for --pkarr-relay"))?,
                );
            }
            other => {
                return Err(startup_failure(format!("unknown argument: {other}")));
            }
        }
        idx += 1;
    }
    if bind_addresses.is_empty() {
        bind_addresses.push("0.0.0.0".to_string());
    }

    Ok(RunOptions {
        listen_port,
        bind_addresses,
        relay_url,
        dns_endpoint,
        pkarr_relay,
    })
}

fn listen_port_from_env() -> Result<u16> {
    match std::env::var("TELEPATHY_LISTEN_PORT") {
        Ok(raw) => parse_listen_port(&raw, "TELEPATHY_LISTEN_PORT"),
        Err(_) => Ok(0),
    }
}

fn parse_listen_port(value: &str, source: &str) -> Result<u16> {
    value
        .parse::<u16>()
        .map_err(|_| startup_failure(format!("invalid value for {source}: {value}")))
}

fn startup_failure(message: impl Into<String>) -> anyhow::Error {
    let message = message.into();
    print_startup_error(&message);
    anyhow!(message)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        _lock: MutexGuard<'static, ()>,
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let lock = ENV_LOCK.lock().expect("env lock poisoned");
            let previous = std::env::var(key).ok();
            // SAFETY: guarded by ENV_LOCK so only one test mutates process env at a time.
            unsafe {
                std::env::set_var(key, value);
            }
            Self {
                _lock: lock,
                key,
                previous,
            }
        }

        fn clear(key: &'static str) -> Self {
            let lock = ENV_LOCK.lock().expect("env lock poisoned");
            let previous = std::env::var(key).ok();
            // SAFETY: guarded by ENV_LOCK so only one test mutates process env at a time.
            unsafe {
                std::env::remove_var(key);
            }
            Self {
                _lock: lock,
                key,
                previous,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: guarded by ENV_LOCK so only one test mutates process env at a time.
            unsafe {
                match &self.previous {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    fn base_args() -> Vec<String> {
        vec!["telepathy-cli".to_string()]
    }

    #[test]
    fn missing_listen_port_flag_value_fails() {
        let _guard = EnvVarGuard::clear("TELEPATHY_LISTEN_PORT");
        let err = parse_args(vec![
            "telepathy-cli".to_string(),
            "--listen-port".to_string(),
        ])
        .expect_err("missing --listen-port value should fail");
        assert!(err.to_string().contains("missing value for --listen-port"));
    }

    #[test]
    fn invalid_listen_port_flag_value_fails() {
        let _guard = EnvVarGuard::clear("TELEPATHY_LISTEN_PORT");
        let err = parse_args(vec![
            "telepathy-cli".to_string(),
            "--listen-port".to_string(),
            "not-a-port".to_string(),
        ])
        .expect_err("invalid --listen-port value should fail");
        assert!(
            err.to_string()
                .contains("invalid value for --listen-port: not-a-port")
        );
    }

    #[test]
    fn invalid_listen_port_env_value_fails() {
        let _guard = EnvVarGuard::set("TELEPATHY_LISTEN_PORT", "not-a-port");
        let err = parse_args(base_args()).expect_err("invalid TELEPATHY_LISTEN_PORT should fail");
        assert!(
            err.to_string()
                .contains("invalid value for TELEPATHY_LISTEN_PORT: not-a-port")
        );
    }

    #[test]
    fn valid_listen_port_sources_apply() {
        let _guard = EnvVarGuard::set("TELEPATHY_LISTEN_PORT", "40142");
        let from_env = parse_args(base_args()).expect("valid env listen port should succeed");
        assert_eq!(from_env.listen_port, 40142);

        let from_flag = parse_args(vec![
            "telepathy-cli".to_string(),
            "--listen-port".to_string(),
            "7777".to_string(),
        ])
        .expect("valid flag listen port should succeed");
        assert_eq!(from_flag.listen_port, 7777);
    }

    #[test]
    fn missing_relay_url_flag_value_fails() {
        let _relay = EnvVarGuard::clear("TELEPATHY_RELAY_URL");
        let err = parse_args(vec!["telepathy-cli".to_string(), "--relay-url".to_string()])
            .expect_err("missing --relay-url value should fail");
        assert!(err.to_string().contains("missing value for --relay-url"));
    }

    #[test]
    fn discovery_flags_and_env_apply_with_flag_precedence() {
        let _relay = EnvVarGuard::set("TELEPATHY_RELAY_URL", "http://10.0.0.1:3340");
        let _dns = EnvVarGuard::set("TELEPATHY_DNS_ENDPOINT", "10.0.0.1:5300");
        let _pkarr = EnvVarGuard::set("TELEPATHY_PKARR_RELAY", "http://10.0.0.1:8080/pkarr");
        let from_env = parse_args(base_args()).expect("discovery env vars should succeed");
        assert_eq!(from_env.relay_url.as_deref(), Some("http://10.0.0.1:3340"));
        assert_eq!(from_env.dns_endpoint.as_deref(), Some("10.0.0.1:5300"));
        assert_eq!(
            from_env.pkarr_relay.as_deref(),
            Some("http://10.0.0.1:8080/pkarr")
        );

        let from_flags = parse_args(vec![
            "telepathy-cli".to_string(),
            "--relay-url".to_string(),
            "http://10.0.10.1:3340".to_string(),
            "--dns-endpoint".to_string(),
            "10.0.10.1:5300".to_string(),
            "--pkarr-relay".to_string(),
            "http://10.0.10.1:8080/pkarr".to_string(),
        ])
        .expect("discovery flags should succeed");
        assert_eq!(
            from_flags.relay_url.as_deref(),
            Some("http://10.0.10.1:3340")
        );
        assert_eq!(from_flags.dns_endpoint.as_deref(), Some("10.0.10.1:5300"));
        assert_eq!(
            from_flags.pkarr_relay.as_deref(),
            Some("http://10.0.10.1:8080/pkarr")
        );
    }
}
