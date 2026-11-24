//! Email links, which wait until a new message can open for them.

use super::*;

/// Hands the app the email link `url`, as the system would.
fn receive_mailto(app: &mut App, url: &str) -> Effects {
    app.update(Message::ReceiveMailto(
        crate::mailto::Request::parse(url).unwrap(),
    ))
}

#[test]
fn email_links_wait_through_login_and_open_only_an_editable_new_message() {
    let mut app = App::new(Settings::default());
    let url = "mailto:team@example.org?subject=Hello&body=Hola%0D%0Amundo";
    assert_eq!(receive_mailto(&mut app, url).units(), 0);

    // Before a mailbox opens, the link waits.
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

    // Once one does, it opens as a new message still being written.
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
    let _ = receive_mailto(&mut app, "mailto:next@example.org?body=Next");

    // A blank draft stays.
    let _ = app.update(Message::OpenPendingMailto);
    assert_eq!(app.compose().unwrap().id(), id);

    // So does a written one.
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

    // And one on its way, which cannot be discarded either.
    let sending = app.update(Message::Send);
    assert!(sending.units() > 0);
    assert!(app.compose().unwrap().in_flight());
    let _ = app.update(Message::DiscardCompose);
    let _ = app.update(Message::OpenPendingMailto);
    assert_eq!(app.compose().unwrap().id(), id);
    assert_eq!(app.pending_mailto_count(), 1);

    // Once the send fails and the draft is discarded, the link opens.
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

    // A link in a message asks first, then joins the queue.
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

    // With confirmation turned off, it joins the queue at once.
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

    // One that would inject a header is refused.
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
    // Settings, while shown, keeps every link waiting.
    let _ = app.update(Message::ShowSettings(true));

    // The queue holds so many links; the next is refused, with an error.
    for index in 0..crate::mailto::MAX_PENDING {
        let _ = receive_mailto(
            &mut app,
            &format!("mailto:team@example.org?subject={index}"),
        );
    }
    let _ = receive_mailto(&mut app, "mailto:overflow@example.org");
    assert!(app.mailto_error().is_some());
    assert_eq!(app.pending_mailto_count(), crate::mailto::MAX_PENDING);
    let _ = app.update(Message::OpenPendingMailto);
    assert!(app.compose().is_none());

    // Dismissing drops the first link, and the error goes too.
    let _ = app.update(Message::DismissPendingMailto);
    let _ = app.update(Message::DismissMailtoError);
    assert!(app.mailto_error().is_none());

    // Leaving Settings opens the next one: the second link received.
    let second_subject = "1";
    let _ = app.update(Message::ShowSettings(false));
    let _ = app.update(Message::OpenPendingMailto);
    assert_eq!(
        app.compose().unwrap().field(ComposeField::Subject),
        second_subject
    );

    // Closing the session takes the queue and the draft with it.
    app.close_mailbox(None);
    assert_eq!(app.pending_mailto_count(), 0);
    assert!(app.compose().is_none());
}

#[test]
fn a_demo_process_refuses_an_email_link_once_its_mailbox_is_closed() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_latest_demo_page(&mut app, 0);
    let _ = receive_mailto(&mut app, "mailto:team@example.org");
    assert_eq!(
        app.pending_mailto_count(),
        1,
        "the demo mailbox takes links"
    );
    let _ = app.update(Message::DismissPendingMailto);

    let _ = app.update(Message::Logout);
    let _ = receive_mailto(&mut app, "mailto:team@example.org");

    assert_eq!(
        app.pending_mailto_count(),
        0,
        "no sign-in could ever open it"
    );
    assert_eq!(app.mailto_error(), Some(MailtoError::Demo));
}
