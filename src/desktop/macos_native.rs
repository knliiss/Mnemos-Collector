use std::ffi::{CStr, CString, c_char, c_void};
use std::path::PathBuf;
use std::ptr::null_mut;

use anyhow::{Context, Result, bail};

type Object = *mut c_void;
type Selector = *mut c_void;

const NS_VARIABLE_STATUS_ITEM_LENGTH: f64 = -1.0;
const NS_MODAL_RESPONSE_OK: isize = 1;
const NS_IMAGE_SCALE_PROPORTIONALLY_DOWN: isize = 0;

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Object;
    fn sel_registerName(name: *const c_char) -> Selector;
    fn objc_msgSend();
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

pub struct MacStatusItem {
    status_bar: Object,
    status_item: Object,
}

impl MacStatusItem {
    pub fn install() -> Result<Self> {
        unsafe {
            let status_bar = message_id_0(class("NSStatusBar")?, selector("systemStatusBar")?);

            if status_bar.is_null() {
                bail!("NSStatusBar.systemStatusBar returned nil");
            }

            let status_item = message_id_f64(
                status_bar,
                selector("statusItemWithLength:")?,
                NS_VARIABLE_STATUS_ITEM_LENGTH,
            );

            if status_item.is_null() {
                bail!("failed to create NSStatusItem");
            }

            let button = message_id_0(status_item, selector("button")?);

            if button.is_null() {
                bail!("NSStatusItem.button returned nil");
            }

            let application = message_id_0(class("NSApplication")?, selector("sharedApplication")?);
            install_status_item_visual(button, application)?;
            message_void_id(
                button,
                selector("setToolTip:")?,
                ns_string("Mnemos Collector")?,
            );

            let menu = create_menu(application)?;

            message_void_id(status_item, selector("setMenu:")?, menu);

            Ok(Self {
                status_bar,
                status_item,
            })
        }
    }
}

impl Drop for MacStatusItem {
    fn drop(&mut self) {
        unsafe {
            if !self.status_bar.is_null()
                && !self.status_item.is_null()
                && let Ok(remove_status_item) = selector("removeStatusItem:")
            {
                message_void_id(self.status_bar, remove_status_item, self.status_item);
            }
        }
    }
}

pub fn hide_application() {
    unsafe {
        let Ok(application_class) = class("NSApplication") else {
            return;
        };
        let Ok(shared_application) = selector("sharedApplication") else {
            return;
        };
        let Ok(hide) = selector("hide:") else {
            return;
        };
        let application = message_id_0(application_class, shared_application);

        if !application.is_null() {
            message_void_id(application, hide, null_mut());
        }
    }
}

pub fn pick_log_file() -> Result<Option<PathBuf>> {
    unsafe {
        let panel = message_id_0(class("NSOpenPanel")?, selector("openPanel")?);

        if panel.is_null() {
            bail!("NSOpenPanel.openPanel returned nil");
        }

        message_void_bool(panel, selector("setCanChooseFiles:")?, true);
        message_void_bool(panel, selector("setCanChooseDirectories:")?, false);
        message_void_bool(panel, selector("setAllowsMultipleSelection:")?, false);
        message_void_id(
            panel,
            selector("setTitle:")?,
            ns_string("Выберите лог Cristalix")?,
        );
        message_void_id(
            panel,
            selector("setMessage:")?,
            ns_string("Можно выбрать latest.log или другой фактический лог текущей игры.")?,
        );

        let response = message_isize_0(panel, selector("runModal")?);

        if response != NS_MODAL_RESPONSE_OK {
            return Ok(None);
        }

        let url = message_id_0(panel, selector("URL")?);

        if url.is_null() {
            bail!("NSOpenPanel returned no selected URL");
        }

        let path = message_id_0(url, selector("path")?);

        if path.is_null() {
            bail!("selected NSOpenPanel URL has no filesystem path");
        }

        let utf8 = message_c_char_0(path, selector("UTF8String")?);

        if utf8.is_null() {
            bail!("selected NSOpenPanel path could not be converted to UTF-8");
        }

        let path = CStr::from_ptr(utf8)
            .to_str()
            .context("selected macOS log path is not valid UTF-8")?;

        Ok(Some(PathBuf::from(path)))
    }
}

unsafe fn install_status_item_visual(button: Object, application: Object) -> Result<()> {
    if application.is_null() {
        unsafe {
            message_void_id(button, selector("setTitle:")?, ns_string("M")?);
        }
        return Ok(());
    }

    let icon = unsafe { message_id_0(application, selector("applicationIconImage")?) };

    if icon.is_null() {
        unsafe {
            message_void_id(button, selector("setTitle:")?, ns_string("M")?);
        }
        return Ok(());
    }

    unsafe {
        message_void_id(button, selector("setTitle:")?, ns_string("")?);
        message_void_id(button, selector("setImage:")?, icon);
        message_void_isize(
            button,
            selector("setImageScaling:")?,
            NS_IMAGE_SCALE_PROPORTIONALLY_DOWN,
        );
    }

    Ok(())
}

