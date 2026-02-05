# Telepathy Audio Library

A standalone audio processing library for the Telepathy project, providing device management, audio capture, playback, and codec support.

## Features

- **Device Management**: Enumerate and select audio input/output devices across platforms
- **Audio Capture**: High-quality audio input with optional RNNoise noise suppression
- **Audio Playback**: Low-latency audio output with automatic resampling
- **Codec Support**: SEA codec encoding/decoding for efficient network transmission
- **SIMD Optimization**: Hardware-accelerated audio processing with automatic CPU feature detection
  - AVX-512 for 16-element aligned frames (where supported)
  - AVX2 for 8-element aligned frames (where supported)
  - Scalar fallback for all other cases
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
    .codec(true, false, 5.0)  // enabled, VBR disabled, 5 residual bits
    .callback(|encoded_data| {
        // Send encoded data over network
    })
    .build(&host)
    .unwrap();

// Output with codec decoding
let output = AudioOutputBuilder::new()
    .codec(true, None)  // enabled, no pre-defined header
    .build(&host)
    .unwrap();
```

> **Note on Sample Rates**: When noise suppression (`denoise`) is enabled, the input
> processor upsamples to 48kHz for RNNoise processing and outputs 48kHz frames. The
> encoder sample rate automatically matches this. When denoise is disabled, the
> processor passes through at the device's native sample rate.

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
- `ProcessorMessage` - Message type for inter-thread communication
- `SeaFileHeader` - SEA codec file header structure

## Dependencies

This library builds on several excellent Rust crates:

- [cpal](https://docs.rs/cpal) - Cross-platform audio I/O
- [rubato](https://docs.rs/rubato) - High-quality audio resampling
- [nnnoiseless](https://docs.rs/nnnoiseless) - RNNoise-based noise suppression
- [sea_codec](https://github.com/Daninet/sea-codec) - SEA audio codec for efficient transmission

## License

MIT License
