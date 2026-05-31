/// In-process audio host for testing and integration without real hardware.
///
/// `MockAudioHost<I, O="">` implements [`AudioHost`] using caller-supplied
/// [`AudioInput`] and [`AudioOutput`] implementations, making it suitable
/// for unit and integration tests that must run without a physical audio device.
///
/// Both `I` and `O` must be `Clone` because `open_input` / `open_output`
/// clone the stored value on each call.
///
/// # Example
///
/// ```rust,no_run
/// use telepathy_audio::devices::{MockAudioHost, MockAudioInput, MockAudioOutput};
///
/// let host = MockAudioHost::new(
///     MockAudioInput::default(),
///     48_000,
///     MockAudioOutput::default(),
///     48_000,
/// );
/// ```
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
    /// Creates a new mock host with the given input/output implementations and sample rates.
    ///
    /// # Arguments
    ///
    /// * `input` - The [`AudioInput`] implementation to clone on each `open_input` call
    /// * `input_rate` - Sample rate reported for the mock input device (Hz)
    /// * `output` - The [`AudioOutput`] implementation to clone on each `open_output` call
    /// * `output_rate` - Sample rate reported for the mock output device (Hz)
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

/// In-process audio input that emits silence at real-time pace.
#[derive(Debug, Clone)]
pub struct MockAudioInput {
    sample_rate: u32,
}

impl MockAudioInput {
    /// Creates a new mock input that emits silence at the given sample rate.
    ///
    /// # Arguments
    ///
    /// * `sample_rate` - The sample rate used to calculate real-time sleep duration per frame
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
