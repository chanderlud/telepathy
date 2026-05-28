---
name: rust-libp2p
description: Use this skill when the user asks about rust-libp2p, Rust libp2p, libp2p::SwarmBuilder, NetworkBehaviour, ConnectionHandler, Transport, PeerId, Multiaddr, Kademlia DHT, Gossipsub, request-response, Identify, Ping, mDNS, Rendezvous, Circuit Relay v2, AutoNAT, DCUtR hole punching, UPnP, QUIC, TCP, WebSocket, WebRTC/WebTransport on WASM, Noise, TLS, Yamux, or how to build peer-to-peer networking applications in Rust.
---

# rust-libp2p

## Scope

This skill is specifically for the Rust implementation of libp2p, not js-libp2p or go-libp2p.

Use the general libp2p skill for protocol concepts like peer IDs, multiaddrs, relays, and DHTs. Use this skill for Rust crate names, feature flags, APIs, module layout, code patterns, and implementation-specific gotchas.

When answering, prefer Rust API names and examples:

- `libp2p::SwarmBuilder`, not `createLibp2p`.
- `#[derive(NetworkBehaviour)]` from `libp2p::swarm`, not JavaScript service configuration objects.
- `Behaviour`, `Config`, and `Event` types from protocol crates.
- `SwarmEvent` loops, not callback/event-emitter style code.
- Cargo feature flags, because the root crate compiles almost nothing unless the right features are enabled. Humanity invented optional compilation and then made everyone debug missing imports. Naturally.

## Version and freshness notes

rust-libp2p evolves quickly. Before giving exact API answers, check the current crate/source when possible.

Known source baseline used for this skill:

- Current published docs on docs.rs show `libp2p` `0.56.0`.
- GitHub `master` metadata has moved to `libp2p` `0.56.1`.
- The workspace MSRV is Rust `1.83.0`, edition `2021`.
- The root `libp2p` crate has no default features in `0.56.0`; users must enable features explicitly.
- DeepWiki is useful for architecture, but source and rustdoc win when they disagree.

Use `version = "0.56"` in examples unless the user asks for an exact pinned version. For production answers, say “verify against the current `libp2p/Cargo.toml` and docs.rs.”

## Mental model

A rust-libp2p node is built from these layers:

1. **Identity**: `libp2p_identity::Keypair` and `PeerId`.
2. **Transport**: how bytes move, such as TCP, QUIC, WebSocket, UDS, memory, or WASM transports.
3. **Security upgrade**: usually Noise or TLS for TCP/WebSocket. QUIC already includes TLS 1.3.
4. **Stream multiplexer**: usually Yamux for TCP/WebSocket. QUIC, WebTransport, and WebRTC have native multiplexing.
5. **Protocol behaviour**: `NetworkBehaviour` implementations such as `ping::Behaviour`, `kad::Behaviour`, `gossipsub::Behaviour`, `request_response::Behaviour`, `identify::Behaviour`, `relay::client::Behaviour`.
6. **Swarm**: the runtime orchestrator that drives transports, connection pools, behaviours, handlers, and events.

Rust-libp2p separates “how to send bytes” from “what bytes to send”:

- `Transport` handles dialing, listening, and producing connections.
- `NetworkBehaviour` decides protocol actions, emits commands to the swarm, and produces application-visible events.
- `ConnectionHandler` owns per-connection state and substream negotiation for a behaviour.
- `Swarm<B>` ties the transport to a root behaviour `B`. It must be polled as a stream to make progress.

## Root crate and features

The root crate is `libp2p`. It re-exports many implementation crates behind Cargo features.

A minimal native Ping example needs:

