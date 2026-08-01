# Telepathy

Private, low-latency peer-to-peer voice chat for desktop, mobile, and the web.

Telepathy is an open-source communication app built with Flutter, Rust, and iroh.
It combines real-time audio, video, and text chat in a lightweight cross-platform application.

Download Telepathy from [GitHub Releases](https://github.com/chanderlud/telepathy/releases) for the native experience, or try Telepathy in the [browser](https://telepathy.chanchan.dev).

## Features

- [Flutter](https://flutter.dev/) UI with Windows, Linux, macOS, iOS, Android, and web support.
- [iroh](https://www.iroh.computer/) networking, direct p2p connectivity with QUIC, TLS 1.3, and post-quantum cryptography by default.
- Lossless 16 bit raw audio and [SEA codec](https://github.com/Daninet/sea-codec) support.
- [nnnoiseless](https://github.com/jneem/nnnoiseless) noise suppression.
- Built-in text chat with media and file attachments.
- Efficient use of CPU and memory resources, more than 10x lower than Discord.
- Low end-to-end latency enabled by direct connectivity and low processing delay.

### Work in Progress

- ffmpeg based screensharing for Windows, macOS, and Linux.
- Game overlay for Windows.
- Telepathy rooms (group calls).

### Planned

- Lossless audio codec support.
- Built in update pipeline with version checking and patching.
- Echo cancellation.
- Automatic input gain.
- Webcam, camera, and screenshare support for desktop, mobile, and web.
- Signed desktop builds, App Store and Android Playstore downloads.
- Standalone Telepathy Audio and Telepathy Video crates.

## Local Development
- Flutter, Dart, and Cargo are required for building the project.
- For development, use `flutter run -d <device>` or `flutter build <device> --debug`.
- Live reload is supported in JetBrains and other IDEs with the Flutter plugin.
- For release builds, use `flutter build <device>`.

### Additional Requirements
- Android development requires Android Studio.
- macOS and iOS development requires Xcode.
- Web development requires the latest wasm-pack and wasm-opt & the nightly Rust toolchain.

## Verifying Release Artifacts

Every artifact published to a GitHub Release is signed with a [Sigstore](https://sigstore.dev/) build-provenance attestation that binds it to the workflow run and commit that produced it. Verify any downloaded asset before installing:

```sh
gh attestation verify <downloaded-file> --repo chanderlud/telepathy
```

Example:

```sh
gh attestation verify telepathy-1.2.3-windows-installer-x86_64.exe --repo chanderlud/telepathy
```

A passing check confirms the file was built from the `chanderlud/telepathy` repository on GitHub Actions and has not been modified since. Requires the [GitHub CLI](https://cli.github.com/) (`gh`) to be installed and authenticated.

Note: attestation is provenance, not code signing. It does not satisfy macOS Gatekeeper or Windows SmartScreen, and the macOS/iOS builds remain unsigned.

## Architecture

### High Level Design
- Flutter to Rust (and back) is enabled by [Flutter Rust Bridge](https://pub.dev/packages/flutter_rust_bridge).
- This design enables the same codebase to target desktop, mobile, and the web.

![a diagram explaining the high level structure of the telepathy app](https://chanchan.dev/vectors/diagrams/telepathy-design.svg)

### Audio Processing
- Telepathy's real-time audio processing is implemented in the [Telepathy Audio crate](https://github.com/chanderlud/telepathy/tree/master/rust/telepathy_audio).
- A simple, high level API is exposed for creating input & output streams, along with device enumeration, and sound effect playback.
- Platform specific SIMD optimizations, a zero-allocation design, and the internal use of [rtrb](https://docs.rs/rtrb/latest/rtrb/)
enables high quality real-time performance on any device with remarkably low resource utilization.

### Classic Call Design

- Telepathy Audio provides the audio processing while iroh handles networking.
- Denoising runs on the sending side; each participant in a call decides if they want to use their compute resources to denoise their audio input.
- Every participant in a call must agree on the same audio codec options for sending & receiving.
- If a frame's RMS is below the input sensitivity threshold, no audio is sent (keep-alive packets are used during silence). The output stream gracefully transitions between speech and silence using cross-fade.
- In a classic two-way call, each client runs an input and output stream.

![a diagram describing the telepathy audio processing stack](https://chanchan.dev/vectors/diagrams/audio-processing-stack.svg)

## Project History

- Telepathy started as "Audio Chat," a Python Tkinter application with simple UDP networking and AES cryptography.
- After proving the concept with Python and Tkinter, the project was rewritten in Rust with Flutter for cross-platform support.
- The networking layer was upgraded from a custom approach to libp2p to gain enterprise-grade security primitives and more capable P2P connectivity.
- libp2p was replaced with iroh for more robust real-time networking & simpler session logic.

## Media

### Main Screen
![screenshot of telepathy main user interface](https://chanchan.dev/cdn-cgi/image/width=828,fit=scale-down,format=auto/images/projects/telepathy/cover.png)

### Settings
![screenshot of telepathy settings user interface](https://chanchan.dev/cdn-cgi/image/width=828,fit=scale-down,format=auto/images/projects/telepathy/settings.png)

### Direct Call
![gif animation showing telepathy direct call](https://chanchan.dev/images/projects/telepathy/call.gif)

## Learn More
- Article: [chanchan.dev](https://chanchan.dev/work/telepathy)
- Contact: [me@chanchan.dev](mailto:me@chanchan.dev)
