//! App-owned Reduce Motion and Reduce Transparency policy.
//!
//! Detection lives at the OS boundary. Paint, theme, and animation code
//! consume [`AccessibilityPolicy`] and never read system settings.

use serde::{Deserialize, Serialize};

/// Snapshot of the two system accessibility display flags this app honors.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccessibilityPolicy {
    pub reduce_motion: bool,
    pub reduce_transparency: bool,
}

impl AccessibilityPolicy {
    /// Explicit flags. Tests and fakes use this. Production code uses
    /// [`Self::detect`].
    pub const fn from_flags(reduce_motion: bool, reduce_transparency: bool) -> Self {
        Self {
            reduce_motion,
            reduce_transparency,
        }
    }

    /// Read the current host settings. On macOS this is NSWorkspace.
    /// Other hosts report both flags off.
    pub fn detect() -> Self {
        detect_host()
    }

    /// Lane B: skip interpolated motion when this is false.
    pub const fn allows_motion(&self) -> bool {
        !self.reduce_motion
    }

    /// Lane C: keep translucency when this is false.
    pub const fn allows_transparency(&self) -> bool {
        !self.reduce_transparency
    }
}

#[cfg(target_os = "macos")]
fn detect_host() -> AccessibilityPolicy {
    macos::detect()
}

#[cfg(not(target_os = "macos"))]
fn detect_host() -> AccessibilityPolicy {
    AccessibilityPolicy::from_flags(false, false)
}

#[cfg(target_os = "macos")]
mod macos {
    use super::AccessibilityPolicy;
    use std::ffi::c_void;
    use std::ptr::NonNull;

    #[repr(C)]
    struct ObjCObject {
        _private: [u8; 0],
    }

    type Class = *mut ObjCObject;
    type Sel = *mut c_void;
    type Id = *mut ObjCObject;

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const i8) -> Class;
        fn sel_registerName(name: *const i8) -> Sel;
    }

    // ObjC BOOL is a signed byte (signed char), not Rust bool / C _Bool.
    type ObjcBool = i8;

    // One untyped declaration. Distinct return types go through transmute so
    // rustc does not see two clashing objc_msgSend signatures.
    unsafe extern "C" {
        fn objc_msgSend();
    }

    fn msg_send_id(receiver: Id, sel: Sel) -> Id {
        unsafe {
            let send: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(
                objc_msgSend as unsafe extern "C" fn(),
            );
            send(receiver, sel)
        }
    }

    fn msg_send_bool(receiver: Id, sel: Sel) -> bool {
        unsafe {
            let send: unsafe extern "C" fn(Id, Sel) -> ObjcBool = std::mem::transmute(
                objc_msgSend as unsafe extern "C" fn(),
            );
            send(receiver, sel) != 0
        }
    }

    fn class(name: &str) -> Option<NonNull<ObjCObject>> {
        let c_name = std::ffi::CString::new(name).ok()?;
        NonNull::new(unsafe { objc_getClass(c_name.as_ptr()) })
    }

    fn sel(name: &str) -> Sel {
        let c_name = std::ffi::CString::new(name).expect("selector names are C strings");
        unsafe { sel_registerName(c_name.as_ptr()) }
    }

    fn workspace_bool(selector: &str) -> bool {
        let Some(cls) = class("NSWorkspace") else {
            return false;
        };
        let shared = msg_send_id(cls.as_ptr(), sel("sharedWorkspace"));
        if shared.is_null() {
            return false;
        }
        msg_send_bool(shared, sel(selector))
    }

    pub(super) fn detect() -> AccessibilityPolicy {
        AccessibilityPolicy::from_flags(
            workspace_bool("accessibilityDisplayShouldReduceMotion"),
            workspace_bool("accessibilityDisplayShouldReduceTransparency"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_flags_covers_both_true_and_false() {
        let off = AccessibilityPolicy::from_flags(false, false);
        assert!(off.allows_motion());
        assert!(off.allows_transparency());

        let motion_only = AccessibilityPolicy::from_flags(true, false);
        assert!(!motion_only.allows_motion());
        assert!(motion_only.allows_transparency());

        let transparency_only = AccessibilityPolicy::from_flags(false, true);
        assert!(transparency_only.allows_motion());
        assert!(!transparency_only.allows_transparency());

        let both = AccessibilityPolicy::from_flags(true, true);
        assert!(!both.allows_motion());
        assert!(!both.allows_transparency());
        assert_eq!(
            both,
            AccessibilityPolicy {
                reduce_motion: true,
                reduce_transparency: true,
            }
        );
    }

    #[test]
    fn detect_returns_a_policy() {
        let policy = AccessibilityPolicy::detect();
        let _ = policy.reduce_motion;
        let _ = policy.reduce_transparency;
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_detect_matches_a_second_read() {
        let a = AccessibilityPolicy::detect();
        let b = AccessibilityPolicy::detect();
        assert_eq!(a, b);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_detect_matches_nsworkspace_jxa() {
        let output = std::process::Command::new("osascript")
            .args([
                "-l",
                "JavaScript",
                "-e",
                "ObjC.import(\"AppKit\"); var ws = $.NSWorkspace.sharedWorkspace; String(ws.accessibilityDisplayShouldReduceMotion) + \" \" + String(ws.accessibilityDisplayShouldReduceTransparency);",
            ])
            .output()
            .expect("osascript");
        assert!(
            output.status.success(),
            "osascript failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let line = if stdout.split_whitespace().count() >= 2 {
            stdout
        } else {
            stderr
        };
        let mut parts = line.split_whitespace();
        let motion = parts.next().expect("motion token") == "true";
        let transparency = parts.next().expect("transparency token") == "true";
        assert_eq!(
            AccessibilityPolicy::detect(),
            AccessibilityPolicy::from_flags(motion, transparency)
        );
    }
}
