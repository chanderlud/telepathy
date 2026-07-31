/// In-process [`AudioHost`] backed by caller-supplied [`AudioInput`] / [`AudioOutput`] impls.
///
/// `I` and `O` must be `Clone` because `open_input` / `open_output` clone the
/// stored value on each call. Used by tests that need to run without a physical
/// audio device.
use crate::Error;
use crate::devices::{AudioDeviceInfo, AudioDeviceList, AudioHost, DeviceError};
use crate::internal::traits::{AudioInput, AudioOutput};
use crate::io::StreamErrorCallback;
use std::thread;
use std::time::Duration;

const DEFAULT_SAMPLE_RATE: u32 = 48_000;
const MOCK_DEVICE_ID: &str = "mock";

#[derive(Debug, Clone)]
pub struct MockAudioHost<I, O> {
    input: I,
    input_rate: u32,
    output: O,
    output_rate: u32,
}

impl<I, O> MockAudioHost<I, O> {
    /// Creates a new mock host that reports the supplied sample rates and
    /// clones `input` / `output` for every `open_input` / `open_output` call.
    pub fn new(input: I, input_rate: u32, output: O, output_rate: u32) -> Self {
        Self {
            input,
            input_rate,
            output,
            output_rate,
        }
    }
}

impl<I, O> AudioHost for MockAudioHost<I, O>
where
    I: AudioInput + Send + Clone + 'static,
    O: AudioOutput + Send + Clone + 'static,
{
    type InputStream = ();
    type OutputStream = ();

    fn list_input_devices(&self) -> Result<Vec<AudioDeviceInfo>, DeviceError> {
        Ok(vec![AudioDeviceInfo {
            name: "Mock Input".to_string(),
            id: MOCK_DEVICE_ID.to_string(),
        }])
    }

    fn list_output_devices(&self) -> Result<Vec<AudioDeviceInfo>, DeviceError> {
        Ok(vec![AudioDeviceInfo {
            name: "Mock Output".to_string(),
            id: MOCK_DEVICE_ID.to_string(),
        }])
    }

    fn list_all_devices(&self) -> Result<AudioDeviceList, DeviceError> {
        Ok(AudioDeviceList {
            input_devices: self.list_input_devices()?,
            output_devices: self.list_output_devices()?,
        })
    }

    fn input_sample_rate(&self, _: Option<&str>) -> Result<u32, DeviceError> {
        Ok(self.input_rate)
    }

    fn output_sample_rate(&self, _: Option<&str>) -> Result<u32, DeviceError> {
        Ok(self.output_rate)
    }

    #[cfg(not(target_family = "wasm"))]
    fn open_input(
        &self,
        _: Option<&str>,
        _: Option<StreamErrorCallback>,
    ) -> Result<(impl AudioInput + Send + 'static, u32, Self::InputStream), DeviceError> {
        Ok((self.input.clone(), self.input_rate, ()))
    }

    fn open_output(
        &self,
        _: Option<&str>,
        _: Option<StreamErrorCallback>,
    ) -> Result<(impl AudioOutput + Send + 'static, u32, Self::OutputStream), DeviceError> {
        Ok((self.output.clone(), self.output_rate, ()))
    }
}

impl<I: Default, O: Default> Default for MockAudioHost<I, O> {
    fn default() -> Self {
        Self {
            input: Default::default(),
            input_rate: DEFAULT_SAMPLE_RATE,
            output: Default::default(),
            output_rate: DEFAULT_SAMPLE_RATE,
        }
    }
}

/// In-process audio input that emits a deterministic signal at real-time pace.
#[derive(Debug, Clone)]
pub struct MockAudioInput {
    sample_rate: u32,
    sample_index: u64,
}

impl MockAudioInput {
    /// Creates a new mock input that emits changing non-silent samples at the given sample rate.
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            sample_index: 0,
        }
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
        for sample in dst.iter_mut() {
            let ramp_position = (self.sample_index % 96) as f32 / 95.0;
            *sample = (ramp_position * 2.0 - 1.0) * 0.25;
            self.sample_index = self.sample_index.wrapping_add(1);
        }
        Ok(dst.len())
    }
}

/// In-process audio output that discards all samples.
#[derive(Default, Debug, Clone)]
pub struct MockAudioOutput;

impl AudioOutput for MockAudioOutput {
    fn is_full(&self) -> bool {
        false
    }

    fn write_samples(&mut self, _samples: &[f32]) -> Result<usize, Error> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_audio_input_emits_changing_non_silent_samples() {
        let mut input = MockAudioInput::new(1_000_000);
        let mut first = [0.0; 4];
        let mut second = [0.0; 4];

        let first_read = input.read_into(&mut first).unwrap();
        let second_read = input.read_into(&mut second).unwrap();

        assert_eq!(first_read, first.len());
        assert_eq!(second_read, second.len());
        assert!(first.iter().any(|sample| *sample != 0.0));
        assert!(second.iter().any(|sample| *sample != 0.0));
        assert_ne!(first, second);
    }
}
