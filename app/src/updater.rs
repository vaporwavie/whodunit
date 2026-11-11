//! Sparkle is embedded only in release bundles. Cargo builds never load it or check the network.

use std::ffi::CStr;
use std::ptr;

use objc2::runtime::{AnyClass, AnyObject};
use objc2::{class, msg_send};

/// Main thread only. The controller and framework live until process exit because AppKit's
/// menu targets are weak and Sparkle owns asynchronous callbacks for the lifetime of the app.
pub(crate) unsafe fn start() -> Option<*mut AnyObject> {
    unsafe {
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        let key: *mut AnyObject =
            msg_send![class!(NSString), stringWithUTF8String: c"SUFeedURL".as_ptr()];
        let feed: *mut AnyObject = msg_send![bundle, objectForInfoDictionaryKey: key];
        if feed.is_null() {
            return None;
        }
        let frameworks: *mut AnyObject = msg_send![bundle, privateFrameworksPath];
        let name: *mut AnyObject =
            msg_send![class!(NSString), stringWithUTF8String: c"Sparkle.framework".as_ptr()];
        let path: *mut AnyObject = msg_send![frameworks, stringByAppendingPathComponent: name];
        let framework: *mut AnyObject = msg_send![class!(NSBundle), bundleWithPath: path];
        let mut error: *mut AnyObject = ptr::null_mut();
        let loaded: bool = msg_send![framework, loadAndReturnError: &mut error];
        if !loaded {
            let description: *mut AnyObject = msg_send![error, localizedDescription];
            let text: *const std::ffi::c_char = msg_send![description, UTF8String];
            let detail = if text.is_null() {
                "embedded framework is missing".into()
            } else {
                CStr::from_ptr(text).to_string_lossy()
            };
            eprintln!("Whodunit updater: couldn't load Sparkle: {detail}");
            return None;
        }
        let Some(controller_class) = AnyClass::get("SPUStandardUpdaterController") else {
            eprintln!("Whodunit updater: Sparkle's updater controller is missing");
            return None;
        };
        let controller: *mut AnyObject = msg_send![controller_class, alloc];
        let controller: *mut AnyObject = msg_send![controller,
            initWithStartingUpdater: true,
            updaterDelegate: ptr::null::<AnyObject>(),
            userDriverDelegate: ptr::null::<AnyObject>()
        ];
        (!controller.is_null()).then_some(controller)
    }
}
