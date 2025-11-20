use crate::telepathy::CHANNEL_SIZE;
use log::error;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use wasm_bindgen::{JsCast, JsValue, prelude::Closure};
use wasm_bindgen_futures::JsFuture;
use wasm_sync::{Condvar, Mutex};
use web_sys::BlobPropertyBag;

// WebAudioWrapper is based on the code found at
// https://github.com/RustAudio/cpal/issues/813#issuecomment-2413007276
pub(crate) struct WebAudioWrapper {
    pair: Arc<(Mutex<Vec<f32>>, Condvar)>,
    finished: Arc<AtomicBool>,
    pub(crate) sample_rate: f32,
    audio_ctx: web_sys::AudioContext,
    _source: web_sys::MediaStreamAudioSourceNode,
    _media_devices: web_sys::MediaDevices,
    _stream: web_sys::MediaStream,
    _js_closure: Closure<dyn FnMut(JsValue)>,
    _worklet_node: web_sys::AudioWorkletNode,
}

impl WebAudioWrapper {
    pub(crate) async fn new() -> Result<Self, JsValue> {
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

    pub(crate) fn resume(&self) {
        _ = self.audio_ctx.resume();
    }
}

impl Drop for WebAudioWrapper {
    fn drop(&mut self) {
        let _ = self.audio_ctx.close();
        self.finished.store(true, Relaxed);
        self.pair.1.notify_all();
    }
}

unsafe impl Send for WebAudioWrapper {}

unsafe impl Sync for WebAudioWrapper {}

pub(crate) struct WebInput {
    pub(crate) pair: Arc<(Mutex<Vec<f32>>, Condvar)>,
    pub(crate) finished: Arc<AtomicBool>,
}

impl From<&WebAudioWrapper> for WebInput {
    fn from(value: &WebAudioWrapper) -> Self {
        Self {
            pair: Arc::clone(&value.pair),
            finished: Arc::clone(&value.finished),
        }
    }
}