```toml
[dependencies]
libp2p = { version = "0.56", features = ["noise", "ping", "tcp", "tokio", "yamux"] }
futures = "0.3"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

Common feature flags:

| Feature | Enables |
|---|---|
| `tokio` | Tokio provider for `SwarmBuilder` and supported transports |
| `tcp` | `libp2p::tcp` native TCP |
| `dns` | DNS address resolution wrapper |
| `quic` | native QUIC transport |
| `websocket` | native WebSocket transport |
| `websocket-websys` | browser/WASM WebSocket transport |
| `webrtc-websys` | browser/WASM WebRTC transport |
| `webtransport-websys` | browser/WASM WebTransport transport |
| `noise` | Noise security upgrade |
| `tls` | libp2p TLS security upgrade |
| `yamux` | Yamux stream muxer |
| `macros` | `#[derive(NetworkBehaviour)]` |
| `ping` | `/ipfs/ping/1.0.0` behaviour |
| `identify` | Identify behaviour |
| `kad` | Kademlia DHT behaviour |
| `gossipsub` | Gossipsub pub/sub behaviour |
| `request-response` | Generic request/response framework |
| `cbor`, `json` | Predefined request-response codecs |
| `mdns` | LAN mDNS discovery |
| `relay` | Circuit Relay v2 client/server support |
| `autonat` | NAT reachability detection |
| `dcutr` | Direct Connection Upgrade through Relay, hole punching |
| `rendezvous` | Rendezvous peer registration/discovery |
| `upnp` | UPnP port mapping |
| `metrics` | OpenMetrics/Prometheus metrics support |
| `pnet` | private network pre-shared-key protection |
| `plaintext` | plaintext transport upgrade; avoid except tests/dev |
| `rsa`, `ed25519`, `ecdsa`, `secp256k1` | identity key algorithm support |
| `serde` | serde integration for selected types |

The `full` feature exists, but do not recommend it blindly. It increases build time and target incompatibility risk. Use feature-specific examples unless the user explicitly wants “everything.”

## Workspace and implementation crates

The repository is a monorepo with core, transports, protocols, muxers, examples, and utilities.

Important crates and paths:

| Crate | Path | Purpose |
|---|---|---|
| `libp2p` | `libp2p/` | Root convenience crate and `SwarmBuilder` |
| `libp2p-core` | `core/` | Core traits and types: `Transport`, upgrades, muxing, multiaddr glue |
| `libp2p-identity` | `identity/` | Keypairs, public keys, `PeerId` |
| `libp2p-swarm` | `swarm/` | `Swarm`, `NetworkBehaviour`, `ConnectionHandler`, events |
| `libp2p-swarm-derive` | `swarm-derive/` | `#[derive(NetworkBehaviour)]` macro |
| `libp2p-yamux` | `muxers/yamux/` | Recommended stream multiplexer for TCP/WebSocket |
| `libp2p-mplex` | `muxers/mplex/` | Legacy muxer; avoid for new work |
| `libp2p-tcp` | `transports/tcp/` | Native TCP transport |
| `libp2p-quic` | `transports/quic/` | QUIC transport with built-in TLS and multiplexing |
| `libp2p-websocket` | `transports/websocket/` | Native WebSocket transport |
| `libp2p-dns` | `transports/dns/` | DNS resolution wrapper |
| `libp2p-noise` | `transports/noise/` | Noise security upgrade |
| `libp2p-tls` | `transports/tls/` | TLS security upgrade |
| `libp2p-kad` | `protocols/kad/` | Kademlia DHT |
| `libp2p-gossipsub` | `protocols/gossipsub/` | Mesh gossip pub/sub |
| `libp2p-request-response` | `protocols/request-response/` | Typed request/response |
| `libp2p-identify` | `protocols/identify/` | Exchange peer metadata, protocols, addresses |
| `libp2p-relay` | `protocols/relay/` | Circuit Relay v2 |
| `libp2p-autonat` | `protocols/autonat/` | NAT reachability detection |
| `libp2p-dcutr` | `protocols/dcutr/` | Hole punching via relayed connection upgrade |
| `libp2p-mdns` | `protocols/mdns/` | LAN discovery |
| `libp2p-rendezvous` | `protocols/rendezvous/` | Rendezvous registration/discovery |
| `libp2p-upnp` | `protocols/upnp/` | Router port mapping |
| `libp2p-stream` | `protocols/stream/` | Raw stream opening with protocol negotiation; check current crate status before recommending |

## SwarmBuilder API

Prefer `SwarmBuilder` for new examples. It is the high-level builder exported from `libp2p`.

Canonical native chain:

```rust
let mut swarm = libp2p::SwarmBuilder::with_new_identity()
    .with_tokio()
    .with_tcp(
        libp2p::tcp::Config::default(),
        libp2p::noise::Config::new,
        libp2p::yamux::Config::default,
    )?
    .with_behaviour(|key| {
        let peer_id = key.public().to_peer_id();
        libp2p::ping::Behaviour::default()
    })?
    .build();
```

