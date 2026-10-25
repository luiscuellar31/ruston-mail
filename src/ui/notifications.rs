//! Native delivery and permission state. Mail detection belongs to App.

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use eframe::egui;
use ruston_core::mail::attachments::is_invisible_mark;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use crate::runtime::Runtime;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Status {
    #[default]
    Off = 0,
    Ready = 1,
    Pending = 2,
    Denied = 3,
    Unavailable = 4,
}

impl Status {
    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Off => "Notifications are inactive until enabled for a signed-in account.",
            Self::Ready => "The operating system controls delivery, sounds, and Do Not Disturb.",
            Self::Pending => "Waiting for the operating system's notification permission.",
            Self::Denied => {
                "Notifications are blocked. Allow Ruston Mail in system notification settings."
            }
            #[cfg(target_os = "macos")]
            Self::Unavailable => {
                "Notifications need the installed Ruston Mail.app and an available system service."
            }
            #[cfg(not(target_os = "macos"))]
            Self::Unavailable => {
                "The system notification service is unavailable. Mail refresh will continue."
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Notifications {
    enabled: Arc<AtomicBool>,
    status: Arc<AtomicU8>,
    #[cfg(target_os = "linux")]
    shown: linux::Shown,
}

impl Notifications {
    pub(super) fn stop(&self) {
        self.enabled.store(false, Ordering::Release);
    }

    pub(super) fn status(&self) -> Status {
        if !self.enabled.load(Ordering::Acquire) {
            return Status::Off;
        }
        match self.status.load(Ordering::Acquire) {
            1 => Status::Ready,
            2 => Status::Pending,
            3 => Status::Denied,
            4 => Status::Unavailable,
            _ => Status::Off,
        }
    }

    pub(super) fn set_enabled(&mut self, enabled: bool, context: &egui::Context) {
        if self.enabled.load(Ordering::Acquire) == enabled {
            return;
        }
        // Revoke the previous activation permanently: queued native work must
        // not become valid again when another account enables notifications.
        self.enabled.store(false, Ordering::Release);
        self.enabled = Arc::new(AtomicBool::new(enabled));
        self.status = Arc::new(AtomicU8::new(Status::Off as u8));
        if !enabled {
            self.withdraw();
        }
        if enabled {
            #[cfg(target_os = "macos")]
            macos::request_permission(self.status.clone(), context.clone());
            #[cfg(not(target_os = "macos"))]
            {
                self.status.store(Status::Ready as u8, Ordering::Release);
                let _ = context;
            }
        }
    }

    /// Takes the notifications already delivered away with the account:
    /// Windows keeps them in its notification center, on a thread of their
    /// own for COM. Tests must not clear the toasts of the Ruston Mail the
    /// machine runs.
    #[cfg(all(target_os = "windows", not(test)))]
    fn withdraw(&self) {
        let _ = std::thread::Builder::new()
            .name("ruston-toast-clear".into())
            .spawn(windows::clear);
    }

    /// Takes the notifications already delivered away with the account: on
    /// Linux, the last one, which stays until it is dismissed. The request
    /// runs on a thread of its own, as on Windows.
    #[cfg(target_os = "linux")]
    fn withdraw(&self) {
        if !self.shown.is_some() {
            return;
        }
        let shown = self.shown.clone();
        let _ = std::thread::Builder::new()
            .name("ruston-notification-close".into())
            .spawn(move || {
                if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    runtime.block_on(linux::close(shown));
                }
            });
    }

    /// macOS keeps what it delivered after signing out, and tests on Windows
    /// leave the machine's toasts alone.
    #[cfg(not(any(target_os = "linux", all(target_os = "windows", not(test)))))]
    fn withdraw(&self) {}

    pub(super) fn show(
        &self,
        sender: &str,
        subject: &str,
        runtime: &Runtime,
        context: &egui::Context,
    ) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        let sender = display_text(sender, "Proton Mail", 160);
        let subject = display_text(subject, "(No subject)", 240);
        #[cfg(target_os = "macos")]
        {
            let _ = runtime;
            if self.status() != Status::Pending {
                macos::show(
                    &sender,
                    &subject,
                    self.enabled.clone(),
                    self.status.clone(),
                    context.clone(),
                );
            }
        }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            let enabled = self.enabled.clone();
            let status = self.status.clone();
            let context = context.clone();
            #[cfg(target_os = "linux")]
            let shown = self.shown.clone();
            runtime.spawn_message(async move {
                if !enabled.load(Ordering::Acquire) {
                    return None;
                }
                #[cfg(target_os = "linux")]
                let result = linux::show(&sender, &subject, enabled, shown).await;
                #[cfg(target_os = "windows")]
                let result = tokio::task::spawn_blocking({
                    let context = context.clone();
                    move || windows::show(&sender, &subject, enabled, context)
                })
                .await
                .unwrap_or(Status::Unavailable);
                status.store(result as u8, Ordering::Release);
                context.request_repaint();
                None
            });
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        let _ = (sender, subject, runtime, context);
    }
}

impl Drop for Notifications {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Notifications are plain text; bound untrusted headers and strip controls
/// and the invisible marks the system would use to reorder or hide them.
fn display_text(text: &str, fallback: &str, limit: usize) -> String {
    let text: String = text
        .chars()
        .filter(|&ch| !ch.is_control() && !is_invisible_mark(ch))
        .take(limit)
        .collect();
    if text.trim().is_empty() {
        fallback.into()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabling_revokes_queued_work_even_after_reenabling() {
        let context = egui::Context::default();
        let mut notifications = Notifications::default();
        notifications.enabled.store(true, Ordering::Release);
        let previous = notifications.enabled.clone();
        let old_status = notifications.status.clone();
        notifications.set_enabled(false, &context);
        assert!(!previous.load(Ordering::Acquire));
        notifications.enabled.store(true, Ordering::Release);
        assert!(!previous.load(Ordering::Acquire));
        old_status.store(Status::Denied as u8, Ordering::Release);
        assert_ne!(notifications.status(), Status::Denied);
    }

    #[test]
    fn notification_headers_are_bounded_plain_text() {
        assert_eq!(display_text("Alice\0\r\n\u{1b}", "fallback", 160), "Alice");
        assert_eq!(
            display_text("Invoice \u{202e}fdp.exe\u{2066}", "fallback", 160),
            "Invoice fdp.exe"
        );
        assert_eq!(display_text("\0\n ", "fallback", 160), "fallback");
        assert_eq!(
            display_text(&"á".repeat(1_000), "fallback", 160)
                .chars()
                .count(),
            160
        );
        assert_eq!(
            display_text("<img src='https://example.com'>", "fallback", 160),
            "<img src='https://example.com'>"
        );
    }
}