unsafe fn create_menu(application: Object) -> Result<Object> {
    let menu_class = class("NSMenu")?;
    let menu = unsafe { message_id_0(menu_class, selector("alloc")?) };
    let menu = unsafe { message_id_id(menu, selector("initWithTitle:")?, ns_string("")?) };

    if menu.is_null() {
        bail!("failed to create NSMenu");
    }

    let open_item = unsafe {
        create_menu_item(
            "Открыть Mnemos Collector",
            selector("unhide:")?,
            application,
        )?
    };
    let separator = unsafe { message_id_0(class("NSMenuItem")?, selector("separatorItem")?) };
    let quit_item = unsafe {
        create_menu_item(
            "Завершить Mnemos Collector",
            selector("terminate:")?,
            application,
        )?
    };

    unsafe {
        message_void_id(menu, selector("addItem:")?, open_item);
        message_void_id(menu, selector("addItem:")?, separator);
        message_void_id(menu, selector("addItem:")?, quit_item);
    }

    Ok(menu)
}

unsafe fn create_menu_item(title: &str, action: Selector, target: Object) -> Result<Object> {
    let menu_item = unsafe { message_id_0(class("NSMenuItem")?, selector("alloc")?) };
    let menu_item = unsafe {
        message_id_id_selector_id(
            menu_item,
            selector("initWithTitle:action:keyEquivalent:")?,
            ns_string(title)?,
            action,
            ns_string("")?,
        )
    };

    if menu_item.is_null() {
        bail!("failed to create NSMenuItem '{title}'");
    }

    unsafe {
        message_void_id(menu_item, selector("setTarget:")?, target);
    }

    Ok(menu_item)
}

fn class(name: &str) -> Result<Object> {
    let name = CString::new(name).context("Objective-C class name contains NUL")?;
    let class = unsafe { objc_getClass(name.as_ptr()) };

    if class.is_null() {
        bail!("Objective-C class is unavailable");
    }

    Ok(class)
}

fn selector(name: &str) -> Result<Selector> {
    let name = CString::new(name).context("Objective-C selector contains NUL")?;
    let selector = unsafe { sel_registerName(name.as_ptr()) };

    if selector.is_null() {
        bail!("Objective-C selector registration failed");
    }

    Ok(selector)
}

fn ns_string(value: &str) -> Result<Object> {
    let value = CString::new(value).context("NSString value contains NUL")?;

    unsafe {
        Ok(message_id_c_char(
            class("NSString")?,
            selector("stringWithUTF8String:")?,
            value.as_ptr(),
        ))
    }
}

unsafe fn message_id_0(receiver: Object, selector: Selector) -> Object {
    let function: unsafe extern "C" fn(Object, Selector) -> Object =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };

    unsafe { function(receiver, selector) }
}

unsafe fn message_id_id(receiver: Object, selector: Selector, value: Object) -> Object {
    let function: unsafe extern "C" fn(Object, Selector, Object) -> Object =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };

    unsafe { function(receiver, selector, value) }
}

unsafe fn message_id_f64(receiver: Object, selector: Selector, value: f64) -> Object {
    let function: unsafe extern "C" fn(Object, Selector, f64) -> Object =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };

    unsafe { function(receiver, selector, value) }
}

unsafe fn message_id_c_char(receiver: Object, selector: Selector, value: *const c_char) -> Object {
    let function: unsafe extern "C" fn(Object, Selector, *const c_char) -> Object =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };

    unsafe { function(receiver, selector, value) }
}

unsafe fn message_id_id_selector_id(
    receiver: Object,
    selector: Selector,
    title: Object,
    action: Selector,
    key_equivalent: Object,
) -> Object {
    let function: unsafe extern "C" fn(Object, Selector, Object, Selector, Object) -> Object =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };

    unsafe { function(receiver, selector, title, action, key_equivalent) }
}

unsafe fn message_void_id(receiver: Object, selector: Selector, value: Object) {
    let function: unsafe extern "C" fn(Object, Selector, Object) =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };

    unsafe { function(receiver, selector, value) };
}

unsafe fn message_void_bool(receiver: Object, selector: Selector, value: bool) {
    let function: unsafe extern "C" fn(Object, Selector, i8) =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };

    unsafe { function(receiver, selector, i8::from(value)) };
}

unsafe fn message_void_isize(receiver: Object, selector: Selector, value: isize) {
    let function: unsafe extern "C" fn(Object, Selector, isize) =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };

    unsafe { function(receiver, selector, value) };
}

unsafe fn message_isize_0(receiver: Object, selector: Selector) -> isize {
    let function: unsafe extern "C" fn(Object, Selector) -> isize =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };

    unsafe { function(receiver, selector) }
}

unsafe fn message_c_char_0(receiver: Object, selector: Selector) -> *const c_char {
    let function: unsafe extern "C" fn(Object, Selector) -> *const c_char =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };

    unsafe { function(receiver, selector) }
}
