use std::error::Error;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("relay_server=info,libp2p=warn"));
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(env_filter)
        .init();

    let info = relay_server::spawn_relay(false).await?;

    tracing::info!(peer.id = %info.peer_id, event = "relay_started");

    // keep process alive forever
    futures::future::pending::<()>().await;
    Ok(())
}
