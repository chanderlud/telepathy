Telepathy is a cross-platform peer-to-peer (P2P) chat application built to deliver real-time communication without relying on centralized servers.
The UI is built with [Flutter](https://flutter.dev/) enabling the app to target Windows, Linux, macOS, iOS, Android, and the [web](https://telepathy.chanchan.dev/).
[libp2p](https://libp2p.io/) is used for P2P networking and cryptography, enabling secure connections behind NAT/firewalls without requiring manual port forwarding.


## Features

- [Flutter](https://flutter.dev/) UI with Windows, Linux, macOS, iOS, Android, and web support
- [libp2p](https://libp2p.io/) networking and cryptography, enables p2p networking without port forwarding
- Lossless raw audio and [SEA codec](https://github.com/Daninet/sea-codec) options
- [nnnoiseless](https://github.com/jneem/nnnoiseless) noise suppression
- Built-in text chat with attachments
- Efficient use of CPU and memory resources
- Low end-to-end latency

### Work in Progress

- Screensharing for Windows, macOS, and Linux
- Game overlay for Windows
- Web support
- Telepathy rooms (group calls)

### Planned

- Lossless audio codec support
- QUIC datagrams for live audio streams
- p2p updates for desktop clients

## Local Development
- Flutter, Dart, and Cargo are required for building the project
- For development, use `flutter run -d <device>` or `flutter build <device> --debug`
- Live reload is supported in JetBrains and other IDEs with the Flutter plugin
- For release builds, use `flutter build <device>`

### Additional Requirements
- Android development requires Android Studio
- macOS and iOS development requires Xcode
- Web development requires the latest wasm-pack and wasm-opt & the nightly Rust toolchain

## Architecture

### High Level Design
- Flutter to Rust (and back) is enabled by [Flutter Rust Bridge](https://pub.dev/packages/flutter_rust_bridge)
- This design enables the same codebase to target desktop, mobile, and the web

![a diagram explaining the high level structure of the telepathy app](https://chanchan.dev/vectors/diagrams/telepathy-design.svg)

### Audio Processing
- Telepathy's real-time audio processing is implemented in the [Telepathy Audio crate](https://github.com/chanderlud/telepathy/tree/master/rust/telepathy_audio)
- A simple, high level API is exposed for creating input & output streams, along with device enumeration, and sound effect playback
- Platform specific SIMD optimizations, a zero-allocation design, and the internal use of [rtrb](https://docs.rs/rtrb/latest/rtrb/)
enables high quality real-time performance on any device with remarkably low resource utilization

### Classic Call Design

- Telepathy Audio provides the audio processing while libp2p handles networking
- Denoising runs on the sending side; each participant in a call decides if they want to use their compute resources to denoise their audio input
- Every participant in a call must agree on the same audio codec options for sending & receiving
- If a frame's RMS is below the input sensitivity threshold, no audio is sent (keep-alive packets are used during silence). The output stream gracefully transitions between speech and silence using cross-fade
- In a classic two-way call, each client runs an input and output stream

![a diagram describing the telepathy audio processing stack](https://chanchan.dev/vectors/diagrams/audio-processing-stack.svg)

## Project History

- Telepathy started as “Audio Chat,” a Python Tkinter application with simple UDP networking and AES cryptography.
- After proving the concept with Python and Tkinter, the project was rewritten in Rust with Flutter for cross-platform support.
- The networking layer was upgraded from a custom approach to libp2p to gain enterprise-grade security primitives and more capable P2P connectivity.

## UI Screenshots
![screenshot of telepathy main user interface](https://chanchan.dev/cdn-cgi/image/width=828,fit=scale-down,format=auto/images/projects/telepathy/cover.png)
![screenshot of telepathy settings user interface](https://chanchan.dev/cdn-cgi/image/width=828,fit=scale-down,format=auto/images/projects/telepathy/settings.png)

## Learn More
[chanchan.dev](https://chanchan.dev/work/telepathy)
