use libp2p::swarm::NetworkBehaviour;
use libp2p::{autonat, dcutr, identify, ping, relay};

pub mod api;
mod frb_generated; /* AUTO INJECTED BY flutter_rust_bridge. This line may not be accurate, and you can change it according to your needs. */

// https://github.com/RustAudio/cpal/issues/720#issuecomment-1311813294
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
extern "C" fn JNI_OnLoad(vm: jni::JavaVM, res: *mut std::os::raw::c_void) -> jni::sys::jint {
    use std::ffi::c_void;

    let vm = vm.get_java_vm_pointer() as *mut c_void;
    unsafe {
        ndk_context::initialize_android_context(vm, res);
    }
    jni::JNIVersion::V6.into()
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
