//! Frontmost-window lookup via the CoreGraphics window list.
//!
//! `CGWindowListCopyWindowInfo` is used rather than the Accessibility API because
//! it needs no permission prompt for *geometry* — only window titles and images
//! are gated. We never read a title.
//!
//! The dictionary keys are declared by hand instead of linking the `kCGWindow*`
//! constants: their string contents are literally their own names, so building
//! the CFStrings locally keeps this file's dependencies down to CoreFoundation.

use std::ffi::c_void;

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;

use super::TermWindow;

const ON_SCREEN_ONLY: u32 = 1 << 0;
const EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
const NULL_WINDOW_ID: u32 = 0;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
}

/// All on-screen windows, front to back, with their window level. Level 0 is the
/// normal application layer; menu bars, docks and our own floating overlay sit
/// above it.
pub fn list_windows() -> Vec<(i64, TermWindow)> {
    let mut out = Vec::new();
    let array_ref = unsafe {
        CGWindowListCopyWindowInfo(ON_SCREEN_ONLY | EXCLUDE_DESKTOP_ELEMENTS, NULL_WINDOW_ID)
    };
    if array_ref.is_null() {
        return out;
    }
    let windows: CFArray<*const c_void> = unsafe { CFArray::wrap_under_create_rule(array_ref) };

    let k_layer = CFString::from_static_string("kCGWindowLayer");
    let k_alpha = CFString::from_static_string("kCGWindowAlpha");
    let k_owner = CFString::from_static_string("kCGWindowOwnerName");
    let k_pid = CFString::from_static_string("kCGWindowOwnerPID");
    let k_bounds = CFString::from_static_string("kCGWindowBounds");

    for item in windows.iter() {
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(*item as CFDictionaryRef) };

        let Some(layer) = number(&dict, &k_layer).map(|n| n as i64) else {
            continue;
        };
        // Fully transparent windows are invisible helpers, not something in front.
        if number(&dict, &k_alpha).unwrap_or(1.0) <= 0.01 {
            continue;
        }

        let Some(app) = dict
            .find(&k_owner)
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
        else {
            continue;
        };

        let Some(bounds) = dict.find(&k_bounds) else {
            continue;
        };
        let bounds: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bounds.as_CFTypeRef() as CFDictionaryRef) };
        let (Some(x), Some(y), Some(w), Some(h)) = (
            number(&bounds, &CFString::from_static_string("X")),
            number(&bounds, &CFString::from_static_string("Y")),
            number(&bounds, &CFString::from_static_string("Width")),
            number(&bounds, &CFString::from_static_string("Height")),
        ) else {
            continue;
        };

        let pid = number(&dict, &k_pid).unwrap_or(0.0) as u32;
        out.push((
            layer,
            TermWindow {
                app,
                pid,
                x,
                y,
                w,
                h,
            },
        ));
    }
    out
}

fn number(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<f64> {
    dict.find(key)
        .and_then(|v| v.downcast::<CFNumber>())
        .and_then(|n| n.to_f64())
}
