//! Signing out, sessions that end, and the draft kept for the same account.

use super::*;

fn deliver_logout(app: &mut App, result: Result<(), AuthError>) {
    let epoch = app.session_epoch;
    let _ = app.update(Message::LogoutFinished(epoch, result));
}

/// Ends the session of account "user-1" the way Proton does, from its first
/// page request, while `app` holds whatever it was given.
fn session_ended_by_proton(app: &mut App) {
    let _ = app.update(Message::ConversationsLoaded(
        OPENING_PAGE,
        Err(MailboxError::SessionExpired),
    ));
}

/// Signs account `account` in again, with the demo mailbox standing in.
fn signed_in_again(app: &mut App, account: &str) -> Effects {
    app.open_mailbox(
        MailBackend::demo(),
        Some("alex@proton.me".into()),
        Some(account.into()),
    )
}

#[test]
fn failed_local_logout_closes_mailbox_and_reports_remaining_credentials() {
    let mut app = authenticated_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "private draft".to_owned(),
    ));
    app.auth_state = AuthState::SigningOut;

    deliver_logout(&mut app, Err(AuthError::LogoutUnavailable));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::LogoutUnavailable));
    assert!(
        app.error_message()
            .unwrap()
            .contains("may remain on this device")
    );
    assert!(app.mailbox().is_none());
    assert!(app.compose().is_none());
}

#[test]
fn successful_logout_opens_login_and_clears_mailbox() {
    let mut app = authenticated_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "private draft".to_owned(),
    ));
    app.auth_state = AuthState::SigningOut;

    deliver_logout(&mut app, Ok(()));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(app.mailbox.is_none());
    assert!(app.compose().is_none());
}

#[test]
fn expired_session_signs_out_and_clears_mailbox() {
    let mut app = authenticated_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "private draft".to_owned(),
    ));

    let _ = app.update(Message::ConversationsLoaded(
        OPENING_PAGE,
        Err(MailboxError::SessionExpired),
    ));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::SessionExpired));
    assert!(app.mailbox.is_none());
    assert!(app.compose().is_none());
}

#[test]
fn a_session_that_cannot_be_read_keeps_the_mailbox_and_the_draft() {
    let mut app = authenticated_app();
    let _ = app.update(Message::OpenCompose);
    let _ = app.update(Message::ComposeChanged(
        ComposeField::Body,
        "private draft".to_owned(),
    ));

    let _ = app.update(Message::ConversationsLoaded(
        OPENING_PAGE,
        Err(MailboxError::SessionUnavailable),
    ));

    // Unlocking the password storage fixes the next request.
    assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
    assert!(app.mailbox.is_some());
    assert!(app.compose().is_some());
}

#[test]
fn an_expired_session_keeps_the_draft_for_its_own_account() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    write(&mut app, "sam@example.com");
    let written = app.compose().unwrap().id();
    session_ended_by_proton(&mut app);

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::SessionExpired));
    assert!(app.compose().is_none(), "no session shows it meanwhile");
    assert_eq!(app.parked_draft_address(), Some("alex@proton.me"));

    let _ = signed_in_again(&mut app, "user-1");
    let draft = app.compose().expect("its own account gets it back");
    assert_eq!(draft.field(ComposeField::To), "sam@example.com");
    assert_eq!(draft.field(ComposeField::Body), "See you then.");
    assert_ne!(
        draft.id(),
        written,
        "nothing from the ended session reaches it"
    );
    assert!(!app.has_parked_draft());
}

#[test]
fn another_account_or_none_never_gets_a_kept_draft() {
    for next in [Some("user-2"), None] {
        let mut app = App::signed_in_as("user-1", "alex@proton.me");
        write(&mut app, "sam@example.com");
        session_ended_by_proton(&mut app);

        let _ = app.open_mailbox(MailBackend::demo(), None, next.map(str::to_owned));
        assert!(app.compose().is_none(), "{next:?}");
        assert!(!app.has_parked_draft(), "{next:?}");
        // Nor does its own account afterwards.
        app.close_mailbox(None);
        let _ = signed_in_again(&mut app, "user-1");
        assert!(app.compose().is_none(), "{next:?}");
    }
}

