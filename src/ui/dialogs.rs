//! Associate native dialogs on the UI thread; the shell retains the root window.

use std::sync::Arc;

use rfd::AsyncFileDialog;
use winit::window::Window;

pub(super) fn open<T>(
    root: Option<&Arc<Window>>,
    composer_window: bool,
    start: impl FnOnce(AsyncFileDialog) -> T,
) -> T {
    // Configure and start together while the native owner is retained. rfd
    // copies handles and macOS creates its panel synchronously on this thread.
    let dialog = AsyncFileDialog::new();
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if composer_window && let Some(parent) = active::Parent::new() {
        return start(dialog.set_parent(&parent));
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    // eframe exposes only the root native handle for immediate child viewports.
    let _ = composer_window;
    let dialog = match root {
        Some(window) => dialog.set_parent(window.as_ref()),
        None => dialog,
    };
    start(dialog)
}

#[cfg(target_os = "macos")]
mod active {
    use objc2::{MainThreadMarker, rc::Retained};
    use objc2_app_kit::{NSApplication, NSView, NSWindow};
    use std::ptr::NonNull;
    use winit::raw_window_handle::{
        AppKitDisplayHandle, AppKitWindowHandle, DisplayHandle, HandleError, HasDisplayHandle,
        HasWindowHandle, WindowHandle,
    };

    pub(super) struct Parent {
        // Retain both objects while rfd copies the handle and creates its panel.
        _window: Retained<NSWindow>,
        view: Retained<NSView>,
    }

    impl Parent {
        pub(super) fn new() -> Option<Self> {
            let marker = MainThreadMarker::new()?;
            let window = NSApplication::sharedApplication(marker).keyWindow()?;
            let view = window.contentView()?;
            Some(Self {
                _window: window,
                view,
            })
        }
    }

    impl HasWindowHandle for Parent {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            let view = NonNull::from(&*self.view).cast();
            // SAFETY: the retained view belongs to a retained window, and the
            // returned handle borrows this main-thread-only owner.
            Ok(unsafe { WindowHandle::borrow_raw(AppKitWindowHandle::new(view).into()) })
        }
    }

    impl HasDisplayHandle for Parent {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            // SAFETY: AppKit display handles contain no borrowed pointers.
            Ok(unsafe { DisplayHandle::borrow_raw(AppKitDisplayHandle::new().into()) })
        }
    }
}

#[cfg(target_os = "windows")]
mod active {
    use std::num::NonZeroIsize;
    use winit::raw_window_handle::{
        DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, Win32WindowHandle,
        WindowHandle, WindowsDisplayHandle,
    };

    pub(super) struct Parent(NonZeroIsize);

    impl Parent {
        pub(super) fn new() -> Option<Self> {
            // SAFETY: this only queries the window active on the UI thread.
            NonZeroIsize::new(unsafe {
                windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow()
            } as isize)
            .map(Self)
        }
    }

    impl HasWindowHandle for Parent {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            // SAFETY: this handle is used synchronously on the UI thread before
            // it can process a close event. rfd copies it as the dialog owner.
            Ok(unsafe { WindowHandle::borrow_raw(Win32WindowHandle::new(self.0).into()) })
        }
    }

    impl HasDisplayHandle for Parent {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            // SAFETY: Windows display handles contain no borrowed pointers.
            Ok(unsafe { DisplayHandle::borrow_raw(WindowsDisplayHandle::new().into()) })
        }
    }
}
