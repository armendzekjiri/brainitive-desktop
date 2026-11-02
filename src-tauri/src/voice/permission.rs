//! Microphone permission (BRAI-439).
//!
//! - **macOS**: the system asks once per app (`NSMicrophoneUsageDescription` in Info.plist says
//!   why); the answer is read and requested through `AVCaptureDevice`. A denied app gets silence,
//!   not an error, so the status is checked before recording.
//! - **Windows**: Settings → Privacy → Microphone; a blocked app gets an error when the stream
//!   opens (`AudioError::Denied`).
//! - **Linux**: no per-app permission.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicPermission {
    Granted,
    /// Never asked yet (macOS): asking shows the system prompt.
    NotDetermined,
    Denied,
    /// Blocked by a device policy (macOS MDM / parental controls).
    Restricted,
    /// The OS has no queryable permission (Windows, Linux): just try.
    Unknown,
}

/// The OS settings page where the user allows the microphone, if the OS has one.
pub fn privacy_settings_url() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
    } else if cfg!(windows) {
        Some("ms-settings:privacy-microphone")
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::Mutex;

    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, Bool};

    use super::MicPermission;

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {
        /// `AVMediaTypeAudio` (an `NSString *` constant of AVFoundation).
        static AVMediaTypeAudio: &'static AnyObject;
    }

    fn device_class() -> Option<&'static AnyClass> {
        AnyClass::get(c"AVCaptureDevice")
    }

    pub fn status() -> MicPermission {
        let Some(cls) = device_class() else {
            return MicPermission::Unknown;
        };
        // AVAuthorizationStatus (NSInteger): 0 not determined, 1 restricted, 2 denied, 3 authorized
        let status: isize = unsafe { msg_send![cls, authorizationStatusForMediaType: AVMediaTypeAudio] };
        match status {
            0 => MicPermission::NotDetermined,
            1 => MicPermission::Restricted,
            2 => MicPermission::Denied,
            3 => MicPermission::Granted,
            _ => MicPermission::Unknown,
        }
    }

    /// Shows the system prompt (only when not determined yet); `done(granted)` runs on a
    /// background queue once the user answered.
    pub fn request(done: impl FnOnce(bool) + Send + 'static) {
        let Some(cls) = device_class() else {
            done(false);
            return;
        };
        let once = Mutex::new(Some(done));
        let block = RcBlock::new(move |granted: Bool| {
            if let Some(f) = once.lock().ok().and_then(|mut f| f.take()) {
                f(granted.as_bool());
            }
        });
        unsafe {
            let _: () = msg_send![cls, requestAccessForMediaType: AVMediaTypeAudio, completionHandler: &*block];
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::MicPermission;

    pub fn status() -> MicPermission {
        MicPermission::Unknown
    }

    pub fn request(done: impl FnOnce(bool) + Send + 'static) {
        done(true);
    }
}

pub use imp::{request, status};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_settings_link_per_os() {
        let url = privacy_settings_url();
        if cfg!(target_os = "macos") {
            assert!(url.unwrap().contains("Privacy_Microphone"));
        } else if cfg!(windows) {
            assert_eq!(url, Some("ms-settings:privacy-microphone"));
        } else {
            assert_eq!(url, None);
        }
    }

    #[test]
    fn the_status_can_be_read() {
        // whatever the machine says, reading it must not crash or prompt
        let _ = status();
    }
}
