# Telepathy Audio Library

A standalone audio processing library for the Telepathy project, providing device management, audio capture, playback, and codec support.

## Features

- **Device Management**: Enumerate and select audio input/output devices across platforms
- **Audio Capture**: High-quality audio input with optional RNNoise noise suppression
- **Audio Playback**: Low-latency audio output with automatic resampling
- **Codec Support**: SEA codec encoding/decoding for efficient network transmission
- **SIMD Optimization**: Hardware-accelerated audio processing with automatic CPU feature detection (x86_64 AVX2/AVX-512, WASM SIMD v128)
- **Cross-Platform**: Native support for Windows, macOS, Linux, iOS, Android, and WebAssembly

## Usage

### Device Enumeration

```rust
use telepathy_audio::{AudioHost, list_all_devices, get_default_input_device};

// Create an audio host
let host = AudioHost::new();

// List all available devices
let devices = list_all_devices(&host).unwrap();
println!("Input devices: {:?}", devices.input_devices);
println!("Output devices: {:?}", devices.output_devices);

// Get the default input device
let input_device = get_default_input_device(&host).unwrap();
println!("Default input: {}", input_device.name().unwrap());
```

### Audio Input with Callback

```rust
use telepathy_audio::{AudioHost, AudioInputBuilder};

let host = AudioHost::new();

// Create an audio input with processing
let input = AudioInputBuilder::new()
    .volume(1.0)
    .denoise(true, None)  // Enable noise suppression
    .rms_threshold(0.01)  // Silence detection threshold
    .callback(|data| {
        // Process or transmit the audio data
        println!("Received {} bytes", data.len());
    })
    .build(&host)
    .unwrap();

// Control the input stream
input.mute();
input.set_volume(0.8);
input.unmute();

// Stream stops when handle is dropped
```

### Audio Output

```rust
use telepathy_audio::{AudioHost, AudioOutputBuilder};

let host = AudioHost::new();

// Create an audio output
let output = AudioOutputBuilder::new()
    .sample_rate(48000)
    .volume(1.0)
    .build(&host)
    .unwrap();

// Get sender for feeding audio data
let sender = output.sender();

// Send audio data through the sender
// sender.send(audio_message).unwrap();

// Control the output
output.set_volume(0.8);
output.deafen();   // Silence output
output.undeafen(); // Resume output
```

### Multiple Outputs

The library supports creating multiple independent output streams:

```rust
use telepathy_audio::{AudioHost, AudioOutputBuilder};

let host = AudioHost::new();

// Create multiple outputs for different audio sources
let output1 = AudioOutputBuilder::new()
    .sample_rate(48000)
    .build(&host)
    .unwrap();

let output2 = AudioOutputBuilder::new()
    .sample_rate(44100)  // Different sample rate
    .build(&host)
    .unwrap();

// Each output has its own sender
let sender1 = output1.sender();
let sender2 = output2.sender();
```

### With Codec Support

```rust
use telepathy_audio::{AudioHost, AudioInputBuilder, AudioOutputBuilder};

let host = AudioHost::new();

// Input with codec encoding
let input = AudioInputBuilder::new()
    .codec(
        true,   // enabled: enable SEA codec encoding
        false,  // vbr: use constant bit rate (CBR) instead of variable bit rate
        5.0     // residual_bits: quality setting (1.0-8.0, higher = better quality)
    )
    .callback(|encoded_data| {
        // Send encoded data over network
    })
    .build(&host)
    .unwrap();

// Output with codec decoding
let output = AudioOutputBuilder::new()
    .codec(true)  // Enable codec decoding
    .build(&host)
    .unwrap();
```

> **Note on Sample Rates**: The processor's output sample rate depends on the configuration:
>
> - **Denoise enabled**: Always outputs at 48kHz (required by RNNoise). Input is upsampled to 48kHz for noise suppression processing.
> - **Denoise disabled with custom `output_sample_rate`**: Uses the specified rate. Useful for matching network requirements without denoising.
> - **Denoise disabled without custom rate**: Uses the device's native sample rate (pass-through, no resampling).
>
> The encoder sample rate automatically matches the processor's output rate.
>
> ```rust
> // Example: Custom output sample rate without denoising
> let input = AudioInputBuilder::new()
>     .denoise(false, None)      // Disable denoising
>     .output_sample_rate(48000) // Force 48kHz output for network compatibility
>     .callback(|data| { /* ... */ })
>     .build(&host)
>     .unwrap();
> ```

## Architecture Notes

### Codec vs. No-Codec Paths

The library supports two distinct processing paths:

**With Codec Enabled:**
```
Input: Audio → Processor (with encoding) → Callback/Network
Output: Network → Processor (with decoding) → Audio
```

**Without Codec (Raw Audio):**
```
Input: Audio → Processor → Callback/Network
Output: Network → Processor → Audio
```

When codec is disabled, audio is transmitted as raw `Bytes` (i16 samples converted to bytes).
When codec is enabled, audio is compressed using the SEA codec before transmission.

### Thread Architecture

Each audio stream spawns dedicated threads:
- **Input**: 1 processor thread (with optional encoding) + optional callback thread
- **Output**: 1 processor thread (with optional decoding)

Encoding/decoding happens within the processor threads for better performance and reduced context switching. All threads communicate via lock-free channels (kanal) for low-latency operation.

## Module Organization

The library is organized into a hierarchical module structure:

