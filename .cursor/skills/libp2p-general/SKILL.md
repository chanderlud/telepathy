---
name: libp2p
description: This skill should be used when the user asks about "libp2p", "peer ID", "multiaddr", "WebRTC", "WebTransport", "WebSocket", "circuit relay", "Kademlia DHT", "NAT traversal", "js-libp2p", "stream multiplexing", "Yamux", "mplex", "Noise protocol", "peer discovery", "kad-dht", "hole punching", "DCUtR", or needs to understand libp2p networking for browser-based or decentralized applications.
---

# libp2p

## Conceptual Overview

libp2p (short for "library peer-to-peer") is a modular networking stack for building P2P applications. It provides peer-to-peer connectivity without requiring a central server. Originally developed as the networking layer for IPFS, it has evolved into a standalone networking framework adopted by many projects.

libp2p is built on a few core principles:

- **Modularity** — mix and match transports, security channels, and multiplexers
- **Transport independence** — work across browsers, Node.js, and native environments
- **NAT traversal** — connect peers behind firewalls and NATs via relay and hole-punching
- **Interoperability** — implementations in Go, Rust, JavaScript, and Nim communicate seamlessly

## Core Concepts

### Peer ID

Each libp2p peer has a unique identity derived from a public/private key pair:

```javascript
const peerId = await createPeerId(); // random key pair → peer ID
// Peer ID is a multihash of the public key
```

Peer IDs are encoded as base58 or multicodec-encoded strings. They're used to:

- Uniquely identify a peer on the network
- Verify peer identity cryptographically (only someone with the private key controls the peer ID)
- Encrypt communication channels

### Multiaddresses

Multiaddresses (`multiaddr`) encode multiple layers of addressing information into a single path:

```
/ip4/198.51.100.0/tcp/4242/p2p/QmYyQSo1c1Ym7orWxLYvCrM2EmxFTANf8wXmmE7DWjhx5N
 ↑                ↑      ↑
 IPv4 address   port   libp2p peer ID
```

**Key protocols in multiaddrs:**

- `/ip4/<addr>` or `/ip6/<addr>` — IP address
- `/tcp/<port>` or `/udp/<port>` — transport port
- `/ws/` — WebSocket
- `/wss/` — WebSocket Secure (TLS)
- `/p2p/<peer-id>` — libp2p peer ID
- `/p2p-circuit/` — circuit relay ( NAT traversal)
- `/webrtc/` or `/webrtc-direct/` — WebRTC transport
- `/webtransport/` — WebTransport (QUIC-based)
- `/certhash/<hash>` — certificate hash for WebTransport verification

**Example: WebRTC multiaddr for browser-to-browser via relay:**

```
/ip4/198.51.100.0/tcp/54321/quic-v1/p2p/<relay-peer-id>/p2p-circuit/webrtc/p2p/<dest-peer-id>
```

### Connections and Streams

- **Connection**: A raw transport-level link between two peers (e.g., a TCP socket, WebRTC channel)
- **Stream**: A bidirectional virtual channel within a connection, multiplexed by a stream multiplexer. Multiple protocols share a single connection by opening separate streams.

```javascript
// Open a stream to a peer
const { stream } = await libp2p.dial(peerId)
// Send data on the stream
stream.write(data)
// Read data from the stream
for await (const chunk of stream.source) { ... }
```

## Transport Layer

Transports are the bottom-most layer — they establish raw connections between peers.

### Browser-Supported Transports

| Transport                       | Browser Support                           | NAT Traversal                 | Certificate Handling                             |
| ------------------------------- | ----------------------------------------- | ----------------------------- | ------------------------------------------------ |
| **WebSocket** (wss)             | All modern browsers                       | Requires CA-signed TLS        | Problematic — most peers have no domain/CA cert  |
| **WebTransport**                | Chrome, Firefox, Opera, Edge (NOT Safari) | Certificate hash in multiaddr | Solves the cert problem via self-signed + hash   |
| **WebRTC Direct**               | All modern browsers                       | STUN required                 | Self-signed OK, Noise handshake verifies peer ID |
| **WebRTC** (browser-to-browser) | All modern browsers                       | Requires relay                | Self-signed OK, Noise handshake verifies peer ID |

### Transport Selection for Browser Apps

For browser-based libp2p apps:

1. **WebTransport** (3 RTT) — best for Chrome/Firefox, self-signed certs with hash-in-multiaddr
2. **WebRTC Direct** (2-3 RTT) — for browser-to-node, cert hash piggybacks on libp2p handshake
3. **WebSocket** (5 RTT) — fallback, requires CA-signed TLS — problematic for peers without domains
4. **WebRTC via Relay** — for browser-to-browser when direct connection fails