#[test]
fn an_untouched_draft_or_a_sign_out_keeps_nothing() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    let _ = app.update(Message::OpenCompose);
    session_ended_by_proton(&mut app);
    assert!(!app.has_parked_draft(), "nothing was written");

    let _ = signed_in_again(&mut app, "user-1");
    write(&mut app, "sam@example.com");
    app.close_mailbox(Some(AuthError::Connection));
    assert!(
        !app.has_parked_draft(),
        "only Proton ending the session keeps it"
    );
}

#[test]
fn a_message_on_its_way_when_the_session_ends_comes_back_unconfirmed() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    write(&mut app, "sam@example.com");
    let ended = app.session_epoch;
    assert_eq!(app.update(Message::Send).units(), 1);
    session_ended_by_proton(&mut app);

    let _ = signed_in_again(&mut app, "user-1");
    // Its answer belonged to the session that ended, and changes nothing.
    let _ = app.update(Message::Sent(ended, Ok(())));
    let draft = app.compose().expect("it may not have gone");
    assert_eq!(draft.sending(), Sending::Failed(SendError::Unconfirmed));
}

#[test]
fn a_send_refused_for_an_expired_session_ends_it_and_keeps_the_draft() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    write(&mut app, "sam@example.com");
    let epoch = app.session_epoch;
    let _ = app.update(Message::Send);

    let _ = app.update(Message::Sent(
        epoch,
        Err(SendError::Mailbox(MailboxError::SessionExpired)),
    ));
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::SessionExpired));

    let _ = signed_in_again(&mut app, "user-1");
    let draft = app.compose().expect("signing in again brings it back");
    assert_eq!(draft.sending(), Sending::Writing, "ready to send again");
}

#[test]
fn discarding_drops_a_kept_draft() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    write(&mut app, "sam@example.com");
    session_ended_by_proton(&mut app);

    let _ = app.update(Message::DiscardCompose);
    assert!(!app.has_parked_draft());
    let _ = signed_in_again(&mut app, "user-1");
    assert!(app.compose().is_none());
}

#[test]
fn stale_expired_session_response_is_ignored() {
    let mut app = authenticated_app();

    let _ = app.update(Message::ConversationsLoaded(
        NEVER_ISSUED,
        Err(MailboxError::SessionExpired),
    ));

    assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
    assert!(app.mailbox.is_some());
}

#[test]
fn logout_result_after_session_expiry_is_ignored() {
    let mut app = authenticated_app();
    let previous_epoch = app.session_epoch;
    app.auth_state = AuthState::SigningOut;
    let _ = app.update(Message::CountsLoaded(
        OPENING_COUNTS,
        Err(MailboxError::SessionExpired),
    ));

    let _ = app.update(Message::LogoutFinished(
        previous_epoch,
        Err(AuthError::Connection),
    ));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::SessionExpired));
}

#[test]
fn logout_result_from_a_previous_session_is_ignored() {
    let mut app = authenticated_app();
    let previous_epoch = app.session_epoch;
    app.auth_state = AuthState::SigningOut;
    let _ = app.update(Message::CountsLoaded(
        OPENING_COUNTS,
        Err(MailboxError::SessionExpired),
    ));
    let _ = app.open_mailbox(MailBackend::demo(), None, None);
    app.auth_state = AuthState::SigningOut;

    let _ = app.update(Message::LogoutFinished(previous_epoch, Ok(())));

    assert_eq!(app.auth_state, AuthState::SigningOut);
    assert!(app.mailbox().is_some());
}

#[test]
fn mailbox_actions_are_ignored_while_signing_out() {
    let mut app = authenticated_app();
    app.auth_state = AuthState::SigningOut;

    let _ = app.update(Message::SelectFolder(sys(MailFolder::Trash)));

    assert_eq!(app.mailbox().unwrap().folder(), &sys(MailFolder::Inbox));
}
