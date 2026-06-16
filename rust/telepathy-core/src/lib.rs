use telepathy_audio::devices::AudioDeviceInfo;

#[cfg(feature = "flutter")]
pub mod flutter;
#[cfg(feature = "flutter")]
mod frb_generated;
/// flutter_rust_bridge:ignore
pub mod internal;
#[cfg(feature = "native")]
pub mod native;
pub mod overlay;
pub mod player;
pub mod types;

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