Important builder methods:

| Method | Purpose |
|---|---|
| `SwarmBuilder::with_new_identity()` | Generate a fresh identity |
| `SwarmBuilder::with_existing_identity(keypair)` | Use a provided `libp2p::identity::Keypair` |
| `.with_tokio()` | Select Tokio runtime provider |
| `.with_tcp(tcp_config, security_upgrade, multiplexer_upgrade)` | Add native TCP plus security and mux upgrades |
| `.with_quic()` / `.with_quic_config(...)` | Add native QUIC |
| `.with_dns()` / `.with_dns_config(...)` | Add DNS resolution wrapper |
| `.with_websocket(security_upgrade, multiplexer_upgrade)` | Add native WebSocket over the existing transport stack |
| `.with_other_transport(|key| ...)` | Add custom transport |
| `.with_relay_client(security_upgrade, multiplexer_upgrade)` | Add relay client transport/behaviour support |
| `.with_bandwidth_metrics(&mut registry)` | Wrap transport with bandwidth metrics |
| `.with_behaviour(|key| behaviour)` | Add the root `NetworkBehaviour` |
| `.with_swarm_config(|cfg| cfg.with_idle_connection_timeout(...))` | Customize swarm config |
| `.build()` | Build `Swarm<B>` |

`with_tcp`, `with_websocket`, and `with_relay_client` accept function items or tuples of function items for upgrades:

```rust
.with_tcp(
    tcp::Config::default(),
    (tls::Config::new, noise::Config::new),
    yamux::Config::default,
)?
```

Use this tuple form when you want fallback negotiation between security upgrades. The same pattern applies to multiplexer upgrades, though Yamux alone is the recommended default for new TCP/WebSocket stacks.

When `.with_relay_client(...)` is used, the `.with_behaviour(...)` closure receives `(key, relay_behaviour)` and the relay behaviour must be included in the root behaviour:

```rust
#[derive(libp2p::swarm::NetworkBehaviour)]
struct Behaviour {
    relay: libp2p::relay::client::Behaviour,
    ping: libp2p::ping::Behaviour,
}

let swarm = libp2p::SwarmBuilder::with_new_identity()
    .with_tokio()
    .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
    .with_relay_client(noise::Config::new, yamux::Config::default)?
    .with_behaviour(|_key, relay| Behaviour {
        relay,
        ping: ping::Behaviour::default(),
    })?
    .build();
```

Without relay client, the behaviour closure receives only `&Keypair`.

## Swarm API

`Swarm<B>` contains network state and a behaviour. It must be continuously polled as a `Stream`.

Useful methods:

| Method | Use |
|---|---|
| `listen_on(Multiaddr)` | Start listening |
| `dial(peer_id_or_multiaddr_or_DialOpts)` | Dial a known peer or raw multiaddr |
| `local_peer_id()` | Access local peer ID |
| `listeners()` | Current listen addresses |
| `external_addresses()` | Confirmed externally reachable addresses |
| `add_external_address(addr)` | Announce an address known to be reachable |
| `add_peer_address(peer, addr)` | Tell behaviours about a peer address |
| `behaviour()` / `behaviour_mut()` | Access protocol behaviour |
| `connected_peers()` | Iterate connected peer IDs |
| `is_connected(&peer_id)` | Check peer connectivity |
| `disconnect_peer_id(peer_id)` | Close all connections to peer |
| `close_connection(connection_id)` | Gracefully close a specific connection |

Event loop skeleton:

```rust
use futures::StreamExt;
use libp2p::swarm::SwarmEvent;

loop {
    match swarm.select_next_some().await {
        SwarmEvent::NewListenAddr { address, .. } => {
            println!("Listening on {address}");
        }
        SwarmEvent::Behaviour(event) => {
            println!("Behaviour event: {event:?}");
        }
        SwarmEvent::ConnectionEstablished { peer_id, .. } => {
            println!("Connected to {peer_id}");
        }
        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
            println!("Connection to {peer_id} closed: {cause:?}");
        }
        _ => {}
    }
}
```

