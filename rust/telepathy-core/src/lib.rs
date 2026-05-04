use libp2p::swarm::NetworkBehaviour;
use libp2p::{autonat, dcutr, identify, ping, relay};

pub mod audio;
pub mod error;
pub mod flutter;
mod frb_generated;
mod internal;
pub mod overlay;

pub use internal::{AudioDevice, Telepathy};

// https://github.com/RustAudio/cpal/issues/720#issuecomment-1311813294
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
extern "system" fn JNI_OnLoad(
    vm: *mut jni::sys::JavaVM,
    reserved: *mut std::os::raw::c_void,
) -> jni::sys::jint {
    unsafe {
        ndk_context::initialize_android_context(vm.cast(), reserved);
    }
    jni::JNIVersion::V9.into()
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
