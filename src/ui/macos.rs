//! Extend winit's menu without replacing its application/window delegates.

use std::cell::{Cell, RefCell};

use eframe::egui;
use objc2::{
    DefinedClass, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem, NSMenuItemValidation, NSPasteboard,
    NSPasteboardTypeString, NSResponder,
};
use objc2_foundation::{MainThreadMarker, NSBundle, NSObject, NSObjectProtocol, NSString};
use winit::raw_window_handle::HasWindowHandle;

/// The class of the windows winit makes for egui's viewports.
const WINIT_WINDOW: &std::ffi::CStr = c"WinitWindow";

#[derive(Clone, Copy)]
pub(super) enum Action {
    Settings,
    NewMessage,
    Quit,
    Edit(Edit),
}

/// An Edit menu command from one of the app's windows. With the menu in
/// place, AppKit gives it the command's keys, so egui gets the command in
/// their place. Undo and Redo never come here: the window answers them from
/// its own, empty, history, so their keys still reach egui.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Edit {
    Cut,
    Copy,
    Paste,
    SelectAll,
}

impl Edit {
    /// What egui would have received for the command's keys. Paste reads the
    /// clipboard now, as egui-winit does for Cmd+V.
    pub(super) fn events(self) -> Vec<egui::Event> {
        edit_events(self, clipboard_text)
    }
}

fn edit_events(edit: Edit, clipboard: impl FnOnce() -> Option<String>) -> Vec<egui::Event> {
    let command = egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND;
    // Only the press: the event is added after egui has read which keys are
    // held, so no key is left looking pressed.
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    match edit {
        Edit::Cut => vec![egui::Event::Cut],
        Edit::Copy => vec![egui::Event::Copy],
        Edit::Paste => clipboard()
            .map(|text| text.replace("\r\n", "\n"))
            .filter(|text| !text.is_empty())
            .map(egui::Event::Paste)
            .into_iter()
            .collect(),
        Edit::SelectAll => vec![key(egui::Key::A, command)],
    }
}

fn clipboard_text() -> Option<String> {
    // SAFETY: AppKit's pasteboard type constants are valid for the process.
    let kind = unsafe { NSPasteboardTypeString };
    let text = NSPasteboard::generalPasteboard().stringForType(kind)?;
    Some(text.to_string())
}

/// Actions that stay available while the mailbox's own commands wait, as on
/// the sign-in screen, where text is still edited.
fn always_available(action: Sel) -> bool {
    [
        sel!(rustonQuit:),
        sel!(cut:),
        sel!(copy:),
        sel!(paste:),
        sel!(selectAll:),
    ]
    .contains(&action)
}

struct TargetState {
    actions: RefCell<Vec<Action>>,
    context: egui::Context,
    enabled: Cell<bool>,
}

