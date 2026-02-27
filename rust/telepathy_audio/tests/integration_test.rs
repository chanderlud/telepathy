//! Integration tests for telepathy_audio library.
//!
//! These tests verify the full workflow of device enumeration and selection.

#![cfg(not(target_family = "wasm"))]

use telepathy_audio::{
    AudioDeviceInfo, AudioDeviceList, AudioHost, DeviceError, get_default_input_device,
    get_default_output_device, get_input_device, get_output_device, list_all_devices,
    list_input_devices, list_output_devices,
};

/// Test full enumeration and selection workflow.
#[test]
fn test_full_enumeration_workflow() {
    // Create host
    let host = AudioHost::new();

    // Enumerate all devices
    let result = list_all_devices(&host);

    match result {
        Ok(devices) => {
            println!("Found {} input devices", devices.input_devices.len());
            println!("Found {} output devices", devices.output_devices.len());

            // If we have devices, try to select them
            for device_info in &devices.input_devices {
                let selected = get_input_device(&host, Some(&device_info.id));
                assert!(selected.is_ok() || matches!(selected, Err(DeviceError::NoDefaultDevice)));
            }

            for device_info in &devices.output_devices {
                let selected = get_output_device(&host, Some(&device_info.id));
                assert!(selected.is_ok() || matches!(selected, Err(DeviceError::NoDefaultDevice)));
            }
        }
        Err(DeviceError::EnumerationFailed(msg)) => {
            // This is acceptable on systems without audio devices
            println!("Enumeration failed (may be expected): {}", msg);
        }
        Err(e) => {
            panic!("Unexpected error: {:?}", e);
        }
    }
}

/// Test that default device selection works.
#[test]
fn test_default_device_selection() {
    let host = AudioHost::new();

    // Try to get default input device
    match get_default_input_device(&host) {
        Ok(handle) => {
            println!("Default input device: {:?}", handle.device_id());
            // Verify we can get the name
            if let Ok(name) = handle.name() {
                println!("Device name: {}", name);
            }
        }
        Err(DeviceError::NoDefaultDevice) => {
            println!("No default input device (acceptable on headless systems)");
        }
        Err(e) => {
            panic!("Unexpected error: {:?}", e);
        }
    }

    // Try to get default output device
    match get_default_output_device(&host) {
        Ok(handle) => {
            println!("Default output device: {:?}", handle.device_id());
            if let Ok(name) = handle.name() {
                println!("Device name: {}", name);
            }
        }
        Err(DeviceError::NoDefaultDevice) => {
            println!("No default output device (acceptable on headless systems)");
        }
        Err(e) => {
            panic!("Unexpected error: {:?}", e);
        }
    }
}

/// Test that AudioHost can be shared across threads.
#[test]
fn test_host_thread_safety() {
    use std::thread;

    let host = AudioHost::new();
    let host_clone = host.clone();

    let handle = thread::spawn(move || {
        // Use the cloned host in another thread
        let _ = list_input_devices(&host_clone);
        let _ = list_output_devices(&host_clone);
    });

    // Use the original host in this thread
    let _ = list_all_devices(&host);

    // Wait for the other thread
    handle.join().expect("Thread panicked");
}

/// Test device info structure behavior.
#[test]
fn test_device_info_debug_and_clone() {
    let info = AudioDeviceInfo {
        name: "Test Microphone".to_string(),
        id: "test-mic-001".to_string(),
    };

    // Test Debug trait
    let debug_str = format!("{:?}", info);
    assert!(debug_str.contains("Test Microphone"));
    assert!(debug_str.contains("test-mic-001"));

    // Test Clone trait
    let cloned = info.clone();
    assert_eq!(info.name, cloned.name);
    assert_eq!(info.id, cloned.id);
    assert_eq!(info, cloned);
}

/// Test AudioDeviceList structure.
#[test]
fn test_device_list_structure() {
    let list = AudioDeviceList {
        input_devices: vec![
            AudioDeviceInfo {
                name: "Mic 1".to_string(),
                id: "mic-1".to_string(),
            },
            AudioDeviceInfo {
                name: "Mic 2".to_string(),
                id: "mic-2".to_string(),
            },
        ],
        output_devices: vec![AudioDeviceInfo {
            name: "Speaker 1".to_string(),
            id: "speaker-1".to_string(),
        }],
    };

    assert_eq!(list.input_devices.len(), 2);
    assert_eq!(list.output_devices.len(), 1);

    // Test Clone
    let cloned = list.clone();
    assert_eq!(cloned.input_devices.len(), 2);
    assert_eq!(cloned.output_devices.len(), 1);
}

/// Test error types.
#[test]
fn test_error_types() {
    let errors: Vec<DeviceError> = vec![
        DeviceError::DeviceNotFound("missing-device".to_string()),
        DeviceError::NoDefaultDevice,
        DeviceError::EnumerationFailed("backend error".to_string()),
        DeviceError::InvalidDeviceId("malformed-id".to_string()),
    ];

    for error in &errors {
        // Test Display trait
        let display = format!("{}", error);
        assert!(!display.is_empty());

        // Test Debug trait
        let debug = format!("{:?}", error);
        assert!(!debug.is_empty());

        // Test Clone trait
        let cloned = error.clone();
        assert_eq!(format!("{}", error), format!("{}", cloned));
    }

    // Verify specific error messages
    assert!(format!("{}", DeviceError::DeviceNotFound("xyz".to_string())).contains("xyz"));
    assert!(format!("{}", DeviceError::NoDefaultDevice).contains("default"));
    assert!(format!("{}", DeviceError::EnumerationFailed("err".to_string())).contains("err"));
    assert!(format!("{}", DeviceError::InvalidDeviceId("bad".to_string())).contains("bad"));
}

/// Test that separate enumeration functions work independently.
#[test]
fn test_separate_enumeration() {
    let host = AudioHost::new();

    // Call enumeration functions separately
    let input_result = list_input_devices(&host);
    let output_result = list_output_devices(&host);

    // Both should either succeed or fail with EnumerationFailed
    match (&input_result, &output_result) {
        (Ok(inputs), Ok(outputs)) => {
            println!("Input devices: {:?}", inputs.len());
            println!("Output devices: {:?}", outputs.len());
        }
        (Err(e1), Err(e2)) => {
            println!("Both failed (may be expected): {:?}, {:?}", e1, e2);
        }
        (Ok(_), Err(e)) | (Err(e), Ok(_)) => {
            // One succeeded, one failed - might happen on some systems
            println!("Partial success/failure: {:?}", e);
        }
    }
}
