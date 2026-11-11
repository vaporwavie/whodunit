//! The menu bar icon with Open, Check for Updates (release bundles only), and Quit.

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::RefCell;
    use std::ffi::{CString, c_void};

    use objc2::declare::ClassBuilder;
    use objc2::encode::{Encode, Encoding};
    use objc2::runtime::{AnyObject, NSObject, Sel};
    use objc2::{ClassType, class, msg_send, sel};

    const ICON: &[u8] = include_bytes!("../assets/icons/menu-bar.png");
    const ICON_POINTS: f64 = 18.0;
    const SQUARE_LENGTH: f64 = -2.0;

    #[repr(C)]
    struct Size {
        width: f64,
        height: f64,
    }

    unsafe impl Encode for Size {
        const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
    }

    thread_local! {
        static OPEN: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    }

    extern "C" fn on_open(_: *mut NSObject, _: Sel, _: *mut AnyObject) {
        OPEN.with(|open| {
            if let Some(open) = open.borrow().as_ref() {
                open();
            }
        });
    }

    unsafe fn ns_string(value: &str) -> *mut AnyObject {
        let value = CString::new(value).unwrap_or_default();
        unsafe { msg_send![class!(NSString), stringWithUTF8String: value.as_ptr()] }
    }

    unsafe fn menu_item(
        title: &str,
        action: Sel,
        key: &str,
        target: *mut AnyObject,
    ) -> *mut AnyObject {
        unsafe {
            let item: *mut AnyObject = msg_send![class!(NSMenuItem), alloc];
            let item: *mut AnyObject = msg_send![item, initWithTitle: ns_string(title), action: action, keyEquivalent: ns_string(key)];
            let _: () = msg_send![item, setTarget: target];
            item
        }
    }

    /// Main thread only, once: AppKit keeps the item while the process runs.
    pub fn install(open: impl Fn() + 'static) {
        OPEN.with(|slot| *slot.borrow_mut() = Some(Box::new(open)));
        // SAFETY: AppKit calls on the main thread. The status item, its menu and the Open target
        // are retained and never released, since AppKit holds menu targets weakly.
        unsafe {
            let Some(mut builder) = ClassBuilder::new("WhodunitMenuBarTarget", NSObject::class())
            else {
                return;
            };
            builder.add_method(
                sel!(open:),
                on_open as extern "C" fn(*mut NSObject, Sel, *mut AnyObject),
            );
            let target_class = builder.register();
            let target: *mut AnyObject = msg_send![target_class, new];

            let bar: *mut AnyObject = msg_send![class!(NSStatusBar), systemStatusBar];
            let item: *mut AnyObject = msg_send![bar, statusItemWithLength: SQUARE_LENGTH];
            let _: *mut AnyObject = msg_send![item, retain];

            let data: *mut AnyObject = msg_send![class!(NSData), dataWithBytes: ICON.as_ptr().cast::<c_void>(), length: ICON.len()];
            let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
            let image: *mut AnyObject = msg_send![image, initWithData: data];
            let _: () = msg_send![image, setSize: Size { width: ICON_POINTS, height: ICON_POINTS }];
            let _: () = msg_send![image, setTemplate: true];
            let button: *mut AnyObject = msg_send![item, button];
            let _: () = msg_send![button, setImage: image];
            let _: () = msg_send![button, setToolTip: ns_string("Whodunit")];

            let menu: *mut AnyObject = msg_send![class!(NSMenu), new];
            let _: () =
                msg_send![menu, addItem: menu_item("Open Whodunit", sel!(open:), "", target)];
            if let Some(updater) = crate::updater::start() {
                let _: () = msg_send![menu, addItem: menu_item("Check for Updates…", sel!(checkForUpdates:), "", updater)];
            }
            let separator: *mut AnyObject = msg_send![class!(NSMenuItem), separatorItem];
            let _: () = msg_send![menu, addItem: separator];
            let _: () = msg_send![menu, addItem: menu_item("Quit Whodunit", sel!(terminate:), "q", std::ptr::null_mut())];
            let _: () = msg_send![item, setMenu: menu];
        }
    }
}

#[cfg(target_os = "macos")]
pub use mac::install;

#[cfg(not(target_os = "macos"))]
pub fn install(_open: impl Fn() + 'static) {}