define_class!(
    // SAFETY: NSResponder imposes no subclassing requirements. The target is
    // only used on AppKit's main thread; its selector signatures match menu
    // actions.
    #[unsafe(super(NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetState]
    struct RustonMenuTarget;

    unsafe impl NSObjectProtocol for RustonMenuTarget {}

    // SAFETY: menu validation adds no protocol-specific safety requirements.
    unsafe impl NSMenuItemValidation for RustonMenuTarget {
        #[unsafe(method(validateMenuItem:))]
        fn validate(&self, item: &NSMenuItem) -> bool {
            item.action().is_some_and(always_available) || self.ivars().enabled.get()
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

        // The Edit menu's standard actions, which reach the target at the end
        // of an app window's responder chain.
        #[unsafe(method(cut:))]
        fn cut(&self, _sender: Option<&AnyObject>) {
            self.queue(Action::Edit(Edit::Cut));
        }

        #[unsafe(method(copy:))]
        fn copy(&self, _sender: Option<&AnyObject>) {
            self.queue(Action::Edit(Edit::Copy));
        }

        #[unsafe(method(paste:))]
        fn paste(&self, _sender: Option<&AnyObject>) {
            self.queue(Action::Edit(Edit::Paste));
        }

        #[unsafe(method(selectAll:))]
        fn select_all(&self, _sender: Option<&AnyObject>) {
            self.queue(Action::Edit(Edit::SelectAll));
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
        // SAFETY: inherited NSResponder init has the expected signature.
        unsafe { msg_send![super(target), init] }
    }

    fn queue(&self, action: Action) {
        self.ivars().actions.borrow_mut().push(action);
        self.ivars().context.request_repaint();
    }
}

pub(super) struct Menu {
    // NSMenuItem does not retain its target, and a window does not retain its
    // next responder. Keep the target alive, and disconnect it from every
    // item and window before dropping it.
    target: Retained<RustonMenuTarget>,
    root: Retained<NSMenu>,
    app: Retained<NSMenu>,
    settings: Retained<NSMenuItem>,
    separator: Retained<NSMenuItem>,
    /// File, Edit, View and Window, after the application menu.
    menus: Vec<Retained<NSMenuItem>>,
    new_message: Retained<NSMenuItem>,
    quit: Retained<NSMenuItem>,
    original_quit_target: Option<Retained<AnyObject>>,
    /// winit's items named after the executable, with the titles they had.
    renamed: Vec<(Retained<NSMenuItem>, Retained<NSString>)>,
}

impl Menu {
    pub(super) fn install(context: &egui::Context) -> Option<Self> {
        let marker = MainThreadMarker::new()?;
        let application = NSApplication::sharedApplication(marker);
        let root = application.mainMenu()?;
        let app = root.itemAtIndex(0)?.submenu()?;
        let quit = app
            .itemArray()
            .iter()
            .find(|item| item.action() == Some(sel!(terminate:)))?
            .clone();
        // Retain the existing target before replacing its non-owning pointer.
        let original_quit_target = quit.target();
        let renamed = name_after_bundle(&app);
        let target = RustonMenuTarget::new(marker, context);
        let settings = action_item(marker, &target, "Settings…", ",", sel!(rustonSettings:));
        let separator = NSMenuItem::separatorItem(marker);
        app.insertItem_atIndex(&separator, 1);
        app.insertItem_atIndex(&settings, 2);

        let command = NSEventModifierFlags::Command;
        let new_message = action_item(marker, &target, "New Message", "n", sel!(rustonNewMessage:));
        let file = submenu(
            marker,
            "File",
            &[
                new_message.clone(),
                NSMenuItem::separatorItem(marker),
                // The window's own close, so its guards for a written message
                // apply.
                standard_item(marker, "Close Window", sel!(performClose:), "w", command),
            ],
        );
        let edit = submenu(
            marker,
            "Edit",
            &[
                standard_item(marker, "Undo", sel!(undo:), "z", command),
                standard_item(
                    marker,
                    "Redo",
                    sel!(redo:),
                    "z",
                    command | NSEventModifierFlags::Shift,
                ),
                NSMenuItem::separatorItem(marker),
                standard_item(marker, "Cut", sel!(cut:), "x", command),
                standard_item(marker, "Copy", sel!(copy:), "c", command),
                standard_item(marker, "Paste", sel!(paste:), "v", command),
                standard_item(marker, "Select All", sel!(selectAll:), "a", command),
            ],
        );
        let view = submenu(
            marker,
            "View",
            &[standard_item(
                marker,
                "Enter Full Screen",
                sel!(toggleFullScreen:),
                "f",
                command | NSEventModifierFlags::Control,
            )],
        );
        let window = submenu(
            marker,
            "Window",
            &[
                standard_item(marker, "Minimize", sel!(performMiniaturize:), "m", command),
                standard_item(marker, "Zoom", sel!(performZoom:), "", command),
                NSMenuItem::separatorItem(marker),
                standard_item(
                    marker,
                    "Bring All to Front",
                    sel!(arrangeInFront:),
                    "",
                    command,
                ),
            ],
        );
        let menus = vec![file, edit, view, window];
        for (index, menu) in (1..).zip(&menus) {
            root.insertItem_atIndex(menu, index);
        }
        // AppKit lists the open windows at the end of this menu.
        application.setWindowsMenu(menus[3].submenu().as_deref());
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
            menus,
            new_message,
            quit,
            original_quit_target,
            renamed,
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

    /// Puts the target at the end of each app window's responder chain, where
    /// the Edit menu's standard actions find it. A native file dialog is left
    /// alone and keeps its own Copy and Paste.
    pub(super) fn adopt_windows(&self) {
        let (Some(marker), Some(winit_window)) =
            (MainThreadMarker::new(), AnyClass::get(WINIT_WINDOW))
        else {
            return;
        };
        for window in NSApplication::sharedApplication(marker).windows().iter() {
            // By kind, not by name: observing a window gives it a subclass.
            if !window.isKindOfClass(winit_window) {
                continue;
            }
            // SAFETY: Menu keeps the target alive, and its Drop takes the
            // target back out of every chain before releasing it.
            unsafe {
                if window.nextResponder().is_none() {
                    window.setNextResponder(Some(&self.target));
                }
            }
        }
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        self.app.removeItem(&self.settings);
        self.app.removeItem(&self.separator);
        for menu in &self.menus {
            self.root.removeItem(menu);
        }
        for (item, title) in &self.renamed {
            item.setTitle(title);
        }
        if let Some(marker) = MainThreadMarker::new() {
            let application = NSApplication::sharedApplication(marker);
            application.setWindowsMenu(None);
            let target = Retained::as_ptr(&self.target).cast::<NSResponder>();
            for window in application.windows().iter() {
                // SAFETY: only the target this menu put in a chain is removed.
                unsafe {
                    if window
                        .nextResponder()
                        .is_some_and(|next| std::ptr::eq(Retained::as_ptr(&next), target))
                    {
                        window.setNextResponder(None);
                    }
                }
            }
        }
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

/// Names winit's About, Hide and Quit items after the bundle, as macOS names
/// the menu itself, instead of after the executable.
fn name_after_bundle(app: &NSMenu) -> Vec<(Retained<NSMenuItem>, Retained<NSString>)> {
    let name = bundle_name();
    app.itemArray()
        .iter()
        .filter_map(|item| {
            let action = item.action()?;
            let verb = if action == sel!(orderFrontStandardAboutPanel:) {
                "About"
            } else if action == sel!(hide:) {
                "Hide"
            } else if action == sel!(terminate:) {
                "Quit"
            } else {
                return None;
            };
            let original = item.title();
            item.setTitle(&NSString::from_str(&format!("{verb} {name}")));
            Some((item, original))
        })
        .collect()
}

/// The bundle's name, or the app's own outside a bundle, as when run from
/// Cargo.
fn bundle_name() -> String {
    NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&NSString::from_str("CFBundleName"))
        .and_then(|name| name.downcast::<NSString>().ok())
        .map(|name| name.to_string())
        .unwrap_or_else(|| "Ruston Mail".to_owned())
}

/// A menu in the menu bar holding `items`.
fn submenu(
    marker: MainThreadMarker,
    title: &str,
    items: &[Retained<NSMenuItem>],
) -> Retained<NSMenuItem> {
    let title = NSString::from_str(title);
    let menu = NSMenu::new(marker);
    menu.setTitle(&title);
    for item in items {
        menu.addItem(item);
    }
    let item = NSMenuItem::new(marker);
    item.setTitle(&title);
    item.setSubmenu(Some(&menu));
    item
}

/// An item with no target: AppKit sends its action up the key window's
/// responder chain, to the window itself, a native dialog's text field, or
/// the target at the end of an app window's chain.
fn standard_item(
    marker: MainThreadMarker,
    title: &str,
    action: Sel,
    key: &str,
    modifiers: NSEventModifierFlags,
) -> Retained<NSMenuItem> {
    // SAFETY: with no target, AppKit only sends the action to a responder that
    // implements it, and disables the item when none does.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            marker.alloc(),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(key),
        )
    };
    item.setKeyEquivalentModifierMask(modifiers);
    item
}

fn action_item(
    marker: MainThreadMarker,
    target: &RustonMenuTarget,
    title: &str,
    key: &str,
    action: Sel,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_commands_become_what_their_keys_would_have_sent() {
        let command = egui::Modifiers::MAC_CMD | egui::Modifiers::COMMAND;
        let pressed = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let no_clipboard = || None;

        assert_eq!(edit_events(Edit::Cut, no_clipboard), [egui::Event::Cut]);
        assert_eq!(edit_events(Edit::Copy, no_clipboard), [egui::Event::Copy]);
        assert_eq!(
            edit_events(Edit::SelectAll, no_clipboard),
            [pressed(egui::Key::A, command)]
        );
        // As egui-winit pastes: Windows line ends become plain ones, and an
        // empty or unreadable clipboard pastes nothing.
        assert_eq!(
            edit_events(Edit::Paste, || Some("one\r\ntwo".to_owned())),
            [egui::Event::Paste("one\ntwo".to_owned())]
        );
        assert!(edit_events(Edit::Paste, || Some(String::new())).is_empty());
        assert!(edit_events(Edit::Paste, no_clipboard).is_empty());
    }

    #[test]
    fn editing_and_quitting_stay_available_outside_the_mailbox() {
        for action in [
            sel!(rustonQuit:),
            sel!(copy:),
            sel!(paste:),
            sel!(selectAll:),
        ] {
            assert!(always_available(action));
        }
        for action in [sel!(rustonSettings:), sel!(rustonNewMessage:)] {
            assert!(!always_available(action));
        }
    }
}