If the swarm is not polled, nothing progresses. Not dialing, not listening, not protocol negotiation. A perfect machine, doing absolutely nothing, as requested by an unpolled future.

## NetworkBehaviour

Every protocol crate exposes a `Behaviour` that implements `NetworkBehaviour`. Compose several behaviours with the derive macro:

```rust
use libp2p::{
    gossipsub, identify, kad, mdns, ping,
    swarm::NetworkBehaviour,
};

#[derive(NetworkBehaviour)]
struct Behaviour {
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    kad: kad::Behaviour<kad::store::MemoryStore>,
    gossipsub: gossipsub::Behaviour,
    mdns: mdns::tokio::Behaviour,
}
```

Enable the `macros` feature for the derive macro:

```toml
libp2p = { version = "0.56", features = ["macros", "..."] }
```

Manual `NetworkBehaviour` implementations are advanced. They must provide:

- Associated type `ConnectionHandler`.
- Associated type `ToSwarm`.
- `handle_established_inbound_connection`.
- `handle_established_outbound_connection`.
- `on_swarm_event`.
- `on_connection_handler_event`.
- `poll`.

Use manual implementations only when writing a custom protocol crate or advanced behaviour. For applications, compose existing behaviours with the derive macro.

## ConnectionHandler

`ConnectionHandler` is per-connection protocol state. Most application authors do not implement it directly.

Use it when implementing a custom protocol that needs to:

- Negotiate inbound or outbound substreams.
- Run protocol-specific state for each connection.
- Receive commands from a behaviour.
- Emit handler events back to the behaviour.

Most built-in protocols follow this pattern:

- `Behaviour`: global protocol state, query state, peer tables, application events.
- `Handler`: per-connection substream negotiation and I/O.
- `Config`: tunable settings.
- `Event`: application-facing events.

## Identity and PeerId

Common identity patterns:

```rust
let keypair = libp2p::identity::Keypair::generate_ed25519();
let peer_id = keypair.public().to_peer_id();
```

Use `SwarmBuilder::with_new_identity()` for throwaway/test nodes and `SwarmBuilder::with_existing_identity(keypair)` when stable peer identity matters.

Enable key features as needed:

- `ed25519`: common default for new keys.
- `rsa`: useful for IPFS interoperability because many IPFS peers still use RSA identities.
- `ecdsa`, `secp256k1`: for networks that standardize on those key types.

## Multiaddr

Rust examples usually parse a multiaddr from a string:

```rust
let addr: libp2p::Multiaddr = "/ip4/0.0.0.0/tcp/0".parse()?;
swarm.listen_on(addr)?;
```

Common native listen addresses:

```text
/ip4/0.0.0.0/tcp/0
/ip6/::/tcp/0
/ip4/0.0.0.0/udp/0/quic-v1
```

Dialing usually requires the remote `PeerId` to appear in the multiaddr or to be known through peer store/behaviour state:

```text
/ip4/203.0.113.10/tcp/4001/p2p/<peer-id>
/dnsaddr/bootstrap.libp2p.io/p2p/<peer-id>
```

Relayed address shape:

```text
/ip4/<relay-ip>/tcp/<relay-port>/p2p/<relay-peer-id>/p2p-circuit/p2p/<dest-peer-id>
```

## Transport choices

| Transport | Feature/crate | Target | Security/mux notes | Use when |
|---|---|---|---|---|
| TCP | `tcp` / `libp2p-tcp` | native | Needs Noise/TLS + Yamux | Default native transport |
| QUIC | `quic` / `libp2p-quic` | native | Built-in TLS 1.3 + multiplexing | Low-latency native transport |
| DNS | `dns` / `libp2p-dns` | native | Wrapper, not a transport by itself | Dial `/dns`, `/dns4`, `/dns6`, `/dnsaddr` addresses |
| WebSocket | `websocket` / `libp2p-websocket` | native | Usually with Noise/TLS + Yamux | Compatibility with WebSocket listeners |
| WebSocket websys | `websocket-websys` | WASM/browser | Browser WebSocket constraints apply | Browser fallback |
| WebRTC websys | `webrtc-websys` | WASM/browser | Native WebRTC streams | Browser peer connections |
| WebTransport websys | `webtransport-websys` | WASM/browser | WebTransport/HTTP3 constraints | Browser-to-compatible endpoints |
| UDS | `uds` / `libp2p-uds` | Unix native | Local IPC | Same-machine communication |
| Memory | `MemoryTransport` in core | tests | In-process only | Unit/integration tests |

