//! Web Audio API integration for WASM targets.
//!
//! This module provides audio input support for web browsers using the
//! Web Audio API and AudioWorklet. It enables microphone capture in web
//! applications with low latency.
//!
//! ## Architecture
//!
//! ```text
//! ┌───────────────┐    ┌──────────────────┐    ┌──────────────────┐
//! │ Microphone    │───▶│ AudioWorklet     │───▶│ WebAudioWrapper  │
//! │ (getUserMedia)│    │ (JS processor)   │    │ (Rust)           │
//! └───────────────┘    └──────────────────┘    └──────────────────┘
//!                              │
//!                              │ postMessage (Float32Array)
//!                              ▼
//!                      ┌──────────────────┐    ┌──────────────────┐
//!                      │ Shared Buffer    │───▶│ WebAudioInput    │
//!                      │ (Mutex<Vec<f32>>)│    │ (AudioInput impl)│
//!                      └──────────────────┘    └──────────────────┘
//! ```
//!
//! ## Browser Compatibility
//!
//! - **Chrome 66+**: Full AudioWorklet support
//! - **Firefox 76+**: Full AudioWorklet support
//! - **Safari 14.1+**: Full AudioWorklet support
//! - **Edge 79+**: Full AudioWorklet support (Chromium-based)
//!
//! Older browsers without AudioWorklet support will fail during initialization.
//!
//! ## Async Initialization Requirement
//!
//! Web Audio API requires async initialization because:
//! 1. `getUserMedia()` requires user permission (async prompt)
//! 2. `audioWorklet.addModule()` loads JavaScript module asynchronously
//! 3. Audio context may start suspended and require user gesture to resume
//!
//! ## Implementation Notes
//!
//! Based on the workaround from [cpal issue #813](https://github.com/RustAudio/cpal/issues/813#issuecomment-2413007276).
//! The AudioWorklet processor is embedded as inline JavaScript and loaded via Blob URL.
//!
//! ## Usage
//!
//! Create a [`WebAudioWrapper`] via [`WebAudioWrapper::new()`] (async, must be called on the
//! main thread during a user interaction), then pass it to
//! [`AudioInputBuilder::web_audio_wrapper()`](crate::AudioInputBuilder::web_audio_wrapper)
//! before calling [`AudioInputBuilder::build()`](crate::AudioInputBuilder::build).

#![cfg(target_family = "wasm")]

use crate::error::AudioError;
use crate::internal::traits::{AudioInput, CHANNEL_SIZE};
use log::error;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
use wasm_bindgen_futures::JsFuture;
use wasm_sync::{Condvar, Mutex};
use web_sys::BlobPropertyBag;

/// Wrapper for Web Audio API input handling.
///
/// This struct manages the audio context, media stream, and AudioWorklet
/// for capturing audio input in a web browser. It handles all the JavaScript
/// interop required for low-latency microphone capture.
///
/// ## Lifecycle
///
/// 1. **Creation** ([`new`](Self::new)): Async initialization
///    - Creates AudioContext
///    - Requests microphone permission via `getUserMedia`
///    - Registers inline AudioWorklet processor
///    - Connects audio graph
///
/// 2. **Running**: Audio flows through worklet to shared buffer
///    - AudioWorklet receives audio in real-time
///    - Posts Float32Array samples via `postMessage`
///    - Rust closure pushes to shared buffer
///    - [`WebAudioInput`] reads from buffer
///
/// 3. **Cleanup** ([`Drop`]): Disconnects and releases resources
///    - Clears message handler
///    - Disconnects audio graph
///    - Stops microphone tracks
///    - Closes AudioContext
///
/// ## Thread Safety
///
/// Implements `Send` and `Sync` despite containing JS objects. The JavaScript
/// objects are only accessed from the browser's main thread through
/// wasm-bindgen's safety guarantees.
///
/// ## Implementation Notes
///
/// Based on the workaround from [cpal issue #813](https://github.com/RustAudio/cpal/issues/813#issuecomment-2413007276).
pub struct WebAudioWrapper {
    pair: Arc<(Mutex<Vec<f32>>, Condvar)>,
    finished: Arc<AtomicBool>,
    pub sample_rate: f32,
    audio_ctx: web_sys::AudioContext,
    _source: web_sys::MediaStreamAudioSourceNode,
    _media_devices: web_sys::MediaDevices,
    _stream: web_sys::MediaStream,
    _js_closure: Closure<dyn FnMut(JsValue)>,
    _worklet_node: web_sys::AudioWorkletNode,
}

