---
title: Adapter-Safe Video Capabilities Separate Runtime Truth From Configuration
date: 2026-07-30
category: docs/solutions/architecture-patterns/
module: telepathy-core video platform
problem_type: architecture_pattern
component: tooling
severity: high
applies_when:
  - A target-specific adapter exposes serializable device or codec settings
  - A compiled adapter has target-dependent command implementations
  - Receiver playback must match a negotiated media format
related_components:
  - flutter-rust-bridge
  - testing-framework
tags: [video-sessions, capabilities, platform-adapters, ffmpeg, configuration]
---

# Adapter-Safe Video Capabilities Separate Runtime Truth From Configuration

## Context

Generic video sessions retain serializable internal `Device`, `Encoder`, and `Decoder` values, but a stored value does not prove that the current target can run it. The selected adapter must report what this runtime can actually start or receive.

## Guidance

Select one private adapter at compile time in `rust/telepathy-core/src/internal/video/platform.rs`: the desktop FFmpeg adapter for Windows, macOS, and Linux, or the unsupported adapter elsewhere. Keep the coordinator and public contract independent of that selection.

Treat the fresh adapter probe as the source of capability truth. `desktop_ffmpeg::video_capabilities` advertises a display source only when it has both a device and an encoder result. `Device::devices` currently supplies capture devices only on Windows, while `Device::to_args` returns a typed platform-unavailable error for unimplemented paths. Therefore a desktop build on macOS or Linux must not advertise display capture merely because the adapter compiled or a `RecordingConfig` can deserialize.

Validate the selected configuration again in `desktop_ffmpeg::prepare_sender`. Missing current devices or encoders return `VideoUnavailable::ConfigurationUnavailable`; missing source formats return the corresponding typed unavailability. The unsupported adapter returns `VideoUnavailable::PlatformUnsupported` for capabilities and sender preparation.

For playback, `desktop_ffmpeg::run_receiver` probes locally immediately before startup. `select_decoder` chooses the first decoder from that fresh local probe list whose codec matches the negotiated `VideoMediaDescriptor`. Decoder preference is local probe order, not sender configuration or a global cross-platform order.

## Why This Matters

Configuration is stable enough to save and present later. Capability is a statement about the current binary, target, installed FFmpeg components, and implemented command paths. Combining them can advertise a mode that cannot start, then turn a user action or restored setting into a panic or a false success.

Keeping the distinction inside the adapter lets the generic session lifecycle remain target-neutral while still reporting precise typed outcomes to native and Flutter callers.

## When to Apply

- A target-specific implementation compiles on more targets than it fully supports.
- Device, encoder, decoder, permission, or binary availability can change after settings are stored.
- A receiver must choose a local implementation compatible with peer-negotiated media.

## Examples

The adapter keeps unavailable configuration on the typed path rather than starting a process:

```rust
if !capabilities.encoders.contains(&config.encoder)
    || !capabilities.devices.contains(&config.device)
{
    return Err(VideoUnavailable::ConfigurationUnavailable);
}
```

The verified adapter tests cover the same boundary: `unimplemented_device_returns_typed_error_without_panicking`, `sender_start_rejects_encoder_removed_after_preflight`, `sender_start_rejects_device_removed_after_preflight`, and `decoder_selection_uses_first_compatible_local_decoder` in `rust/telepathy-core/src/internal/video/platform/desktop_ffmpeg.rs`. `unsupported_adapter_query_and_start_report_typed_unavailable` in `rust/telepathy-core/src/internal/video/platform.rs` covers the selected unsupported contract.

## Related

- [Joined Video Session Teardown Keeps Slots Safe for Reuse](joined-video-session-teardown.md) covers lifecycle ownership after a session has started.
- The generic video-session implementation plan is `docs/plans/2026-07-17-001-refactor-generic-video-sessions-plan.md`.