QUIC does not need `.with_tcp(... noise ... yamux ...)` style upgrades because QUIC already provides encryption and multiplexing. TCP and WebSocket do need security and mux upgrades.

The builder’s native transport methods are gated out on `wasm32`. Browser builds use the `*-websys` transports and `wasm-bindgen`.

## Minimal Ping node

```rust
use std::{error::Error, time::Duration};

use futures::StreamExt;
use libp2p::{noise, ping, swarm::SwarmEvent, tcp, yamux, Multiaddr};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|_| ping::Behaviour::default())?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    if let Some(addr) = std::env::args().nth(1) {
        let remote: Multiaddr = addr.parse()?;
        swarm.dial(remote)?;
    }

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("Listening on {address}");
            }
            SwarmEvent::Behaviour(event) => {
                println!("{event:?}");
            }
            _ => {}
        }
    }
}
```

## Composite behaviour template

Use this pattern for real applications that need discovery plus messaging:

```rust
use std::{error::Error, time::Duration};

use futures::StreamExt;
use libp2p::{
    gossipsub, identify, kad, noise, ping,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};

#[derive(NetworkBehaviour)]
struct Behaviour {
    identify: identify::Behaviour,
    kad: kad::Behaviour<kad::store::MemoryStore>,
    gossipsub: gossipsub::Behaviour,
    ping: ping::Behaviour,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut swarm = libp2p::SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_behaviour(|key| {
            let peer_id = key.public().to_peer_id();

            let identify = identify::Behaviour::new(identify::Config::new(
                "/myapp/1.0.0".to_string(),
                key.public(),
            ));

            let store = kad::store::MemoryStore::new(peer_id);
            let kad = kad::Behaviour::new(peer_id, store);

            let gossipsub = gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub::Config::default(),
            )?;

            Ok(Behaviour {
                identify,
                kad,
                gossipsub,
                ping: ping::Behaviour::default(),
            })
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(60)))
        .build();

    swarm.listen_on("/ip4/0.0.0.0/tcp/0".parse()?)?;

    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("Listening on {address}");
            }
            SwarmEvent::Behaviour(event) => {
                println!("{event:?}");
            }
            _ => {}
        }
    }
}
```

For this template, enable:

```toml
libp2p = { version = "0.56", features = [
  "gossipsub", "identify", "kad", "macros", "noise", "ping", "tcp", "tokio", "yamux"
] }
```

## Identify

`identify::Behaviour` exchanges `Info` messages on established connections. It reports listen addresses, protocol support, observed address, and public key information.

Important Rust-specific discrepancy: Identify is not treated as core. Other behaviours do not automatically get Identify data. You must manually hook Identify events into protocols that need address information.

Kademlia integration pattern:

```rust
match event {
    SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
        peer_id,
        info,
        ..
    })) => {
        for addr in info.listen_addrs {
            swarm.behaviour_mut().kad.add_address(&peer_id, addr);
        }
    }
    _ => {}
}
```

The exact generated `BehaviourEvent` enum name depends on the derive macro output and the containing behaviour name. Adjust match paths to the actual compiler-generated event type.

## Kademlia DHT

Use `kad::Behaviour` for:

- Peer discovery and routing.
- Provider records: “peer X can provide content key Y.”
- Record get/put operations.
- Interop with IPFS Kademlia when configured for the IPFS protocol name.

Common setup:

```rust
let local_peer_id = key.public().to_peer_id();
let store = kad::store::MemoryStore::new(local_peer_id);
let behaviour = kad::Behaviour::new(local_peer_id, store);
```

Custom config example, useful for IPFS DHT interop:

```rust
use libp2p::swarm::StreamProtocol;

const IPFS_PROTO_NAME: StreamProtocol = StreamProtocol::new("/ipfs/kad/1.0.0");

let mut cfg = kad::Config::new(IPFS_PROTO_NAME);
cfg.set_query_timeout(std::time::Duration::from_secs(5 * 60));

let store = kad::store::MemoryStore::new(local_peer_id);
let kad = kad::Behaviour::with_config(local_peer_id, store, cfg);
```