impl WebAudioWrapper {
    /// Creates a new WebAudioWrapper and initializes the audio input.
    ///
    /// This async method performs the complete audio initialization sequence:
    ///
    /// 1. Creates an AudioContext with default sample rate
    /// 2. Requests microphone access via `navigator.mediaDevices.getUserMedia`
    /// 3. Creates inline AudioWorklet processor (embedded JavaScript)
    /// 4. Registers processor via Blob URL
    /// 5. Connects: MediaStreamSource → AudioWorkletNode
    /// 6. Sets up message handler for receiving audio data
    ///
    /// # Errors
    ///
    /// Returns `JsValue` error if:
    /// - Window or navigator not available
    /// - Microphone permission denied by user
    /// - AudioWorklet registration fails
    /// - Audio graph connection fails
    ///
    /// # Browser Behavior
    ///
    /// - Will show browser permission prompt for microphone access
    /// - AudioContext starts in suspended state in some browsers
    /// - Call [`resume`](Self::resume) after user interaction if needed
    pub async fn new() -> Result<Self, JsValue> {
        let audio_ctx = web_sys::AudioContext::new()?;
        let sample_rate = audio_ctx.sample_rate();

        let media_devices = web_sys::window()
            .ok_or(JsValue::from_str("unable to get window"))?
            .navigator()
            .media_devices()?;

        let constraints = web_sys::MediaStreamConstraints::new();
        constraints.set_audio(&JsValue::TRUE);

        let stream_promise = media_devices.get_user_media_with_constraints(&constraints)?;
        let stream_value = JsFuture::from(stream_promise).await?;
        let stream = stream_value.dyn_into::<web_sys::MediaStream>()?;
        let source = audio_ctx.create_media_stream_source(&stream)?;

        // Return about Float32Array
        // return first input's first channel's samples
        // https://developer.mozilla.org/ja/docs/Web/API/AudioWorkletProcessor/process
        let processor_js_code = r#"
            class TelepathyProcessor extends AudioWorkletProcessor {
                process(inputs, outputs, parameters) {
                    this.port.postMessage(Float32Array.from(inputs[0][0]));

                    return true;
                }
            }

            registerProcessor('telepathy-processor', TelepathyProcessor);
        "#;

        let blob_parts = js_sys::Array::new();
        blob_parts.push(&JsValue::from_str(processor_js_code));

        let type_: BlobPropertyBag = BlobPropertyBag::new();
        type_.set_type("application/javascript");

        let blob = web_sys::Blob::new_with_str_sequence_and_options(&blob_parts, &type_)?;

        let url = web_sys::Url::create_object_url_with_blob(&blob)?;

        let processor = audio_ctx.audio_worklet()?.add_module(&url)?;

        JsFuture::from(processor).await?;

        web_sys::Url::revoke_object_url(&url)?;

        let worklet_node = web_sys::AudioWorkletNode::new(&audio_ctx, "telepathy-processor")?;

        source.connect_with_audio_node(&worklet_node)?;

        let pair: Arc<(Mutex<Vec<f32>>, _)> = Arc::new((Mutex::default(), Condvar::new()));
        let pair_clone = Arc::clone(&pair);

        // Float32Array
        let js_closure = Closure::wrap(Box::new(move |msg: JsValue| {
            let data_result: Result<Result<Vec<f32>, _>, _> = msg
                .dyn_into::<web_sys::MessageEvent>()
                .map(|msg| serde_wasm_bindgen::from_value(msg.data()));

            match (data_result, pair_clone.0.lock()) {
                (Ok(Ok(data)), Ok(mut data_clone)) => {
                    if data_clone.len() > CHANNEL_SIZE {
                        return;
                    }

                    data_clone.extend(data);
                    pair_clone.1.notify_one();
                }
                (Err(error), _) => error!("failed to handle worker message: {:?}", error),
                (Ok(Err(error)), _) => error!("failed to handle worker message: {:?}", error),
                (_, Err(error)) => error!("failed to lock pair: {}", error),
            }
        }) as Box<dyn FnMut(JsValue)>);

        let js_func = js_closure.as_ref().unchecked_ref();

        worklet_node.port()?.set_onmessage(Some(js_func));

        Ok(WebAudioWrapper {
            pair,
            finished: Default::default(),
            sample_rate,
            audio_ctx,
            _source: source,
            _media_devices: media_devices,
            _stream: stream,
            _js_closure: js_closure,
            _worklet_node: worklet_node,
        })
    }

