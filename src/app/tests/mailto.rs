//! Email links, which wait until a new message can open for them.

use super::*;

#[test]
fn email_links_wait_through_login_and_open_only_an_editable_new_message() {
    let mut app = App::new(Settings::default());
    let url = "mailto:team@example.org?subject=Hello&body=Hola%0D%0Amundo";
    assert_eq!(
        app.update(Message::ReceiveMailto(
            crate::mailto::Request::parse(url).unwrap()
        ))
        .units(),
        0
    );
    for auth in [
        AuthState::CheckingSession,
        AuthState::SignedOut,
        AuthState::NeedsTotp,
    ] {
        app.auth_state = auth.clone();
        assert_eq!(
            app.update(Message::OpenPendingMailto).units(),
            0,
            "{auth:?}"
        );
        assert!(app.compose().is_none(), "{auth:?}");
        assert_eq!(app.pending_mailto_count(), 1, "{auth:?}");
    }
    let _ = app.open_mailbox(MailBackend::demo(), None, None);
    assert_eq!(app.update(Message::OpenPendingMailto).units(), 0);
    let compose = app.compose().unwrap();
    assert_eq!(compose.field(ComposeField::To), "team@example.org");
    assert_eq!(compose.field(ComposeField::Subject), "Hello");
    assert_eq!(compose.field(ComposeField::Body), "Hola\nmundo");
    assert_eq!(compose.sending(), Sending::Writing);
    assert!(compose.attachments().is_empty());
    assert_eq!(app.pending_mailto_count(), 0);
}

#[test]
fn email_links_never_replace_a_blank_written_or_sending_draft() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    let id = app.compose().unwrap().id();
    let _ = app.update(Message::ReceiveMailto(
        crate::mailto::Request::parse("mailto:next@example.org?body=Next").unwrap(),
    ));
    let _ = app.update(Message::OpenPendingMailto);
    assert_eq!(app.compose().unwrap().id(), id);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::To,
        "current@example.org".into(),
    ));
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "My draft".into(),
    ));
    let _ = app.update(Message::OpenPendingMailto);
    assert_eq!(app.compose().unwrap().field(ComposeField::Body), "My draft");
    let sending = app.update(Message::Send);
    assert!(sending.units() > 0);
    assert!(app.compose().unwrap().in_flight());
    let _ = app.update(Message::DiscardCompose);
    let _ = app.update(Message::OpenPendingMailto);
    assert_eq!(app.compose().unwrap().id(), id);
    assert_eq!(app.pending_mailto_count(), 1);
    let _ = app.update(Message::Sent(
        app.session_epoch,
        Err(SendError::Unconfirmed),
    ));
    let _ = app.update(Message::DiscardCompose);
    let _ = app.update(Message::OpenPendingMailto);
    assert_ne!(app.compose().unwrap().id(), id);
    assert_eq!(
        app.compose().unwrap().field(ComposeField::To),
        "next@example.org"
    );
}

#[test]
fn internal_email_links_use_confirmation_and_the_same_local_queue() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::LinkClicked(
        "mailto:team@example.org?subject=Confirmed".into(),
    ));
    assert!(app.pending_link().is_some());
    assert_eq!(app.pending_mailto_count(), 0);
    assert_eq!(app.update(Message::OpenLink).units(), 0);
    let _ = app.update(Message::OpenPendingMailto);
    assert_eq!(
        app.compose().unwrap().field(ComposeField::Subject),
        "Confirmed"
    );
    let _ = app.update(Message::DiscardCompose);
    let _ = app.update(Message::SetConfirmLinks(false));
    assert_eq!(
        app.update(Message::LinkClicked(
            "mailto:team@example.org?subject=Direct".into()
        ))
        .units(),
        0
    );
    let _ = app.update(Message::OpenPendingMailto);
    assert_eq!(
        app.compose().unwrap().field(ComposeField::Subject),
        "Direct"
    );
    let _ = app.update(Message::DiscardCompose);
    let _ = app.update(Message::LinkClicked(
        "mailto:team@example.org?subject=%0AInjected".into(),
    ));
    assert!(app.compose().is_none());
    assert_eq!(app.pending_mailto_count(), 0);
    assert!(app.mailto_error().is_some());
}

#[test]
fn email_links_are_bounded_dismissible_and_do_not_cross_a_closed_session() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::ShowSettings(true));
    for index in 0..crate::mailto::MAX_PENDING {
        let _ = app.update(Message::ReceiveMailto(
            crate::mailto::Request::parse(&format!("mailto:team@example.org?subject={index}"))
                .unwrap(),
        ));
    }
    let _ = app.update(Message::ReceiveMailto(
        crate::mailto::Request::parse("mailto:overflow@example.org").unwrap(),
    ));
    assert!(app.mailto_error().is_some());
    assert_eq!(app.pending_mailto_count(), crate::mailto::MAX_PENDING);
    let _ = app.update(Message::OpenPendingMailto);
    assert!(app.compose().is_none());
    let _ = app.update(Message::DismissPendingMailto);
    let _ = app.update(Message::DismissMailtoError);
    assert!(app.mailto_error().is_none());
    let _ = app.update(Message::ShowSettings(false));
    let _ = app.update(Message::OpenPendingMailto);
    assert_eq!(app.compose().unwrap().field(ComposeField::Subject), "1");
    app.close_mailbox(None);
    assert_eq!(app.pending_mailto_count(), 0);
    assert!(app.compose().is_none());
}

#[test]
fn a_demo_process_refuses_an_email_link_once_its_mailbox_is_closed() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);
    let _ = app.update(Message::ReceiveMailto(
        crate::mailto::Request::parse("mailto:team@example.org").unwrap(),
    ));
    assert_eq!(
        app.pending_mailto_count(),
        1,
        "the demo mailbox takes links"
    );
    let _ = app.update(Message::DismissPendingMailto);

    let _ = app.update(Message::Logout);
    let _ = app.update(Message::ReceiveMailto(
        crate::mailto::Request::parse("mailto:team@example.org").unwrap(),
    ));

    assert_eq!(
        app.pending_mailto_count(),
        0,
        "no sign-in could ever open it"
    );
    assert!(
        app.mailto_error()
            .is_some_and(|error| error.contains("RUSTON_DEMO"))
    );
}
