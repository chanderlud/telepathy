use libp2p::swarm::NetworkBehaviour;
use libp2p::{autonat, dcutr, identify, ping, relay};
use telepathy_audio::devices::AudioDeviceInfo;

pub mod audio;
pub mod error;
// #[cfg(feature = "native")]
pub mod flutter;
mod frb_generated;
mod internal;
// #[cfg(feature = "native")]
pub mod native;
pub mod overlay;

pub struct AudioDevice {
    pub name: String,
    pub id: String,
}

impl From<AudioDeviceInfo> for AudioDevice {
    fn from(value: AudioDeviceInfo) -> Self {
        Self {
            name: value.name,
            id: value.id,
        }
    }
}

#[derive(NetworkBehaviour)]
pub(crate) struct Behaviour {
    relay_client: relay::client::Behaviour,
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    dcutr: dcutr::Behaviour,
    stream: libp2p_stream::Behaviour,
    auto_nat: autonat::Behaviour,
}

// https://github.com/RustAudio/cpal/issues/720#issuecomment-1311813294
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
extern "C" fn JNI_OnLoad(vm: jni::JavaVM, res: *mut std::os::raw::c_void) -> jni::sys::jint {
    use std::ffi::c_void;

    let vm = vm.get_raw() as *mut c_void;
    unsafe {
        ndk_context::initialize_android_context(vm, res);
    }
    jni::JNIVersion::V9.into()
}
