use log::error;
use objc2::runtime::{AnyObject, Bool};
use objc2::{class, msg_send};
use objc2_foundation::ns_string;

pub(crate) fn configure_audio_session() {
    unsafe {
        let av_audio_session: *mut AnyObject = msg_send![class!(AVAudioSession), sharedInstance];

        // set category to `AVAudioSessionCategoryPlayAndRecord`
        let category = ns_string!("AVAudioSessionCategoryPlayAndRecord");
        let mode = ns_string!("AVAudioSessionModeDefault");
        let error: *mut AnyObject = std::ptr::null_mut();

        let success: Bool = msg_send![av_audio_session, setCategory: category,
            mode: mode,
            options: 0_u64,
            error: &error];

        if success == Bool::NO {
            error!("Failed to set AVAudioSession category.");
        }

        let override_output: *mut AnyObject = msg_send![class!(AVAudioSession), sharedInstance];
        let _: Bool = msg_send![override_output, overrideOutputAudioPort: 1_u64, error: &error];

        // Activate the audio session
        let success: Bool = msg_send![av_audio_session, setActive: Bool::YES, error: &error];

        if success == Bool::NO {
            error!("Failed to activate AVAudioSession.");
        }
    }
}

pub(crate) fn deactivate_audio_session() {
    unsafe {
        let av_audio_session: *mut AnyObject = msg_send![class!(AVAudioSession), sharedInstance];

        let error: *mut AnyObject = std::ptr::null_mut();
        let success: Bool = msg_send![av_audio_session, setActive: Bool::NO, error: &error];

        if success == Bool::NO {
            error!("Failed to deactivate AVAudioSession.");
        }
    }
}
