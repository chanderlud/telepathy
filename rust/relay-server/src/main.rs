use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let info = relay_server::spawn_relay(false).await?;

    println!("relay peer id: {}", info.peer_id);

    // keep process alive forever
    futures::future::pending::<()>().await;
    Ok(())
}