Practical Kademlia calls:

```rust
swarm.behaviour_mut().kad.bootstrap()?;
swarm.behaviour_mut().kad.add_address(&peer_id, addr);
swarm.behaviour_mut().kad.get_closest_peers(peer_id);
swarm.behaviour_mut().kad.get_providers(key);
swarm.behaviour_mut().kad.start_providing(key)?;
```

Gotchas:

- Identify is not automatic. Hook `identify::Event::Received` to `kad.add_address`.
- Without Identify or another peer discovery source, Kad may not discover beyond bootnodes.
- For provider records/content routing, make sure the node is configured for the intended mode and reachable enough for the network’s expectations.
- IPFS public DHT interop often needs RSA identity support and `/ipfs/kad/1.0.0`.

## Gossipsub

Use `gossipsub::Behaviour` for pub/sub.

Common setup:

```rust
let behaviour = gossipsub::Behaviour::new(
    gossipsub::MessageAuthenticity::Signed(key.clone()),
    gossipsub::Config::default(),
)?;
```

Topic example:

```rust
let topic = gossipsub::IdentTopic::new("chat");
swarm.behaviour_mut().gossipsub.subscribe(&topic)?;
swarm.behaviour_mut().gossipsub.publish(topic, b"hello".to_vec())?;
```

Handle messages:

```rust
match event {
    gossipsub::Event::Message {
        propagation_source,
        message_id,
        message,
    } => {
        println!(
            "Got message {message_id} from {propagation_source}: {:?}",
            String::from_utf8_lossy(&message.data)
        );
    }
    _ => {}
}
```

Gotchas:

- Gossipsub does not do peer discovery. Combine it with mDNS, Kademlia, Rendezvous, bootstrap peers, or explicit peer dialing.
- Topic hashing behavior is configurable. Rust defaults differ from some implementations when `hash_topics` is enabled.
- For production networks, configure validation mode and peer scoring deliberately. Do not leave validation and scoring as an afterthought unless the application is a toy, which, fair enough, many are.

## Request-response

Use `request_response::Behaviour` for request/response protocols where each request is sent over a new substream.

Options:

- Implement `request_response::Codec` yourself.
- Use predefined `request_response::cbor::Behaviour` if request/response types implement serde and `cbor` feature is enabled.
- Use predefined `request_response::json::Behaviour` if `json` feature is enabled.

Core API:

```rust
let request_id = swarm
    .behaviour_mut()
    .request_response
    .send_request(&peer_id, request);

swarm
    .behaviour_mut()
    .request_response
    .send_response(channel, response)?;
```

Events arrive as `request_response::Event::Message` with `request_response::Message::{Request, Response}`.

Use request-response for:

- RPC-like interactions.
- File chunk fetching.
- Metadata queries.
- One-shot application messages where pub/sub is the wrong tool.

Do not use it for long-lived streaming. Use custom streams or a protocol built for streaming.

## Ping

`ping::Behaviour` measures liveness/RTT and emits ping events. It is useful for diagnostics but does not keep a connection alive forever by itself in a minimal node.

The default idle connection timeout can close otherwise-unused connections. Set a larger idle timeout in examples where Ping is the only active behaviour:

```rust
.with_swarm_config(|cfg| {
    cfg.with_idle_connection_timeout(std::time::Duration::from_secs(60))
})
```

The upstream tutorial uses a very large timeout so users can observe pings indefinitely. Do not blindly copy that into production.

## mDNS

Use `mdns::tokio::Behaviour` for local network discovery on native Tokio builds.

Typical pattern:

- Enable `mdns`, `tokio`, and `macros`.
- Compose `mdns::tokio::Behaviour` into your root behaviour.
- On `mdns::Event::Discovered`, add discovered peers to the relevant behaviour, for example `gossipsub.add_explicit_peer` or `kad.add_address`.
- On `mdns::Event::Expired`, remove explicit peers if appropriate.

mDNS is LAN-only. It does not discover peers across the internet.

## Rendezvous

Use `rendezvous` for peer registration/discovery around a rendezvous server. It is useful when:

