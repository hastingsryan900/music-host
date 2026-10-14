//! A ♪ in the macOS menu bar: what's playing, play/pause, next/previous,
//! the mini player, and a way back to the main window. (Other platforms get
//! a no-op stand-in.)

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cmd {
    Toggle = 1,
    Next = 2,
    Prev = 3,
    Show = 4,
    Mini = 5,
}

#[cfg(target_os = "macos")]
pub use mac::{activate, MenuBar};

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // from the objc macros
mod mac {
    use super::Cmd;
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel, BOOL, YES};
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::CString;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::OnceLock;


    static PENDING: AtomicU8 = AtomicU8::new(0);
    static WAKE: OnceLock<egui::Context> = OnceLock::new();

    type Id = *mut Object;

    fn nsstring(s: &str) -> Id {
        let c = CString::new(s.replace('\0', "")).unwrap_or_default();
        unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
    }

    fn target_class() -> &'static Class {
        static CLASS: OnceLock<&'static Class> = OnceLock::new();
        CLASS.get_or_init(|| {
            if let Some(c) = Class::get("MHMenuTarget") {
                return c;
            }
            let mut decl = ClassDecl::new("MHMenuTarget", class!(NSObject)).expect("MHMenuTarget");
            extern "C" fn fire(_: &Object, _: Sel, item: Id) {
                let tag: isize = unsafe { msg_send![item, tag] };
                PENDING.store(tag as u8, Ordering::Relaxed);
                if let Some(ctx) = WAKE.get() {
                    ctx.request_repaint();
                }
            }
            unsafe { decl.add_method(sel!(fire:), fire as extern "C" fn(&Object, Sel, Id)) };
            decl.register()
        })
    }

    pub struct MenuBar {
        _item: Id,
        info: Id,
        play: Id,
        shown: (String, bool),
    }

    impl MenuBar {
        /// Must be called on the main thread (eframe's update is).
        pub fn new(ctx: &egui::Context) -> Option<Self> {
            let _ = WAKE.set(ctx.clone());
            unsafe {
                let target: Id = msg_send![target_class(), new];
                let bar: Id = msg_send![class!(NSStatusBar), systemStatusBar];
                let item: Id = msg_send![bar, statusItemWithLength: -1.0f64];
                if item.is_null() {
                    return None;
                }
                let _: () = msg_send![item, retain];
                let button: Id = msg_send![item, button];
                let img: Id = msg_send![class!(NSImage), imageWithSystemSymbolName: nsstring("music.note") accessibilityDescription: nsstring("Music Host")];
                if img.is_null() {
                    let _: () = msg_send![button, setTitle: nsstring("♪")];
                } else {
                    let _: () = msg_send![img, setTemplate: YES];
                    let _: () = msg_send![button, setImage: img];
                }

                let menu: Id = msg_send![class!(NSMenu), new];
                let _: () = msg_send![menu, setAutoenablesItems: false as BOOL];
                let add = |title: &str, tag: Option<Cmd>, key: &str| -> Id {
                    let action = if tag.is_some() { sel!(fire:) } else { sel!(description) };
                    let it: Id = msg_send![class!(NSMenuItem), alloc];
                    let it: Id = msg_send![it, initWithTitle: nsstring(title) action: action keyEquivalent: nsstring(key)];
                    match tag {
                        Some(t) => {
                            let _: () = msg_send![it, setTarget: target];
                            let _: () = msg_send![it, setTag: t as isize];
                        }
                        None => {
                            let _: () = msg_send![it, setEnabled: false as BOOL];
                        }
                    }
                    let _: () = msg_send![menu, addItem: it];
                    it
                };
                let sep = || {
                    let s: Id = msg_send![class!(NSMenuItem), separatorItem];
                    let _: () = msg_send![menu, addItem: s];
                };
                let info = add("Not Playing", None, "");
                sep();
                let play = add("Play", Some(Cmd::Toggle), "");
                add("Next", Some(Cmd::Next), "");
                add("Previous", Some(Cmd::Prev), "");
                sep();
                add("Mini Player", Some(Cmd::Mini), "");
                add("Show Music Host", Some(Cmd::Show), "");
                let _: () = msg_send![item, setMenu: menu];
                Some(MenuBar { _item: item, info, play, shown: (String::new(), false) })
            }
        }

        /// Keep the menu in sync with what's playing (cheap when unchanged).
        pub fn update(&mut self, now_playing: &str, playing: bool) {
            if self.shown.0 == now_playing && self.shown.1 == playing {
                return;
            }
            self.shown = (now_playing.to_string(), playing);
            let info = if now_playing.is_empty() { "Not Playing" } else { now_playing };
            unsafe {
                let _: () = msg_send![self.info, setTitle: nsstring(info)];
                let _: () = msg_send![self.play, setTitle: nsstring(if playing { "Pause" } else { "Play" })];
            }
        }

        pub fn poll(&self) -> Option<Cmd> {
            match PENDING.swap(0, Ordering::Relaxed) {
                1 => Some(Cmd::Toggle),
                2 => Some(Cmd::Next),
                3 => Some(Cmd::Prev),
                4 => Some(Cmd::Show),
                5 => Some(Cmd::Mini),
                _ => None,
            }
        }
    }

    /// Bring the app to the front (e.g. from the menu bar).
    pub fn activate() {
        unsafe {
            let app: Id = msg_send![class!(NSApplication), sharedApplication];
            let _: () = msg_send![app, activateIgnoringOtherApps: YES];
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub struct MenuBar;

#[cfg(not(target_os = "macos"))]
impl MenuBar {
    pub fn new(_ctx: &egui::Context) -> Option<Self> {
        None
    }
    pub fn update(&mut self, _now_playing: &str, _playing: bool) {}
    pub fn poll(&self) -> Option<Cmd> {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub fn activate() {}
