## Features

- [Flutter](https://flutter.dev/) UI with Windows, Linux, macOS, iOS, and Android support
- [libp2p](https://libp2p.io/) networking and cryptography, enables p2p networking without port forwarding
- Lossless raw audio and [SEA codec](https://github.com/Daninet/sea-codec) options
- [nnnoiseless](https://github.com/jneem/nnnoiseless) noise suppression
- Built-in text chat with attachments
- Efficient use of CPU and memory resources
- Low end-to-end latency

## Work in Progress

- Screensharing for Windows, macOS, and Linux
- Game overlay for Windows
- Web support
- Telepathy rooms (group calls)

## Planned

- Lossless audio codec support
- QUIC datagrams for live audio streams
- p2p updates for desktop clients

## History

- Began in 2023 as Audio Chat, a Python Tkinter app with simple UDP networking and AES cryptography
- Moved to the current Rust audio processing stack & Flutter UI for improved stability in 2024
- Soon after, the custom networking and cryptography stack was replaced with libp2p for improved security, p2p networking without port forwarding, and p2p networking in web browsers
- In 2025, performance improvements were made, SEA codec support was added, and many bugs were fixed

## Architecture

### High Level Design
- Flutter to Rust (and back) is enabled by [Flutter Rust Bridge](https://pub.dev/packages/flutter_rust_bridge)
- This design enables the same codebase to target desktop, mobile, and web

![a diagram explaining the high level structure of the telepathy app](assets/diagrams/telepathy-design.svg)

### Audio Processing Stack
- Denoising runs on the sending side; each participant in a call decides if they want to use their compute resources to denoise their audio input
- Every participant in a call must agree on the same audio codec options for sending & receiving
- If a frame's RMS is below the input sensitivity threshold, no audio is sent (keep-alive packets are used during silence)
- In a classic two-way call, each client runs a sending stack and a receiving stack
- In a Telepathy room, certain parts of each stack are duplicated to support more participants

![a diagram describing the telepathy audio processing stack](assets/diagrams/audio-processing-stack.svg)

## UI Screenshots
![screenshot of telepathy main user interface](https://chanchan.dev/static/images/telepathy.png)
![screenshot of telepathy settings user interface](https://chanchan.dev/static/images/telepathy-settings.png)
