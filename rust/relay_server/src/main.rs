use std::net::Ipv4Addr;
use std::time::Duration;
use std::{error::Error, path::Path};

use futures::stream::StreamExt;
use libp2p::relay::Config;
use libp2p::{
    autonat,
    core::Multiaddr,
    core::multiaddr::Protocol,
    identify, identity, noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use tokio::fs as async_fs;

const KEY_FILE: &str = "local_key.pem";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let local_key = load_or_generate_key().await?;
    println!("relay peer id: {}", local_key.public().to_peer_id());

    let relay_config = Config {
        max_circuit_bytes: u64::MAX,
        max_circuit_duration: Duration::from_secs(u32::MAX as u64),
        reservation_duration: Duration::from_secs(u32::MAX as u64),
        ..Default::default()
    };

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key.clone())
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_behaviour(|key| Behaviour {
            relay: relay::Behaviour::new(key.public().to_peer_id(), relay_config),
            ping: ping::Behaviour::new(ping::Config::new()),
            identify: identify::Behaviour::new(identify::Config::new(
                "/telepathy/0.0.1".to_string(),
                key.public(),
            )),
            auto_nat: autonat::Behaviour::new(
                local_key.public().to_peer_id(),
                autonat::Config {
                    ..Default::default()
                },
            ),
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(30)))
        .build();

    let listen_addr_tcp = Multiaddr::from(Ipv4Addr::UNSPECIFIED).with(Protocol::Tcp(40142));
    swarm.listen_on(listen_addr_tcp)?;

    let listen_addr_quic = Multiaddr::from(Ipv4Addr::UNSPECIFIED)
        .with(Protocol::Udp(40142))
        .with(Protocol::QuicV1);
    swarm.listen_on(listen_addr_quic)?;

    loop {
        match swarm.next().await.expect("Infinite Stream.") {
            SwarmEvent::Behaviour(event) => {
                if let BehaviourEvent::Identify(identify::Event::Received {
                    info: identify::Info { observed_addr, .. },
                    ..
                }) = &event
                {
                    swarm.add_external_address(observed_addr.clone());
                }
                println!("{event:?}")
            }
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("Listening on {address:?}");
            }
            event => println!("{:?}", event),
        }
    }
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay: relay::Behaviour,
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    auto_nat: autonat::Behaviour,
}

async fn load_or_generate_key() -> Result<identity::Keypair, Box<dyn Error>> {
    if Path::new(KEY_FILE).exists() {
        let key_bytes = async_fs::read(KEY_FILE).await?;
        Ok(identity::Keypair::from_protobuf_encoding(&key_bytes)?)
    } else {
        let key = identity::Keypair::generate_ed25519();
        async_fs::write(KEY_FILE, key.to_protobuf_encoding().unwrap()).await?;
        Ok(key)
    }
}