- You control rendezvous points.
- Peers cannot rely on global DHT discovery.
- You want application-scoped discovery.

Rendezvous complements Kademlia or can replace it in controlled networks.

## Relay, AutoNAT, DCUtR, UPnP

rust-libp2p has a NAT traversal suite:

| Capability | Crate/feature | Use |
|---|---|---|
| Circuit Relay v2 | `relay` | Reach peers via public relay nodes |
| AutoNAT | `autonat` | Detect whether the node is publicly reachable |
| DCUtR | `dcutr` | Upgrade relayed connections to direct connections via hole punching |
| UPnP | `upnp` | Ask local router to map ports |

Relay address shape:

```text
/ip4/<relay-ip>/tcp/<relay-port>/p2p/<relay-peer-id>/p2p-circuit/p2p/<dest-peer-id>
```

Relay client setup uses `with_relay_client(...)` and requires embedding the relay client behaviour in your root behaviour.

DCUtR generally requires:

- Relay connectivity first.
- Identify, so peers can exchange observed/listen addresses.
- A relay client behaviour.
- Direct transports configured, commonly TCP and/or QUIC.
- Patience with NAT reality, the universe’s least charming firewall ruleset.

UPnP is native-only and only helps when the local router supports it. It is not available to browsers.

## Metrics and observability

Enable `metrics` to collect protocol/swarm metrics.

Common options:

- `with_bandwidth_metrics(&mut registry)` wraps the transport with bandwidth metrics.
- `libp2p-metrics` records protocol and swarm events and exposes OpenMetrics-compatible metrics via `prometheus-client`.

Use metrics for production services, relay nodes, and debugging peer churn.

## Connection limits and allow/block lists

The root crate re-exports:

- `allow_block_list`: manage allow/block lists for peers.
- `connection_limits`: connection count/limit utilities.
- `memory_connection_limits`: memory-aware limits on native builds.

Use these when building public-facing nodes, relays, or high-churn DHT/pubsub services.

## Browser and WASM notes

Rust-libp2p has WASM/browser transports behind `*-websys` feature flags:

- `webrtc-websys`
- `websocket-websys`
- `webtransport-websys`
- `wasm-bindgen`

Do not use native `tcp`, `dns`, `quic`, `websocket`, `uds`, or `upnp` builder paths on `wasm32`; they are target-gated out. Browser networking has platform constraints, certificate constraints, and event-loop constraints. Treat browser examples separately from native Tokio examples.

For browser-focused libp2p answers, cross-check the current WASM examples and transport crates. The Rust WASM implementation is not a drop-in translation of js-libp2p config.

## Source layout navigation

When source-checking:

- Root re-exports and feature gates: `libp2p/src/lib.rs`, `libp2p/Cargo.toml`.
- Builder: `libp2p/src/builder.rs` and `libp2p/src/builder/phase/*.rs`.
- Swarm: `swarm/src/lib.rs`.
- Behaviour trait: `swarm/src/behaviour.rs`.
- Handler trait: `swarm/src/handler.rs`.
- Connection pool: `swarm/src/connection/pool.rs`.
- Transports: `transports/*/src/lib.rs`.
- Protocols: `protocols/*/src/lib.rs`.
- Examples: `examples/*`.
- Ping tutorial: `libp2p/src/tutorials/ping.rs`.

## Example applications in the repo

Use these examples as source-backed patterns:

| Example | Demonstrates |
|---|---|
| `examples/ping` | Minimal SwarmBuilder + TCP + Noise + Yamux + Ping |
| `examples/chat` | Gossipsub chat |
| `examples/file-sharing` | Kademlia provider records + request-response file fetching |
| `examples/ipfs-kad` | IPFS Kademlia interoperability and bootstrapping |
| `examples/dcutr` | Relay + DCUtR hole punching |
| `examples/relay-server` | Running a relay server |
| `examples/autonat`, `examples/autonatv2` | Reachability detection |
| `examples/rendezvous` | Rendezvous protocol |
| `examples/identify` | Identify protocol |
| `examples/metrics` | Metrics collection |
| `examples/upnp` | Router port mapping |
| `examples/browser-webrtc` | Browser WebRTC/WASM pattern |
| `examples/stream` | Raw stream protocol example |

