use crate::api::error::{Error, ErrorKind};
use crate::api::telepathy::{DeviceName, Transport};
use bincode::config::standard;
use bincode::{Decode, Encode, decode_from_slice, encode_to_vec};
use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, Host, Stream};
use flutter_rust_bridge::for_generated::futures::{Sink, SinkExt};
use kanal::AsyncReceiver;
use libp2p::bytes::Bytes;
use libp2p::futures::StreamExt;
use rubato::{SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use serde::Deserialize;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering::Relaxed;
use tokio::io::{AsyncRead, AsyncWrite};

type Result<T> = std::result::Result<T, Error>;

/// Parameters used for resampling throughout the application
const RESAMPLER_PARAMETERS: SincInterpolationParameters = SincInterpolationParameters {
    sinc_len: 256,
    f_cutoff: 0.95,
    interpolation: SincInterpolationType::Linear,
    oversampling_factor: 256,
    window: WindowFunction::BlackmanHarris2,
};

/// wraps a cpal stream to unsafely make it send
pub(crate) struct SendStream {
    pub(crate) stream: Stream,
}

/// Safety: SendStream must not be used across awaits
unsafe impl Send for SendStream {}

/// mul with internal selection of optimal implementation
pub(crate) fn wide_mul(frame: &mut [f32], factor: f32) {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") && frame.len().is_multiple_of(16) {
            unsafe { mul_simd_avx512(frame, factor) }
            return;
        }
        if is_x86_feature_detected!("avx2") && frame.len().is_multiple_of(8) {
            unsafe { mul_simd_avx2(frame, factor) }
            return;
        }
    }

    scalar_mul(frame, factor);
}

/// mul for any length input
fn scalar_mul(frame: &mut [f32], factor: f32) {
    for p in frame.iter_mut() {
        *p *= factor;
        *p = p.clamp(-1_f32, 1_f32);
    }
}

/// optimized mul for avx2 with 8|frame.len==true
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn mul_simd_avx2(frame: &mut [f32], factor: f32) {
    let len = frame.len();
    let mut i = 0;

    let factor_vec = _mm256_set1_ps(factor);
    let min_vec = _mm256_set1_ps(-1_f32);
    let max_vec = _mm256_set1_ps(1_f32);

    while i + 8 <= len {
        let mut chunk = _mm256_loadu_ps(frame.as_ptr().add(i)); // load
        chunk = _mm256_mul_ps(chunk, factor_vec); // multiply
        chunk = _mm256_max_ps(min_vec, _mm256_min_ps(max_vec, chunk)); // clamp
        _mm256_storeu_ps(frame.as_mut_ptr().add(i), chunk); // write
        i += 8;
    }
}

/// optimized mul for avx512f with 16|frame.len==true
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn mul_simd_avx512(frame: &mut [f32], factor: f32) {
    let len = frame.len();
    let mut i = 0;

    let factor_vec = _mm512_set1_ps(factor);
    let min_vec = _mm512_set1_ps(-1_f32);
    let max_vec = _mm512_set1_ps(1_f32);

    // process 16 floats per iteration
    while i + 16 <= len {
        let mut chunk = _mm512_loadu_ps(frame.as_ptr().add(i)); // load
        chunk = _mm512_mul_ps(chunk, factor_vec); // multiply
        chunk = _mm512_min_ps(max_vec, _mm512_max_ps(min_vec, chunk)); // clamp
        _mm512_storeu_ps(frame.as_mut_ptr().add(i), chunk); // write
        i += 16;
    }
}

pub(crate) fn i16_to_f32_scalar(ints: &[i16], out: &mut [f32], scale: f32) {
    for (out, &x) in out.iter_mut().zip(ints.iter()) {
        *out = x as f32 * scale;
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "avx2")]
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn i16_to_f32_avx2(ints: &[i16], out: &mut [f32], scale: f32) {
    use core::arch::x86_64::*;
    let n = ints.len();
    let mut i = 0;

    let scale_ps = _mm256_set1_ps(scale);

    // process 16 i16 -> 16 f32 per loop
    while i + 16 <= n {
        // load 16 i16
        let v16 = _mm256_loadu_si256(ints.as_ptr().add(i) as *const __m256i);

        // lower 8 i16 -> 8 i32 -> 8 f32
        let lo16 = _mm256_castsi256_si128(v16);
        let lo32 = _mm256_cvtepi16_epi32(lo16);
        let lo_ps = _mm256_mul_ps(_mm256_cvtepi32_ps(lo32), scale_ps);

        // upper 8 i16 -> 8 i32 -> 8 f32
        let hi16 = _mm256_extracti128_si256::<1>(v16);
        let hi32 = _mm256_cvtepi16_epi32(hi16);
        let hi_ps = _mm256_mul_ps(_mm256_cvtepi32_ps(hi32), scale_ps);

        // store 16 f32
        _mm256_storeu_ps(out.as_mut_ptr().add(i), lo_ps);
        _mm256_storeu_ps(out.as_mut_ptr().add(i + 8), hi_ps);

        i += 16;
    }
}

/// calculates the RMS of the frame (loop is unrolled for optimization)
pub(crate) fn calculate_rms(data: &[f32]) -> f32 {
    let mut sum1 = 0.0;
    let mut sum2 = 0.0;
    let mut sum3 = 0.0;
    let mut sum4 = 0.0;

    let mut i = 0;
    while i + 3 < data.len() {
        sum1 += data[i] * data[i];
        sum2 += data[i + 1] * data[i + 1];
        sum3 += data[i + 2] * data[i + 2];
        sum4 += data[i + 3] * data[i + 3];
        i += 4;
    }

    let mean_of_squares = (sum1 + sum2 + sum3 + sum4) / data.len() as f32;
    mean_of_squares.sqrt()
}

