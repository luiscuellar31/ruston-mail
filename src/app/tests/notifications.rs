//! New mail notifications and the unread badge.

use super::*;

/// The counts request the open mailbox waits for.
fn pending_counts_request(app: &App) -> RequestId {
    app.mailbox()
        .and_then(Mailbox::counts_request)
        .expect("a counts request in flight")
}

fn deliver_new_mail(app: &mut App, result: Result<Vec<IncomingMail>, MailboxError>) -> Effects {
    assert_eq!(app.fetch_new_mail_notification().units(), 1);
    let request = app.new_mail.pending.unwrap();
    app.update(Message::NewMailLoaded(app.session_epoch, request, result))
}

#[test]
fn unread_badge_reflects_inbox_unread_and_preference() {
    let mut app = loaded_demo_app();
    assert_eq!(crate::ui::unread_badge_count(&app), Some(6));

    let _ = app.update(Message::SetShowUnreadBadge(false));
    assert_eq!(crate::ui::unread_badge_count(&app), None);

    let _ = app.update(Message::SetShowUnreadBadge(true));
    assert_eq!(crate::ui::unread_badge_count(&app), Some(6));
}

#[test]
fn inbox_arrivals_notify_independently_of_unread_counts() {
    let mut app = loaded_demo_app();
    let old = IncomingMail {
        id: "old".into(),
        time: 100,
        unread: true,
        sender: "Old sender".into(),
        subject: "Old mail".into(),
    };
    assert_eq!(deliver_new_mail(&mut app, Ok(vec![old.clone()])).units(), 0);
    let current = IncomingMail {
        id: "new".into(),
        time: 101,
        unread: true,
        sender: "Alice".into(),
        subject: "New mail".into(),
    };
    // Reading the previous mail leaves the unread count unchanged.
    let read_old = IncomingMail {
        unread: false,
        ..old
    };
    let effects = deliver_new_mail(&mut app, Ok(vec![current.clone(), read_old.clone()]));
    let notify_effect = effects.into_iter().next().expect("ui effect");
    assert!(matches!(
        notify_effect,
        Effect::Ui(UiEffect::NotifyNewMail { ref sender, ref subject })
            if sender == "Alice" && subject == "New mail"
    ));
    assert_eq!(
        deliver_new_mail(&mut app, Ok(vec![current, read_old])).units(),
        0
    );
}

#[test]
fn notification_polls_are_bounded_and_stale_or_disabled_results_are_ignored() {
    let mut app = loaded_demo_app();
    assert_eq!(app.fetch_new_mail_notification().units(), 1);
    let first = app.new_mail.pending.unwrap();
    assert_eq!(app.fetch_new_mail_notification().units(), 0);
    let _ = app.update(Message::SetDesktopNotifications(false));
    let _ = app.update(Message::SetDesktopNotifications(true));
    assert_eq!(app.fetch_new_mail_notification().units(), 1);
    let second = app.new_mail.pending.unwrap();
    assert_ne!(first, second);
    assert_eq!(
        app.update(Message::NewMailLoaded(
            app.session_epoch,
            first,
            Ok(Vec::new())
        ))
        .units(),
        0
    );
    assert_eq!(app.new_mail.pending, Some(second));
    assert_eq!(
        app.update(Message::NewMailLoaded(
            app.session_epoch,
            second,
            Err(MailboxError::Unavailable)
        ))
        .units(),
        0
    );
    assert!(app.new_mail.pending.is_none());
    assert_eq!(deliver_new_mail(&mut app, Ok(Vec::new())).units(), 0);
    assert_eq!(
        deliver_new_mail(&mut app, Err(MailboxError::SessionExpired)).units(),
        0
    );
    assert!(app.mailbox().is_none());
}

#[test]
fn late_new_mail_result_never_notifies_after_logout_or_in_another_session() {
    let mut app = loaded_demo_app();
    let effect = app
        .fetch_new_mail_notification()
        .into_iter()
        .next()
        .expect("notification request");
    let Effect::Future(future) = effect else {
        panic!("expected future effect");
    };
    let late_result = futures::executor::block_on(future);

    app.auth_state = AuthState::SigningOut;
    assert_eq!(app.update(late_result.clone()).units(), 0);
    app.auth_state = AuthState::Authenticated { email: None };

    let _ = app.update(Message::Logout);
    assert_eq!(app.update(late_result.clone()).units(), 0);

    let _ = app.open_mailbox(MailBackend::demo(), None, None);
    assert_eq!(app.update(late_result).units(), 0);
}

#[test]
fn automatic_refresh_does_not_notify_when_inbox_unread_stays_same() {
    let mut app = loaded_demo_app();
    let initial_unread = app
        .mailbox()
        .unwrap()
        .counts()
        .unwrap()
        .unread(&Folder::INBOX)
        .unwrap();

    let _ = app.update(Message::AutoRefreshMailbox);
    let counts_request = pending_counts_request(&app);

    let new_counts: crate::mail::MailboxCounts =
        [(Folder::INBOX, initial_unread)].into_iter().collect();
    let effects = app.update(Message::CountsLoaded(counts_request, Ok(new_counts)));
    assert_eq!(effects.units(), 0);
}

#[test]
fn manual_refresh_does_not_trigger_notification_when_unread_increases() {
    let mut app = loaded_demo_app();
    let initial_unread = app
        .mailbox()
        .unwrap()
        .counts()
        .unwrap()
        .unread(&Folder::INBOX)
        .unwrap();

    let _ = app.update(Message::RefreshMailbox);
    let counts_request = pending_counts_request(&app);

    let new_counts: crate::mail::MailboxCounts =
        [(Folder::INBOX, initial_unread + 1)].into_iter().collect();
    let effects = app.update(Message::CountsLoaded(counts_request, Ok(new_counts)));
    assert_eq!(effects.units(), 0);
}

#[test]
fn automatic_refresh_does_not_notify_when_desktop_notifications_disabled() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SetDesktopNotifications(false));
    assert!(!app.settings().desktop_notifications);

    let initial_unread = app
        .mailbox()
        .unwrap()
        .counts()
        .unwrap()
        .unread(&Folder::INBOX)
        .unwrap();

    let _ = app.update(Message::AutoRefreshMailbox);
    let counts_request = pending_counts_request(&app);

    let new_counts: crate::mail::MailboxCounts =
        [(Folder::INBOX, initial_unread + 1)].into_iter().collect();
    let effects = app.update(Message::CountsLoaded(counts_request, Ok(new_counts)));
    assert_eq!(effects.units(), 0);
}
