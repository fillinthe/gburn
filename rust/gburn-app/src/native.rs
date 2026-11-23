//! macOS: the menus in the menu bar at the top of the screen (muda), built from the same `menu::Node`s as the
//! in-window bar. A click comes back as the command id; shortcuts stay with `App::keys` except the few the
//! menu shows (⌘N, ⌘O, ⌘S…), which the menu then handles itself.

use crate::i18n::tr;
use crate::menu::{Node, Top};
use muda::{accelerator::Accelerator, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use std::sync::mpsc::{channel, Receiver};

pub struct Native {
    menu: Menu,
    shown: Vec<Top>,
    rx: Receiver<String>,
}

/// "⇧⌘S" → "Shift+Cmd+S". Keys that edit text (⌘C ⌘V ⌘X ⌘A ⌘Z) are left to the window, which needs them for its text fields.
fn accel(key: &str) -> Option<Accelerator> {
    let (mut mods, mut rest) = (Vec::new(), key);
    while let Some(c) = rest.chars().next() {
        let m = match c {
            '⌃' => "Ctrl",
            '⌥' => "Alt",
            '⇧' => "Shift",
            '⌘' => "Cmd",
            _ => break,
        };
        mods.push(m);
        rest = &rest[c.len_utf8()..];
    }
    let mut chars = rest.chars();
    let (k, none_more) = (chars.next()?, chars.next().is_none());
    if !mods.contains(&"Cmd") || !none_more || !(k.is_ascii_alphanumeric() || k == ',') {
        return None;
    }
    if "CVXAZ".contains(k.to_ascii_uppercase()) {
        return None;
    }
    let k = if k == ',' { "Comma".to_string() } else { k.to_ascii_uppercase().to_string() };
    format!("{}+{k}", mods.join("+")).parse().ok()
}

fn build(nodes: &[Node], into: &Submenu) {
    for n in nodes {
        match n {
            Node::Item { id, .. } if matches!(id.as_str(), "about" | "prefs" | "quit") => {}
            Node::Item { label, key, on, id } => {
                let _ = into.append(&MenuItem::with_id(id.as_str(), tr(label), *on, accel(key)));
            }
            Node::Sep => {
                let _ = into.append(&PredefinedMenuItem::separator());
            }
            Node::Sub { label, on, items } => {
                let s = Submenu::new(tr(label), *on);
                build(items, &s);
                let _ = into.append(&s);
            }
        }
    }
}

impl Native {
    pub fn new(ctx: &eframe::egui::Context) -> Native {
        let (tx, rx) = channel();
        let c = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = tx.send(e.id.0);
            c.request_repaint();
        }));
        let menu = Menu::new();
        let app = Submenu::new("GBurn", true);
        // About, Settings and Quit live here, as in every Mac app (the File / Edit menus' copies are left out below)
        let _ = app.append_items(&[
            &MenuItem::with_id("about", tr("About GBurn"), true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id("prefs", tr("Settings…"), true, accel("⌘,")),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::services(None),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::hide(None),
            &PredefinedMenuItem::hide_others(None),
            &PredefinedMenuItem::show_all(None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id("quit", tr("Quit GBurn"), true, accel("⌘Q")),
        ]);
        let _ = menu.append(&app);
        let window = Submenu::new(tr("Window"), true);
        let _ = window.append_items(&[&PredefinedMenuItem::close_window(None), &PredefinedMenuItem::minimize(None), &PredefinedMenuItem::maximize(None), &PredefinedMenuItem::bring_all_to_front(None)]);
        let _ = menu.append(&window);
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        Native { menu, shown: Vec::new(), rx }
    }

    /// Rebuild the menus when their text or enabled state changed (the model is cheap to compare).
    pub fn sync(&mut self, model: &[Top]) {
        if self.shown == model {
            return;
        }
        // everything between the app menu (first) and Window (last)
        while self.menu.items().len() > 2 {
            self.menu.remove_at(1);
        }
        for (i, (name, nodes)) in model.iter().enumerate() {
            let s = Submenu::new(tr(name), true);
            build(nodes, &s);
            let _ = self.menu.insert(&s, 1 + i);
        }
        self.shown = model.to_vec();
    }

    pub fn poll(&mut self) -> Option<String> {
        self.rx.try_recv().ok()
    }
}

