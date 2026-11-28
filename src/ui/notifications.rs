//! Native delivery and permission state. Mail detection belongs to App.

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use eframe::egui;
use ruston_core::text::is_invisible_mark;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use crate::runtime::Runtime;

/// The most characters of a sender a notification shows.
const MAX_SENDER_CHARS: usize = 160;
/// The most characters of a subject a notification shows.
const MAX_SUBJECT_CHARS: usize = 240;

/// Why notifications cannot be shown here: macOS sends them only from the
/// installed app.
#[cfg(target_os = "macos")]
const UNAVAILABLE: &str =
    "Notifications need the installed Ruston Mail.app and an available system service.";
/// Why notifications cannot be shown here.
#[cfg(not(target_os = "macos"))]
const UNAVAILABLE: &str =
    "The system notification service is unavailable. Mail refresh will continue.";

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
    /// The status stored as `value`, by the numbers the variants carry, so
    /// they are written once; a value none carries is Off.
    fn from_u8(value: u8) -> Self {
        [Self::Ready, Self::Pending, Self::Denied, Self::Unavailable]
            .into_iter()
            .find(|status| *status as u8 == value)
            .unwrap_or_default()
    }

    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Off => "Notifications are inactive until enabled for a signed-in account.",
            Self::Ready => "The operating system controls delivery, sounds, and Do Not Disturb.",
            Self::Pending => "Waiting for the operating system's notification permission.",
            Self::Denied => {
                "Notifications are blocked. Allow Ruston Mail in system notification settings."
            }
            Self::Unavailable => UNAVAILABLE,
        }
    }
}

/// The status the window reads and the system's answers write, from
/// whichever thread they arrive on. Only this type knows it is kept as the
/// number its variant carries.
#[derive(Clone, Default)]
struct SharedStatus(Arc<AtomicU8>);

impl SharedStatus {
    fn get(&self) -> Status {
        Status::from_u8(self.0.load(Ordering::Acquire))
    }

    fn set(&self, status: Status) {
        self.0.store(status as u8, Ordering::Release);
    }
}

#[derive(Default)]
pub(super) struct Notifications {
    enabled: Arc<AtomicBool>,
    status: SharedStatus,
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
        self.status.get()
    }

    pub(super) fn set_enabled(&mut self, enabled: bool, context: &egui::Context) {
        if self.enabled.load(Ordering::Acquire) == enabled {
            return;
        }
        // Revoke the previous activation permanently: queued native work must
        // not become valid again when another account enables notifications.
        self.enabled.store(false, Ordering::Release);
        self.enabled = Arc::new(AtomicBool::new(enabled));
        self.status = SharedStatus::default();
        if enabled {
            activate(&self.status, context);
        } else {
            self.withdraw();
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

    /// Takes the notifications already delivered away with the account:
    /// macOS keeps them in Notification Center.
    #[cfg(target_os = "macos")]
    fn withdraw(&self) {
        macos::clear();
    }

    /// Tests on Windows leave the machine's toasts alone, and other systems
    /// have none to take away.
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        all(target_os = "windows", not(test))
    )))]
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
        let sender = display_text(sender, "Proton Mail", MAX_SENDER_CHARS);
        let subject = display_text(subject, "(No subject)", MAX_SUBJECT_CHARS);
        self.deliver(sender, subject, runtime, context);
    }

    /// Hands the notification to macOS, unless its permission is still
    /// being asked for.
    #[cfg(target_os = "macos")]
    fn deliver(&self, sender: String, subject: String, _: &Runtime, context: &egui::Context) {
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

    /// Hands the notification to the desktop's notification service, off the
    /// interface thread.
    #[cfg(target_os = "linux")]
    fn deliver(&self, sender: String, subject: String, runtime: &Runtime, context: &egui::Context) {
        let enabled = self.enabled.clone();
        let shown = self.shown.clone();
        self.spawn_delivery(runtime, context, async move {
            linux::show(&sender, &subject, enabled, shown).await
        });
    }

    /// Hands the notification to Windows, on a blocking thread, as its
    /// notification API asks.
    #[cfg(target_os = "windows")]
    fn deliver(&self, sender: String, subject: String, runtime: &Runtime, context: &egui::Context) {
        let enabled = self.enabled.clone();
        let toast_context = context.clone();
        self.spawn_delivery(runtime, context, async move {
            tokio::task::spawn_blocking(move || {
                windows::show(&sender, &subject, enabled, toast_context)
            })
            .await
            .unwrap_or(Status::Unavailable)
        });
    }

    /// Other systems have no notifications to show.
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    fn deliver(&self, _: String, _: String, _: &Runtime, _: &egui::Context) {}

    /// Runs `delivery` on the runtime, unless notifications are turned off
    /// before it starts, and keeps the status it reports.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    fn spawn_delivery(
        &self,
        runtime: &Runtime,
        context: &egui::Context,
        delivery: impl std::future::Future<Output = Status> + Send + 'static,
    ) {
        let enabled = self.enabled.clone();
        let status = self.status.clone();
        let context = context.clone();
        runtime.spawn_message(async move {
            if !enabled.load(Ordering::Acquire) {
                return None;
            }
            status.set(delivery.await);
            context.request_repaint();
            None
        });
    }
}

/// Lets an account that turned notifications on have them: macOS asks the
/// system's permission first.
#[cfg(target_os = "macos")]
fn activate(status: &SharedStatus, context: &egui::Context) {
    macos::request_permission(status.clone(), context.clone());
}

/// Lets an account that turned notifications on have them: other systems
/// ask no permission of an app.
#[cfg(not(target_os = "macos"))]
fn activate(status: &SharedStatus, _: &egui::Context) {
    status.set(Status::Ready);
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
    fn each_status_reads_back_as_it_was_stored() {
        for status in [
            Status::Off,
            Status::Ready,
            Status::Pending,
            Status::Denied,
            Status::Unavailable,
        ] {
            assert_eq!(Status::from_u8(status as u8), status);
        }
        assert_eq!(Status::from_u8(u8::MAX), Status::Off);
    }

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
        old_status.set(Status::Denied);
        assert_ne!(notifications.status(), Status::Denied);
    }

    #[test]
    fn notification_headers_are_bounded_plain_text() {
        assert_eq!(
            display_text("Alice\0\r\n\u{1b}", "fallback", MAX_SENDER_CHARS),
            "Alice"
        );
        assert_eq!(
            display_text(
                "Invoice \u{202e}fdp.exe\u{2066}",
                "fallback",
                MAX_SENDER_CHARS
            ),
            "Invoice fdp.exe"
        );
        assert_eq!(
            display_text("\0\n ", "fallback", MAX_SENDER_CHARS),
            "fallback"
        );
        assert_eq!(
            display_text(&"á".repeat(1_000), "fallback", MAX_SENDER_CHARS)
                .chars()
                .count(),
            160
        );
        assert_eq!(
            display_text(
                "<img src='https://example.com'>",
                "fallback",
                MAX_SENDER_CHARS
            ),
            "<img src='https://example.com'>"
        );
    }
}
