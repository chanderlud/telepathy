use crate::devices::{AudioDeviceInfo, AudioDeviceList, AudioHost, DeviceError, DeviceHandle};
use crate::error::Error;
use crate::internal::traits::{AudioInput, AudioOutput};
use std::thread;
use std::time::Duration;

const DEFAULT_SAMPLE_RATE: u32 = 48_000;

/// In-process audio input that emits silence at real-time pace.
#[derive(Debug)]
pub struct MockAudioInput {
    sample_rate: u32,
}

impl MockAudioInput {
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }
}

impl Default for MockAudioInput {
    fn default() -> Self {
        Self::new(DEFAULT_SAMPLE_RATE)
    }
}

impl AudioInput for MockAudioInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, Error> {
        let frame_seconds = dst.len() as f64 / self.sample_rate as f64;
        if frame_seconds.is_normal() || frame_seconds > 0.0 {
            thread::sleep(Duration::from_secs_f64(frame_seconds));
        }
        dst.fill(0.0);
        Ok(dst.len())
    }
}

/// In-process audio output that discards all samples.
#[derive(Default, Debug)]
pub struct MockAudioOutput;

impl AudioOutput for MockAudioOutput {
    fn is_full(&self) -> bool {
        false
    }

    fn write_samples(&mut self, _samples: &[f32]) -> Result<usize, Error> {
        Ok(0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct MockAudioHost;

impl AudioHost for MockAudioHost {
    fn new() -> Self
    where
        Self: Sized,
    {
        Self
    }

    fn list_input_devices(&self) -> Result<Vec<AudioDeviceInfo>, DeviceError> {
        Ok(vec![])
    }

    fn list_output_devices(&self) -> Result<Vec<AudioDeviceInfo>, DeviceError> {
        Ok(vec![])
    }

    fn list_all_devices(&self) -> Result<AudioDeviceList, DeviceError> {
        Ok(AudioDeviceList {
            input_devices: vec![],
            output_devices: vec![],
        })
    }

    fn get_input_device(&self, _device_id: Option<&str>) -> Result<DeviceHandle, DeviceError> {
        Err(DeviceError::NoDefaultDevice)
    }

    fn get_output_device(&self, _device_id: Option<&str>) -> Result<DeviceHandle, DeviceError> {
        Err(DeviceError::NoDefaultDevice)
    }

    fn get_default_input_device(&self) -> Result<DeviceHandle, DeviceError> {
        Err(DeviceError::NoDefaultDevice)
    }

    fn get_default_output_device(&self) -> Result<DeviceHandle, DeviceError> {
        Err(DeviceError::NoDefaultDevice)
    }
}