    /// Resumes the audio context if it was suspended.
    pub fn resume(&self) {
        _ = self.audio_ctx.resume();
    }
}

impl Drop for WebAudioWrapper {
    fn drop(&mut self) {
        // stop JS from ever calling the Rust closure again
        if let Ok(port) = self._worklet_node.port() {
            port.set_onmessage(None);
            port.close();
        }
        // disconnect the audio graph
        let _ = self._source.disconnect();
        let _ = self._worklet_node.disconnect();
        // stop mic tracks (releases the device and stops producing audio frames)
        let tracks = self._stream.get_tracks();
        for i in 0..tracks.length() {
            if let Ok(t) = tracks.get(i).dyn_into::<web_sys::MediaStreamTrack>() {
                t.stop();
            }
        }
        // close context
        let _ = self.audio_ctx.close();
        // cleans up shared state
        self.finished.store(true, Relaxed);
        self.pair.1.notify_all();
    }
}

unsafe impl Send for WebAudioWrapper {}

unsafe impl Sync for WebAudioWrapper {}

/// Web Audio input source that implements [`AudioInput`].
///
/// This struct provides the [`AudioInput`] trait implementation for WASM
/// targets. It reads audio samples from the shared buffer populated by
/// [`WebAudioWrapper`]'s AudioWorklet message handler.
///
/// ## Blocking Behavior
///
/// The [`read_into`](AudioInput::read_into) method blocks (via condvar) when
/// the buffer is empty, waiting for more audio data to arrive. This makes it
/// suitable for use in a dedicated processing thread.
///
/// ## Buffer Management
///
/// - Samples are pushed by the JS message handler (WebAudioWrapper)
/// - Samples are consumed by this struct's `read_into` method
/// - Buffer is bounded by [`CHANNEL_SIZE`] (2,400 samples)
/// - Backpressure is handled in WebAudioWrapper (drops when buffer full)
pub struct WebAudioInput {
    pair: Arc<(Mutex<Vec<f32>>, Condvar)>,
    finished: Arc<AtomicBool>,
}

impl From<&WebAudioWrapper> for WebAudioInput {
    fn from(value: &WebAudioWrapper) -> Self {
        Self {
            pair: Arc::clone(&value.pair),
            finished: Arc::clone(&value.finished),
        }
    }
}

impl AudioInput for WebAudioInput {
    fn read_into(&mut self, dst: &mut [f32]) -> Result<usize, AudioError> {
        let mut written = 0;

        while written < dst.len() {
            let mut data = self.pair.0.lock().unwrap();
            // if producer is done and there's no more buffered audio, we're done
            if self.finished.load(Relaxed) && data.is_empty() {
                return Ok(written);
            }

            if data.is_empty() {
                let cond = |v: &mut Vec<f32>| v.is_empty() && !self.finished.load(Relaxed);

                data = self.pair.1.wait_while(data, cond).unwrap();

                // if we woke up finished and still empty, we're done
                if data.is_empty() && self.finished.load(Relaxed) {
                    return Ok(written);
                }
            }

            let take = (dst.len() - written).min(data.len());
            dst[written..written + take].copy_from_slice(&data[..take]);
            data.drain(..take);
            written += take;
        }

        Ok(written)
    }
}