### WebRTC Details

WebRTC in libp2p comes in two flavors:

- **WebRTC Direct** — browser to public node (simplified, no signaling needed)
- **WebRTC** — browser to browser (requires relay for signaling)

**Connection establishment:**

1. SDP (Session Description Protocol) offer/answer exchange
2. ICE candidates for NAT traversal
3. DTLS handshake
4. SCTP stream establishment
5. libp2p Noise handshake on top (verifies peer ID)

**Multiaddr format for WebRTC Direct:**

```
/ip4/192.0.2.0/udp/12345/webrtc-direct/certhash/<sha256-hash>/p2p/<peer-id>
```

### WebTransport

WebTransport uses HTTP/3 (QUIC) and solves WebSocket's certificate problem:

- Certificate verification via hash in multiaddr: `/certhash/<sha256-of-cert>`
- Noise handshake on first stream binds cert hash to peer ID (prevents MITM)
- 3 RTT total (QUIC 1 + WebTransport upgrade 1 + Noise 1)

## NAT Traversal

### The NAT Problem

Most peers are behind NAT (home routers) or firewalls. They can't receive incoming TCP/UDP connections from the public internet. NAT traversal techniques solve this.

### Circuit Relay v2

The primary NAT traversal solution in libp2p. A relay peer acts as a middleman:

- Peers behind NAT **dial out** to the relay and maintain a reservation
- Other peers connect **through** the relay to reach them
- Connections are **end-to-end encrypted** — the relay cannot read traffic

**Relay address construction:**

```
/p2p/<relay-id>/p2p-circuit/p2p/<target-peer-id>
```

**Protocol flow:**

1. Peer A (behind NAT) requests reservation with relay R
2. Peer B connects to R, asking to relay to A
3. R forwards traffic between A and B

**Reservation**: Peers behind NAT dial out to relays to maintain a "reservation" — a long-lived connection the relay keeps open on their behalf.

### AutoNAT

AutoNAT helps peers determine if they're actually behind a NAT and unable to receive connections. When enabled, libp2p can:

- Detect that it can't be dialed directly
- Automatically request circuit relay reservations
- Advertise relay addresses instead of non-working direct addresses

### Hole Punching (DCUtR)

DCUtR (Direct Connection Upgrade through Relay) enables direct connections after initial relay contact:

1. Both peers connect via relay
2. They exchange direct address candidates (via relay stream)
3. They attempt direct connection (UDP hole punching or TCP)
4. On success, upgrade to direct connection and close relay stream

### NAT Port Mapping (UPnP / nat-pmp)

For standalone nodes (not browsers), libp2p can automatically configure router port forwarding via UPnP or NAT-PMP protocols. Browser environments cannot use this.

## Peer Discovery & Routing

### Kademlia DHT

libp2p uses a Kademlia-style DHT for peer and content routing:

- **Keyspace**: 256-bit (SHA-256), divided into buckets
- **Routing table**: maintains k=20 closest peers for each prefix length 0-255
- **Distance metric**: XOR of SHA-256 hashes — smaller XOR distance = closer in keyspace

**DHT Operations:**

- `FIND_NODE`: Given a peer ID, find the k closest known peers
- `FIND_VALUE`: Given a content key, find providers or the value itself
- `PUT_VALUE`: Store a value under a key

**Browser limitation**: Browser peers typically can't participate fully in DHT due to connection instability. They're usually consumers of DHT lookups, not full DHT nodes.

### mDNS

Multicast DNS for local network peer discovery:

- Peers broadcast their presence to the local network via multicast
- Zero-configuration discovery on LAN (home networks, conference WiFi)
- No internet required

### Bootstrap Nodes

A common pattern is connecting to one or more well-known "bootstrap" peers on startup:

```javascript
import { bootstrap } from "@libp2p/bootstrap";

const node = await createLibp2p({
  peerDiscovery: [
    bootstrap({
      list: [
        "/ip4/1.2.3.4/tcp/4001/p2p/QmBootstrapPeer",
        "/dnsaddr/bootstrap.libp2p.io/p2p/QmBootstrapPeer2",
      ],
    }),
  ],
});
```

These bootstrap peers help discover the rest of the network.

## Stream Multiplexing

Stream multiplexers allow multiple streams (virtual connections) over a single connection. This is essential because:

