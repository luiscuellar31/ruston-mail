//! Several accounts: adding one, switching between them, going back from
//! the sign-in screen, and the most there can be.

use super::*;
use crate::accounts::MAX_ACCOUNTS;

/// The account under `profile`'s address, as the menu shows it.
fn address_of<'a>(app: &'a App, profile: &str) -> Option<&'a str> {
    app.accounts().get(profile)?.address.as_deref()
}

/// Signs in, under the profile being added, as `address`, as a sign-in that
/// Proton accepted would.
fn signed_in_as(app: &mut App, address: &str, user: &str) {
    let _ = app.open_mailbox(
        MailBackend::demo(),
        Some(address.to_owned()),
        Some(user.to_owned()),
    );
}

#[test]
fn an_added_account_signs_in_under_the_next_profile_and_each_keeps_its_session() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    assert_eq!(address_of(&app, PROFILE), Some("alex@proton.me"));

    // Adding leaves the open account, its session kept, for the sign-in.
    assert_eq!(app.update(Message::AddAccount).units(), 0);
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(app.mailbox.is_none());
    assert_eq!(app.session_profile(), "ruston-2");
    assert_eq!(
        app.returning_to()
            .and_then(|account| account.address.as_deref()),
        Some("alex@proton.me")
    );
    signed_in_as(&mut app, "sam@proton.me", "user-2");
    assert_eq!(app.accounts().active_profile(), "ruston-2");
    assert_eq!(address_of(&app, "ruston-2"), Some("sam@proton.me"));
    assert!(app.returning_to().is_none());

    // Switching opens the other account's saved session, as at startup.
    assert_eq!(
        app.update(Message::SwitchAccount(PROFILE.into())).units(),
        1
    );
    assert_eq!(app.auth_state, AuthState::CheckingSession);
    assert_eq!(app.accounts().active_profile(), PROFILE);
    assert!(app.mailbox.is_none());
    // A late answer for another account changes nothing.
    let _ = app.update(Message::SessionChecked(
        "ruston-2".into(),
        ResumeOutcome::SignedOut,
    ));
    assert_eq!(app.auth_state, AuthState::CheckingSession);
    // Its session is gone: it signs in again, as the address it had, with a
    // way back to the account it left.
    let _ = app.update(Message::SessionChecked(
        PROFILE.into(),
        ResumeOutcome::SignedOut,
    ));
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.username_mut().as_str(), "alex@proton.me");
    assert_eq!(
        app.returning_to()
            .and_then(|account| account.address.as_deref()),
        Some("sam@proton.me")
    );
    assert_eq!(app.update(Message::ReturnToAccount).units(), 1);
    assert_eq!(app.accounts().active_profile(), "ruston-2");
    assert_eq!(app.auth_state, AuthState::CheckingSession);
    assert!(app.username_mut().is_empty());
}

#[test]
fn going_back_from_adding_an_account_reopens_the_one_left() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    let _ = app.update(Message::AddAccount);
    assert_eq!(app.update(Message::ReturnToAccount).units(), 1);
    assert_eq!(app.session_profile(), PROFILE);
    assert_eq!(app.auth_state, AuthState::CheckingSession);
    // Nothing was added.
    assert_eq!(app.accounts().all().len(), 1);
}

#[test]
fn there_are_at_most_five_accounts() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    for n in 2..=MAX_ACCOUNTS {
        assert!(app.can_add_account());
        let _ = app.update(Message::AddAccount);
        signed_in_as(
            &mut app,
            &format!("user{n}@proton.me"),
            &format!("user-{n}"),
        );
    }
    assert_eq!(app.accounts().all().len(), MAX_ACCOUNTS);
    assert!(!app.can_add_account());
    assert_eq!(app.update(Message::AddAccount).units(), 0);
    assert!(app.auth_state.is_authenticated());
}

#[test]
fn a_written_message_or_the_demo_keeps_the_account_open() {
    // The interface asks before discarding the message.
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    write(&mut app, "sam@example.com");
    assert!(!app.can_change_account());
    let _ = app.update(Message::AddAccount);
    assert!(app.auth_state.is_authenticated());
    assert!(app.compose().is_some());

    let (mut demo, _) = App::boot(true, Settings::default());
    assert!(!demo.can_add_account());
    assert_eq!(demo.update(Message::AddAccount).units(), 0);
    assert!(demo.is_demo());
}