```
telepathy_audio/
├── devices       - Device enumeration and selection (public)
├── io/           - Audio I/O module (public)
│   ├── input     - Audio input capture
│   └── output    - Audio output playback
├── player        - Audio file playback (public)
├── constants     - Public constants
├── error         - Error types
├── internal/     - Implementation details (private)
│   ├── codec     - SEA codec encoding/decoding
│   ├── processing- SIMD-optimized audio functions
│   ├── processor - Core audio processors
│   ├── state     - Processor state structs
│   ├── traits    - AudioInput/AudioOutput traits
│   └── utils     - Internal utilities
└── platform/     - Platform-specific code (private)
    └── web_audio - WASM audio implementation
```

**Public API**: `devices`, `io`, `player`, `constants`, `error`

## API Reference

### Types

- `AudioHost` - Central audio host for device management
- `AudioDeviceInfo` - Device name and ID information
- `AudioDeviceList` - Collection of input/output devices
- `DeviceHandle` - Handle to a selected device
- `AudioInputBuilder` - Builder for audio input configuration
- `AudioInputHandle` - Handle to running input stream
- `AudioInputConfig` - Configuration struct for audio input
- `AudioOutputBuilder` - Builder for audio output configuration
- `AudioOutputHandle` - Handle to running output stream
- `AudioOutputConfig` - Configuration struct for audio output
- `InputProcessorState` - State management for input processing (advanced)
- `OutputProcessorState` - State management for output processing (advanced)

### Functions

- `list_input_devices()` - List available input devices
- `list_output_devices()` - List available output devices
- `list_all_devices()` - List all devices
- `get_input_device()` - Get input device by ID (with fallback)
- `get_output_device()` - Get output device by ID (with fallback)
- `get_default_input_device()` - Get default input device
- `get_default_output_device()` - Get default output device

### Advanced APIs

These are re-exported for consumers that need lower-level access:

- `input_processor` / `output_processor` - Core processing functions
- `encoder` / `decoder` - Direct codec access
- `wide_mul` - SIMD-optimized audio multiplication
- `resampler_factory` - Create resamplers for sample rate conversion
- `InputProcessorState` / `OutputProcessorState` - State management for processors
- `FRAME_SIZE` - Standard frame size constant (480 samples)

## Dependencies

This library builds on several excellent Rust crates:

- [cpal](https://docs.rs/cpal) - Cross-platform audio I/O
- [rubato](https://docs.rs/rubato) - High-quality audio resampling
- [nnnoiseless](https://docs.rs/nnnoiseless) - RNNoise-based noise suppression
- [sea_codec](https://github.com/Daninet/sea-codec) - SEA audio codec for efficient transmission

## WASM Notes

On WASM targets, a `WebAudioWrapper` must be created ahead of time (async,
on the main thread during a user interaction) and provided to the builder
via `web_audio_wrapper()` before calling `build()`:

```rust
#[cfg(target_family = "wasm")]
fn setup_audio(wrapper: WebAudioWrapper) -> Result<(), AudioError> {
    use telepathy_audio::{AudioHost, AudioInputBuilder};
    use std::sync::Arc;
    
    let host = AudioHost::new();
    
    // Input requires a pre-initialized WebAudioWrapper on WASM
    let input = AudioInputBuilder::new()
        .web_audio_wrapper(wrapper)
        .callback(|data| { /* process audio */ })
        .build(&host)?;
    
    // Output uses synchronous build even on WASM
    let output = AudioOutputBuilder::new()
        .sample_rate(48000)
        .build(&host)?;
    
    Ok(())
}
```

**Key WASM Differences:**

1. **Input**: Must set `WebAudioWrapper` via `web_audio_wrapper()` before `build()`
2. **Output**: Uses `build` (no permissions needed)
3. **Sample Rate**: Fixed at 48kHz for Web Audio API compatibility
4. **Threading**: Uses Web Workers for processor threads (via `wasm_thread` crate)
5. **Buffer Management**: Output uses shared `Arc<Mutex<Vec<f32>>>` instead of cpal stream

### WASM Threading Requirements

The library uses the `wasm_thread` crate to provide Web Worker-based threading on WASM targets. This requires the following setup:

**SharedArrayBuffer Headers:**

WASM builds require `SharedArrayBuffer` support, which necessitates serving the application with COOP and COEP headers:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

**Main Thread Blocking:**

Blocking operations (like `JoinHandle::join()`) should not be called on the browser's main thread. The library's `Drop` implementations call `join()` on processor threads, so `AudioInputHandle` and `AudioOutputHandle` should be dropped from a Web Worker or async context, not the main thread.

**Thread Spawning Overhead:**

Thread spawning in WASM (Web Workers) has higher overhead than native threads. Once spawned, Web Workers provide true parallelism. The processor and callback threads are long-lived, so spawn overhead is amortized.

**Browser Compatibility:**

Minimum browser versions that support Web Workers with `SharedArrayBuffer`:
- Chrome 68+
- Firefox 79+
- Safari 15.2+
- Edge 79+

**Build Configuration:**

For WASM builds with atomics support, you may need nightly Rust and specific build flags. See `.cargo/config.toml` for recommended settings:

```toml
[target.wasm32-unknown-unknown]
rustflags = ["-C", "target-feature=+atomics,+bulk-memory,+mutable-globals"]

[unstable]
build-std = ["std", "panic_abort"]
```

## License

MIT License