## Common mistakes

- Forgetting Cargo features, then wondering why modules do not exist.
- Using `libp2p::NetworkBehaviour` instead of `libp2p::swarm::NetworkBehaviour` for the derive macro.
- Forgetting the `macros` feature for `#[derive(NetworkBehaviour)]`.
- Not polling the `Swarm`.
- Assuming Identify automatically feeds Kademlia.
- Assuming Gossipsub discovers peers by itself.
- Adding QUIC and then also trying to wrap it in Noise/Yamux.
- Using browser transports in native examples, or native transports in WASM examples.
- Using `mplex` in new code when Yamux is the recommended muxer.
- Relying on Ping to keep a connection open without adjusting idle timeout or having an actual application protocol.
- Advertising private or unroutable addresses as external addresses.
- Assuming relay traffic is invisible to the relay. Payloads are end-to-end encrypted, but relay peer IDs, timing, and connection metadata are observable.

## Answering checklist

When answering rust-libp2p questions:

1. Identify target environment: native Tokio, WASM/browser, tests, or embedded/server.
2. Identify required protocols: discovery, pub/sub, request-response, DHT, NAT traversal, raw streams.
3. List the Cargo features required.
4. Prefer `SwarmBuilder` unless the user is writing a custom transport/protocol.
5. Use `#[derive(NetworkBehaviour)]` for app-level composition.
6. Include the event loop.
7. Mention Identify wiring when using Kademlia or address discovery.
8. Mention discovery requirements when using Gossipsub.
9. For NAT traversal, include relay, Identify, AutoNAT/DCUtR, and direct transports.
10. Warn about target-gated transports and version/API drift.
11. For exact APIs, cite or inspect docs.rs/source for the current version.

## Quick recipes

### Native TCP + Noise + Yamux

Features:

```toml
["tcp", "tokio", "noise", "yamux"]
```

Builder:

```rust
.with_tokio()
.with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
```

### Native TCP + QUIC

Features:

```toml
["tcp", "quic", "tokio", "noise", "yamux"]
```

Builder:

```rust
.with_tokio()
.with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
.with_quic()
```

### TCP + DNS + WebSocket

Features:

```toml
["tcp", "dns", "websocket", "tokio", "noise", "yamux"]
```

Builder:

```rust
.with_tokio()
.with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default)?
.with_dns()?
.with_websocket(noise::Config::new, yamux::Config::default).await?
```

### DHT + Identify

Features:

```toml
["kad", "identify", "macros", "tcp", "tokio", "noise", "yamux"]
```

Pattern:

- Compose both behaviours.
- On Identify received events, call `kad.add_address(&peer_id, addr)` for listen addresses.
- Add bootstrap peer addresses explicitly.
- Call `kad.bootstrap()` after bootstrap peers are in the routing table.

### Pub/sub

Features:

```toml
["gossipsub", "identify", "mdns", "macros", "tcp", "tokio", "noise", "yamux"]
```

Pattern:

- Compose Gossipsub with mDNS and/or Kademlia/Rendezvous.
- Subscribe to topics.
- Add explicit peers or rely on discovered/dialed peers.
- Configure validation and scoring for production.

### Request/response with CBOR

Features:

```toml
["request-response", "cbor", "macros", "tcp", "tokio", "noise", "yamux"]
```

Pattern:

- Define serde request and response types.
- Use `request_response::cbor::Behaviour`.
- Send with `send_request`.
- Respond with `send_response` using the response channel from inbound request events.

### Relay client

Features:

```toml
["relay", "macros", "tcp", "tokio", "noise", "yamux"]
```

Pattern:

- Add `.with_relay_client(...)`.
- Include relay behaviour in the root behaviour.
- Listen or dial relayed multiaddrs containing `/p2p-circuit`.
- Add Identify and DCUtR for hole punching.

## Reference links

- DeepWiki rust-libp2p: https://deepwiki.com/libp2p/rust-libp2p
- GitHub repository: https://github.com/libp2p/rust-libp2p
- docs.rs `libp2p`: https://docs.rs/libp2p/latest/libp2p/
- crates.io `libp2p`: https://crates.io/crates/libp2p
- Official libp2p docs: https://docs.libp2p.io/