/// Files Finder asked us to open (double-click, drop on the Dock icon, `open -a`), waiting for the next frame.
static OPENED: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());
static WAKE: std::sync::OnceLock<eframe::egui::Context> = std::sync::OnceLock::new();

pub fn opened() -> Vec<std::path::PathBuf> {
    OPENED.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
}

unsafe extern "C" fn open_urls(_this: *mut objc2::runtime::AnyObject, _sel: objc2::runtime::Sel, _app: *mut objc2::runtime::AnyObject, urls: *mut objc2::runtime::AnyObject) {
    use objc2::runtime::AnyObject;
    let n: usize = objc2::msg_send![urls, count];
    if std::env::var_os("GBURN_DEBUG_OPEN").is_some() {
        eprintln!("open_urls: {n} files");
    }
    for i in 0..n {
        let url: *mut AnyObject = objc2::msg_send![urls, objectAtIndex: i];
        let path: *mut AnyObject = objc2::msg_send![url, path];
        let c: *const std::ffi::c_char = objc2::msg_send![path, UTF8String];
        if !c.is_null() {
            if let Ok(mut v) = OPENED.lock() {
                v.push(std::path::PathBuf::from(std::ffi::CStr::from_ptr(c).to_string_lossy().into_owned()));
            }
        }
    }
    if let Some(c) = WAKE.get() {
        c.request_repaint();
    }
}

/// The delegate class gets `application:openURLs:` (once).
unsafe fn add_open_method(delegate: *mut objc2::runtime::AnyObject) {
    use objc2::runtime::{AnyClass, AnyObject, Sel};
    let cls: &AnyClass = (*delegate).class();
    let imp: unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject) = open_urls;
    // v = void, @ = the object, : = the selector, @ @ = the application and the array of URLs
    objc2::ffi::class_addMethod(
        cls as *const AnyClass as *mut _,
        objc2::sel!(application:openURLs:).as_ptr().cast(),
        std::mem::transmute::<unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject), Option<unsafe extern "C" fn()>>(imp),
        c"v@:@@".as_ptr(),
    );
}

/// The window's close button / ⌘W: hide GBurn like a Mac app does (it keeps running; the Dock icon brings it back).
pub fn hide_app() {
    unsafe {
        use objc2::runtime::AnyObject;
        let app: *mut AnyObject = objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
        let _: () = objc2::msg_send![app, hide: std::ptr::null_mut::<AnyObject>()];
    }
}

/// From the first frame on: wake the window when a file arrives, and make sure the method is there.
pub fn accept_opened_files(ctx: &eframe::egui::Context) {
    use objc2::runtime::AnyObject;
    let _ = WAKE.set(ctx.clone());
    unsafe {
        let app: *mut AnyObject = objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
        let delegate: *mut AnyObject = objc2::msg_send![app, delegate];
        if !delegate.is_null() {
            add_open_method(delegate);
        }
    }
}

/// Finder starts GBurn with the file in an event that arrives right after launch, before any frame: winit's app
/// delegate has no open-files method then, so add it the moment winit has set that delegate (a short-lived thread
/// watches for it while the event loop is being built).
pub fn catch_launch_files() {
    std::thread::spawn(|| {
        for _ in 0..4000 {
            let done = unsafe {
                use objc2::runtime::AnyObject;
                // the NSApp global, null until winit makes the app: `sharedApplication` from here would create it
                // on this thread and break the main event queue (a launch crash)
                extern "C" {
                    static NSApp: *mut AnyObject;
                }
                let app = std::ptr::read_volatile(std::ptr::addr_of!(NSApp));
                let delegate: *mut AnyObject = if app.is_null() { std::ptr::null_mut() } else { objc2::msg_send![app, delegate] };
                if delegate.is_null() {
                    false
                } else {
                    add_open_method(delegate);
                    true
                }
            };
            if done {
                if std::env::var_os("GBURN_DEBUG_OPEN").is_some() {
                    eprintln!("open method added early");
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_micros(500));
        }
    });
}
