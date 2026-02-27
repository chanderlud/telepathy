use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use std::{error::Error, path::Path};

use futures::stream::StreamExt;
use libp2p::relay::Config;
use libp2p::{
    SwarmBuilder, autonat,
    core::Multiaddr,
    core::multiaddr::Protocol,
    identify, identity, noise, ping, relay,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use tokio::fs as async_fs;

const KEY_FILE: &str = "local_key.pem";

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    relay: relay::Behaviour,
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    auto_nat: autonat::Behaviour,
}

pub struct RelayInfo {
    pub peer_id: libp2p::PeerId,
}

pub async fn spawn_relay(local: bool) -> Result<RelayInfo, Box<dyn Error>> {
    let local_key = load_or_generate_key().await?;
    let peer_id = local_key.public().to_peer_id();

    let relay_config = Config {
        max_circuit_bytes: u64::MAX,
        max_circuit_duration: Duration::from_secs(u32::MAX as u64),
        reservation_duration: Duration::from_secs(u32::MAX as u64),
        ..Default::default()
    };

    let mut swarm = SwarmBuilder::with_existing_identity(local_key.clone())
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

    let addresses = if local {
        vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]
    } else {
        vec![
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        ]
    };

    for address in addresses {
        let listen_addr_tcp = Multiaddr::from(address).with(Protocol::Tcp(40142));
        let listen_addr_quic = Multiaddr::from(address)
            .with(Protocol::Udp(40142))
            .with(Protocol::QuicV1);

        swarm.listen_on(listen_addr_tcp)?;
        swarm.listen_on(listen_addr_quic)?;
    }

    // spawn the infinite loop in the background so tests & main can continue
    tokio::spawn(async move {
        loop {
            match swarm.next().await {
                Some(SwarmEvent::Behaviour(event)) => {
                    if let BehaviourEvent::Identify(identify::Event::Received {
                        info: identify::Info { observed_addr, .. },
                        ..
                    }) = &event
                    {
                        swarm.add_external_address(observed_addr.clone());
                    }
                }
                Some(SwarmEvent::NewListenAddr { address, .. }) => {
                    println!("Listening on {address:?}");
                }
                Some(event) => {
                    println!("{:?}", event);
                }
                None => break,
            }
        }
    });

    Ok(RelayInfo { peer_id })
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
