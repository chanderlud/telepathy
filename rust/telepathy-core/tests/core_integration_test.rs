#![cfg(feature = "integration-testing")]

#[path = "core_integration_test/audio_streams.rs"]
mod audio_streams;
#[path = "core_integration_test/call_end_copy.rs"]
mod call_end_copy;
#[path = "core_integration_test/call_lifecycle.rs"]
mod call_lifecycle;
#[path = "core_integration_test/common.rs"]
mod common;
#[path = "core_integration_test/device_failures.rs"]
mod device_failures;
#[path = "core_integration_test/identity_switch.rs"]
mod identity_switch;
#[path = "core_integration_test/room_lifecycle.rs"]
mod room_lifecycle;
#[path = "core_integration_test/runtime_readiness.rs"]
mod runtime_readiness;
#[path = "core_integration_test/session_lifecycle.rs"]
mod session_lifecycle;