- Opening connections is expensive (especially NAT traversal)
- Different protocols need independent channels
- It prevents head-of-line blocking

**libp2p muxers:**

- **Yamux** (recommended) — header-based, windowed flow control
- **mplex** — message-based, simpler but deprecated

**Note:** QUIC, WebTransport, and WebRTC have native multiplexing built-in. Only raw TCP and WebSocket need external muxers.

## Secure Channels

All libp2p connections must be encrypted. Two options:

### Noise

The most common option for libp2p:

- `@chainsafe/libp2p-noise` — used by IPFS and most libp2p apps
- Handles key exchange and encryption
- Used by WebRTC and WebTransport connections

### TLS 1.3

`@libp2p/tls` — newer option, uses native TLS 1.3

## js-libp2p Configuration (Browser)

A typical browser libp2p configuration:

```javascript
import { createLibp2p } from "libp2p";
import { webSockets } from "@libp2p/websockets";
import { webRTC } from "@libp2p/webrtc";
import { webTransport } from "@libp2p/webtransport";
import { circuitRelayTransport } from "@libp2p/circuit-relay-v2";
import { noise } from "@chainsafe/libp2p-noise";
import { yamux } from "@chainsafe/libp2p-yamux";
import { identify } from "@libp2p/identify";
import { kadDHT } from "@libp2p/kad-dht";
import { bootstrap } from "@libp2p/bootstrap";

const node = await createLibp2p({
  addresses: {
    listen: [
      "/p2p-circuit", // listen for relayed connections
      "/webrtc", // listen for direct WebRTC
    ],
  },
  transports: [webSockets(), webTransport(), webRTC(), circuitRelayTransport()],
  connectionEncrypters: [noise()],
  streamMuxers: [yamux()],
  peerDiscovery: [
    bootstrap({
      list: ["/ip4/1.2.3.4/tcp/4001/ws/p2p/QmBootstrap"],
    }),
  ],
  services: {
    identify: identify(),
    dht: kadDHT(), // browser DHT is limited — read-only
  },
});
```

## Key Packages (npm)

| Package                       | Purpose                                |
| ----------------------------- | -------------------------------------- |
| `libp2p`                      | Core library                           |
| `@libp2p/webrtc`              | WebRTC transport                       |
| `@libp2p/websockets`          | WebSocket transport                    |
| `@libp2p/webtransport`        | WebTransport transport                 |
| `@libp2p/circuit-relay-v2`    | Circuit Relay v2                       |
| `@libp2p/kad-dht`             | Kademlia DHT                           |
| `@libp2p/bootstrap`           | Bootstrap peer discovery               |
| `@libp2p/mdns`                | mDNS discovery                         |
| `@libp2p/identify`            | Identify protocol (exchange peer info) |
| `@libp2p/ping`                | Ping service                           |
| `@chainsafe/libp2p-noise`     | Noise encryption                       |
| `@chainsafe/libp2p-yamux`     | Yamux multiplexer                      |
| `@chainsafe/libp2p-gossipsub` | GossipSub pubsub                       |

## Important Constraints / Gotchas

- **Browser peer stability**: Browser peers are often short-lived and can't maintain many long-term connections. DHT participation is limited.
- **NAT types**: NAT traversal success rates vary. Cone NATs are easy; symmetric NATs are hard. Mobile networks often use carrier-grade NAT (CGNAT) which is very difficult.
- **Certificate management for WebSocket**: WebSocket requires CA-signed TLS certificates in browsers. Most libp2p nodes don't have domains or CA certs. This makes WebSocket a niche transport in browser contexts.
- **WebTransport is not Safari**: If Safari support matters, you can't rely on WebTransport alone.
- **Circuit relay is not transparent**: Both peers know they're relayed. The relay sees peer IDs and timing but not content (end-to-end encrypted).
- **Yamux required for WebSocket relay**: WebTransport and WebRTC have native multiplexing, but when using WebSocket with circuit relay, Yamux is needed to multiplex streams on the single WebSocket connection.
- **Self-signed certs for WebTransport**: You can generate self-signed certs and encode the hash in the multiaddr. This is how WebTransport solves the CA cert problem.

## Reference Links

- [libp2p official docs](https://libp2p.io/docs/)
- [js-libp2p GitHub](https://github.com/libp2p/js-libp2p)
- [libp2p specs](https://github.com/libp2p/specs)
- [WebRTC with js-libp2p guide](https://github.com/libp2p/libp2p-webrtc-guide)
- [Protocol Labs PeerID docs](https://docs.libp2p.io/concepts/peer-id/)