/// converts a decibel value to a multiplier
pub(crate) fn db_to_multiplier(db: f32) -> f32 {
    10_f32.powf(db / 20_f32)
}

/// Produces a resampler if needed
pub(crate) fn resampler_factory(
    ratio: f64,
    channels: usize,
    size: usize,
) -> Result<Option<SincFixedIn<f32>>> {
    if ratio == 1_f64 {
        Ok(None)
    } else {
        // create the resampler if needed
        Ok(Some(SincFixedIn::<f32>::new(
            ratio,
            2_f64,
            RESAMPLER_PARAMETERS,
            size,
            channels,
        )?))
    }
}

/// Gets the output device
pub(crate) async fn get_output_device(
    output_device: &DeviceName,
    host: &Arc<Host>,
) -> Result<Device> {
    match *output_device.lock().await {
        Some(ref name) => Ok(host
            .output_devices()?
            .find(|device| {
                if let Ok(ref device_name) = device.name() {
                    name == device_name
                } else {
                    false
                }
            })
            .unwrap_or(
                host.default_output_device()
                    .ok_or(ErrorKind::NoOutputDevice)?,
            )),
        None => host
            .default_output_device()
            .ok_or(ErrorKind::NoOutputDevice.into()),
    }
}

/// Returns the percentage of the max input volume in the window compared to the max volume
pub(crate) async fn level_from_window(receiver: &AsyncReceiver<f32>, max: &mut f32) -> f32 {
    let mut window = Vec::new();

    while let Ok(Some(rms)) = receiver.try_recv() {
        window.push(rms);
    }

    let level = if window.is_empty() {
        0_f32
    } else {
        let local_max = window.into_iter().reduce(f32::max).unwrap_or(0_f32);
        *max = max.max(local_max);

        if *max != 0_f32 {
            local_max / *max
        } else {
            0_f32
        }
    };

    if level < 0.01 { 0_f32 } else { level }
}

/// Writes a bincode message to the stream
pub(crate) async fn write_message<M: Encode, W>(
    transport: &mut Transport<W>,
    message: &M,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
    Transport<W>: Sink<Bytes> + Unpin,
{
    let buffer = encode_to_vec(message, standard())?;

    transport
        .send(Bytes::from(buffer))
        .await
        .map_err(|_| ErrorKind::TransportSend)
        .map_err(Into::into)
}

/// Reads a bincode message from the stream
pub(crate) async fn read_message<M: Decode<()>, R: AsyncRead + Unpin>(
    transport: &mut Transport<R>,
) -> Result<M> {
    if let Some(Ok(buffer)) = transport.next().await {
        // TODO could decode from slice borrowed be used here to potentially avoid copying
        let (message, _) = decode_from_slice(&buffer[..], standard())?; // decode the message
        Ok(message)
    } else {
        Err(ErrorKind::TransportRecv.into())
    }
}

pub(crate) fn atomic_u32_serialize<S>(
    value: &Arc<AtomicU32>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let value = value.load(Relaxed);
    serializer.serialize_u32(value)
}

pub(crate) fn atomic_u32_deserialize<'de, D>(
    deserializer: D,
) -> std::result::Result<Arc<AtomicU32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u32::deserialize(deserializer)?;
    Ok(Arc::new(AtomicU32::new(value)))
}

pub(crate) mod rwlock_option_recording_config {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::api::flutter::RecordingConfig;

    pub fn serialize<S>(
        value: &Arc<RwLock<Option<RecordingConfig>>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let lock = value.blocking_read();
        lock.serialize(serializer)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<Arc<RwLock<Option<RecordingConfig>>>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let inner = Option::<RecordingConfig>::deserialize(deserializer)?;
        Ok(Arc::new(RwLock::new(inner)))
    }
}

#[cfg(test)]
mod tests {
    use crate::api::utils::i16_to_f32_scalar;
    use nnnoiseless::FRAME_SIZE;

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn mul_normal_frame_equal_outputs() {
        let frame = crate::api::telepathy::tests::dummy_frame();
        let mut scalar_frame = frame.clone();
        let mut simd_avx2_frame = frame.clone();
        let mut wide_frame = frame.clone();
        // let mut simd_avx512_frame = frame.clone();

        super::scalar_mul(&mut scalar_frame, 200_f32);
        unsafe {
            super::mul_simd_avx2(&mut simd_avx2_frame, 200_f32);
        }
        super::wide_mul(&mut wide_frame, 200_f32);

        // unsafe {
        //     super::mul_simd_avx512(&mut simd_avx512_frame, 2_f32);
        // }

        assert_eq!(scalar_frame, simd_avx2_frame);
        assert_eq!(wide_frame, simd_avx2_frame);
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn int_conversion_equal_outputs() {
        let frame = crate::api::telepathy::tests::dummy_int_frame();
        let mut scalar_frame = [0_f32; FRAME_SIZE];
        let mut simd_avx2_frame = [0_f32; FRAME_SIZE];
        let scale = 1_f32 / i16::MAX as f32;

        unsafe { super::i16_to_f32_avx2(&frame, &mut simd_avx2_frame, scale) };

        i16_to_f32_scalar(&frame, &mut scalar_frame, scale);

        println!("{:?}", scalar_frame);
        println!("{:?}", simd_avx2_frame);
        println!("{:?}", frame);

        assert_eq!(scalar_frame, simd_avx2_frame);
    }
}
