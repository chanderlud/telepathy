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

use crate::Error;
use crate::internal::traits::{CHANNEL_SIZE, RingBufferInput};
use log::error;
use rtrb::RingBuffer;
use std::sync::Arc;
use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
use wasm_bindgen_futures::JsFuture;
use wasm_sync::Condvar;
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
    consumer: Option<rtrb::Consumer<f32>>,
    notify: Arc<Condvar>,
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
    pub async fn new() -> Result<Self, Error> {
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
                    const frame = inputs[0][0];
                    if (frame != undefined) {
                        this.port.postMessage(Float32Array.from(frame));
                    }
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

        let (mut input_producer, input_consumer) = RingBuffer::<f32>::new(CHANNEL_SIZE);
        let notify = Arc::new(Condvar::new());
        let notify_clone = notify.clone();

        // Float32Array
        let js_closure = Closure::wrap(Box::new(move |msg: JsValue| {
            let data_result: Result<Result<Vec<f32>, _>, _> = msg
                .dyn_into::<web_sys::MessageEvent>()
                .map(|msg| serde_wasm_bindgen::from_value(msg.data()));

            match data_result {
                Ok(Ok(data)) => {
                    let Ok(chunk) = input_producer.write_chunk_uninit(data.len()) else {
                        return;
                    };
                    chunk.fill_from_iter(data.into_iter());
                    notify_clone.notify_one();
                }
                Err(error) => error!("failed to handle worker message: {:?}", error),
                Ok(Err(error)) => error!("failed to handle worker message: {:?}", error),
            }
        }) as Box<dyn FnMut(JsValue)>);

        let js_func = js_closure.as_ref().unchecked_ref();

        worklet_node.port()?.set_onmessage(Some(js_func));

        Ok(WebAudioWrapper {
            consumer: Some(input_consumer),
            notify,
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

    pub fn get_input(&mut self) -> RingBufferInput {
        RingBufferInput::new(
            self.consumer.take().expect("consumer already consumed"),
            self.notify.clone(),
        )
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
        self.notify.notify_one();
    }
}

unsafe impl Send for WebAudioWrapper {}

unsafe impl Sync for WebAudioWrapper {}
