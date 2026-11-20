use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use block2::RcBlock;
use eframe::egui;
use objc2::runtime::Bool;
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent,
    UNNotificationRequest, UNNotificationSettings, UNNotificationSound, UNUserNotificationCenter,
};

use super::{SharedStatus, Status};

/// Whether this is the packaged application, whose identity a notification
/// center needs: `currentNotificationCenter` can raise an Objective-C
/// exception for a bare executable, as tests run.
fn has_app_identity() -> bool {
    NSBundle::mainBundle()
        .bundleIdentifier()
        .is_some_and(|id| id.to_string() == crate::ui::identity::APP_ID)
}

pub(super) fn request_permission(status: SharedStatus, context: egui::Context) {
    if !has_app_identity() {
        status.set(Status::Unavailable);
        return;
    }
    status.set(Status::Pending);
    let completion = RcBlock::new(move |granted: Bool, error: *mut NSError| {
        let result = if !error.is_null() {
            Status::Unavailable
        } else if granted.as_bool() {
            Status::Ready
        } else {
            Status::Denied
        };
        status.set(result);
        context.request_repaint();
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &completion,
        );
}

pub(super) fn show(
    sender: &str,
    subject: &str,
    enabled: Arc<AtomicBool>,
    status: SharedStatus,
    context: egui::Context,
) {
    if !has_app_identity() {
        return;
    }
    let sender = sender.to_owned();
    let subject = subject.to_owned();
    // Read current authorization for each arrival so permission changes in
    // System Settings take effect without asking again or restarting the app.
    let settings = RcBlock::new(move |settings: std::ptr::NonNull<UNNotificationSettings>| {
        if !enabled.load(Ordering::Acquire) {
            return;
        }
        // SAFETY: Apple's completion block provides a valid settings object for
        // the duration of this callback. No reference escapes it.
        let authorization = unsafe { settings.as_ref() }.authorizationStatus();
        if authorization != UNAuthorizationStatus::Authorized
            && authorization != UNAuthorizationStatus::Provisional
        {
            status.set(Status::Denied);
            context.request_repaint();
            return;
        }
        status.set(Status::Ready);
        context.request_repaint();
        deliver(&sender, &subject, status.clone(), context.clone());
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .getNotificationSettingsWithCompletionHandler(&settings);
}

/// Takes what Ruston delivered away from the screen and Notification Center.
pub(super) fn clear() {
    if has_app_identity() {
        UNUserNotificationCenter::currentNotificationCenter().removeAllDeliveredNotifications();
    }
}

fn deliver(sender: &str, subject: &str, status: SharedStatus, context: egui::Context) {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(sender));
    content.setBody(&NSString::from_str(subject));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    // One owned identifier replaces the previous incoming-mail notification.
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str("ruston-new-mail"),
        &content,
        None,
    );
    let completion = RcBlock::new(move |error: *mut NSError| {
        if !error.is_null() {
            status.set(Status::Unavailable);
            context.request_repaint();
        }
    });
    UNUserNotificationCenter::currentNotificationCenter()
        .addNotificationRequest_withCompletionHandler(&request, Some(&completion));
}
