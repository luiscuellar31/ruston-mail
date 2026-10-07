//! Extend winit's menu without replacing its application/window delegates.

use std::cell::{Cell, RefCell};

use eframe::egui;
use objc2::{
    DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::AnyObject, sel,
};
use objc2_app_kit::{
    NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem, NSMenuItemValidation,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString};
use winit::raw_window_handle::HasWindowHandle;

#[derive(Clone, Copy)]
pub(super) enum Action {
    Settings,
    NewMessage,
    Quit,
}

struct TargetState {
    actions: RefCell<Vec<Action>>,
    context: egui::Context,
    enabled: Cell<bool>,
}

define_class!(
    // SAFETY: NSObject imposes no subclassing requirements. The target is only
    // used on AppKit's main thread; its selector signatures match menu actions.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetState]
    struct RustonMenuTarget;

    unsafe impl NSObjectProtocol for RustonMenuTarget {}

    // SAFETY: menu validation adds no protocol-specific safety requirements.
    unsafe impl NSMenuItemValidation for RustonMenuTarget {
        #[unsafe(method(validateMenuItem:))]
        fn validate(&self, item: &NSMenuItem) -> bool {
            item.action() == Some(sel!(rustonQuit:)) || self.ivars().enabled.get()
        }
    }

    impl RustonMenuTarget {
        #[unsafe(method(rustonSettings:))]
        fn settings(&self, _sender: Option<&AnyObject>) {
            self.queue(Action::Settings);
        }

        #[unsafe(method(rustonNewMessage:))]
        fn new_message(&self, _sender: Option<&AnyObject>) {
            self.queue(Action::NewMessage);
        }

        #[unsafe(method(rustonQuit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            self.queue(Action::Quit);
        }
    }
);

impl RustonMenuTarget {
    fn new(marker: MainThreadMarker, context: &egui::Context) -> Retained<Self> {
        let target = Self::alloc(marker).set_ivars(TargetState {
            actions: RefCell::new(Vec::new()),
            context: context.clone(),
            enabled: Cell::new(false),
        });
        // SAFETY: inherited NSObject init has the expected signature.
        unsafe { msg_send![super(target), init] }
    }

    fn queue(&self, action: Action) {
        self.ivars().actions.borrow_mut().push(action);
        self.ivars().context.request_repaint();
    }
}

pub(super) struct Menu {
    // NSMenuItem does not retain its target. Keep it alive and disconnect every
    // owned item before dropping it.
    target: Retained<RustonMenuTarget>,
    root: Retained<NSMenu>,
    app: Retained<NSMenu>,
    settings: Retained<NSMenuItem>,
    separator: Retained<NSMenuItem>,
    file: Retained<NSMenuItem>,
    new_message: Retained<NSMenuItem>,
    quit: Retained<NSMenuItem>,
    original_quit_target: Option<Retained<AnyObject>>,
}

impl Menu {
    pub(super) fn install(context: &egui::Context) -> Option<Self> {
        let marker = MainThreadMarker::new()?;
        let root = NSApplication::sharedApplication(marker).mainMenu()?;
        let app = root.itemAtIndex(0)?.submenu()?;
        let quit = app
            .itemArray()
            .iter()
            .find(|item| item.action() == Some(sel!(terminate:)))?
            .clone();
        // Retain the existing target before replacing its non-owning pointer.
        let original_quit_target = quit.target();
        let target = RustonMenuTarget::new(marker, context);
        let settings = action_item(marker, &target, "Settings…", ",", sel!(rustonSettings:));
        let separator = NSMenuItem::separatorItem(marker);
        app.insertItem_atIndex(&separator, 1);
        app.insertItem_atIndex(&settings, 2);

        let file = NSMenuItem::new(marker);
        file.setTitle(&NSString::from_str("File"));
        let file_menu = NSMenu::new(marker);
        file_menu.setTitle(&NSString::from_str("File"));
        let new_message = action_item(marker, &target, "New Message", "n", sel!(rustonNewMessage:));
        file_menu.addItem(&new_message);
        file.setSubmenu(Some(&file_menu));
        root.insertItem_atIndex(&file, 1);
        // SAFETY: the retained target implements this menu-action selector.
        unsafe {
            quit.setAction(Some(sel!(rustonQuit:)));
            quit.setTarget(Some(&target));
        }

        Some(Self {
            target,
            root,
            app,
            settings,
            separator,
            file,
            new_message,
            quit,
            original_quit_target,
        })
    }

    pub(super) fn set_enabled(&self, enabled: bool) {
        // Auto-validation finds our action methods; additionally implement
        // validation on the target so AppKit preserves the application guards.
        self.settings.setEnabled(enabled);
        self.new_message.setEnabled(enabled);
        self.target.ivars().enabled.set(enabled);
    }

    pub(super) fn drain(&self) -> Vec<Action> {
        self.target.ivars().actions.take()
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        self.app.removeItem(&self.settings);
        self.app.removeItem(&self.separator);
        self.root.removeItem(&self.file);
        // SAFETY: restore the original selector/target while both are retained;
        // detached owned items are disconnected before our target is released.
        unsafe {
            self.settings.setTarget(None);
            self.new_message.setTarget(None);
            self.quit.setTarget(self.original_quit_target.as_deref());
            self.quit.setAction(Some(sel!(terminate:)));
        }
    }
}

fn action_item(
    marker: MainThreadMarker,
    target: &RustonMenuTarget,
    title: &str,
    key: &str,
    action: objc2::runtime::Sel,
) -> Retained<NSMenuItem> {
    // SAFETY: each selector names a matching action method on the retained
    // target; Menu owns the target for the lifetime of attached items.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            marker.alloc(),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    };
    unsafe { item.setTarget(Some(target)) };
    item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
    item.setEnabled(false);
    item
}

pub(super) fn update_window_chrome(context: &egui::Context, frame: &eframe::Frame) {
    let Some(metrics) = frame
        .window_handle()
        .ok()
        .and_then(|handle| eframe::WindowChromeMetrics::from_window_handle(&handle.as_raw()))
    else {
        return;
    };
    let height = metrics.traffic_lights_size.y;
    if height.is_finite() && height >= 0.0 {
        context.data_mut(|data| {
            data.insert_temp(egui::Id::new("native-titlebar-height"), height);
        });
    }
}
