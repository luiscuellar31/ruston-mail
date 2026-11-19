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

/// The addresses of the accounts the sign-in screen offers instead.
fn others(app: &App) -> Vec<Option<&str>> {
    app.other_accounts()
        .map(|account| account.address.as_deref())
        .collect()
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
    assert_eq!(others(&app), [Some("alex@proton.me")]);
    signed_in_as(&mut app, "sam@proton.me", "user-2");
    assert_eq!(app.accounts().active_profile(), "ruston-2");
    assert_eq!(address_of(&app, "ruston-2"), Some("sam@proton.me"));
    assert!(others(&app).is_empty());

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
    // Its session is gone: it signs in again, as the address it had, or
    // another account opens instead.
    let _ = app.update(Message::SessionChecked(
        PROFILE.into(),
        ResumeOutcome::SignedOut,
    ));
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.username_mut().as_str(), "alex@proton.me");
    assert_eq!(others(&app), [Some("sam@proton.me")]);
    assert_eq!(
        app.update(Message::SwitchAccount("ruston-2".into()))
            .units(),
        1
    );
    assert_eq!(app.accounts().active_profile(), "ruston-2");
    assert_eq!(app.auth_state, AuthState::CheckingSession);
    assert!(app.username_mut().is_empty());
}

#[test]
fn opening_another_account_instead_of_adding_one_adds_nothing() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    let _ = app.update(Message::AddAccount);
    assert_eq!(
        app.update(Message::SwitchAccount(PROFILE.into())).units(),
        1
    );
    assert_eq!(app.session_profile(), PROFILE);
    assert_eq!(app.auth_state, AuthState::CheckingSession);
    assert_eq!(app.accounts().all().len(), 1);
}

#[test]
fn an_account_whose_session_ended_holds_none_of_the_others_back_at_startup() {
    let settings = App::signed_in_to(&["alex@proton.me", "sam@proton.me"])
        .settings()
        .clone();
    let (mut app, _) = App::boot(false, settings);
    let _ = app.update(Message::SessionChecked(
        "ruston-2".into(),
        ResumeOutcome::SignedOut,
    ));
    assert_eq!(app.username_mut().as_str(), "sam@proton.me");
    assert_eq!(others(&app), [Some("alex@proton.me")]);
    assert_eq!(
        app.update(Message::SwitchAccount(PROFILE.into())).units(),
        1
    );
    assert_eq!(app.accounts().active_profile(), PROFILE);
    // While a sign-in runs, none is offered.
    let _ = app.update(Message::SessionChecked(
        PROFILE.into(),
        ResumeOutcome::SignedOut,
    ));
    app.auth_state = AuthState::SigningIn(SignInStep::Credentials);
    assert!(others(&app).is_empty());
    assert_eq!(
        app.update(Message::SwitchAccount("ruston-2".into()))
            .units(),
        0
    );
}

#[test]
fn each_account_comes_back_to_the_folder_it_was_left_in() {
    let mut app = App::signed_in_to(&["alex@proton.me", "sam@proton.me"]);
    // A real account's folders are remembered, the demo mailbox's are not.
    app.backend = None;
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Archive)));

    // Alex has read no folder yet: the Inbox, not where Sam was.
    let _ = app.update(Message::SwitchAccount(PROFILE.into()));
    assert_eq!(app.settings.start_folder(), Folder::INBOX);
    signed_in_as(&mut app, "alex@proton.me", "user-1");
    app.backend = None;
    let invoices = Folder::custom("kZ9", "Invoices");
    let _ = app.update(Message::SelectFolder(invoices.clone()));

    // Each opens where it was left, a folder the account made included.
    let _ = app.update(Message::SwitchAccount("ruston-2".into()));
    assert_eq!(app.settings.start_folder(), sys(MailFolder::Archive));
    signed_in_as(&mut app, "sam@proton.me", "user-2");
    assert_eq!(app.mailbox().unwrap().folder(), &sys(MailFolder::Archive));
    let _ = app.update(Message::SwitchAccount(PROFILE.into()));
    assert_eq!(app.settings.start_folder(), invoices);
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

#[test]
fn signing_out_forgets_the_account_and_opens_the_next() {
    let mut app = App::signed_in_to(&["alex@proton.me", "sam@proton.me"]);
    app.auth_state = AuthState::SigningOut;
    let epoch = app.session_epoch;
    assert_eq!(
        app.update(Message::LogoutFinished(epoch, Ok(()))).units(),
        1
    );
    assert_eq!(app.accounts().all().len(), 1);
    assert_eq!(app.accounts().active_profile(), PROFILE);
    assert_eq!(app.auth_state, AuthState::CheckingSession);

    // The last one leaves the sign-in screen, for a first account again.
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    app.auth_state = AuthState::SigningOut;
    let epoch = app.session_epoch;
    assert_eq!(
        app.update(Message::LogoutFinished(epoch, Ok(()))).units(),
        0
    );
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.accounts(), &crate::accounts::Accounts::default());

    // A cleanup that failed may have left the session saved: it stays.
    let mut app = App::signed_in_to(&["alex@proton.me", "sam@proton.me"]);
    app.auth_state = AuthState::SigningOut;
    let epoch = app.session_epoch;
    let _ = app.update(Message::LogoutFinished(
        epoch,
        Err(AuthError::LogoutUnavailable),
    ));
    assert_eq!(app.accounts().all().len(), 2);
    assert_eq!(app.accounts().active_profile(), "ruston-2");
    assert_eq!(app.auth_state, AuthState::SignedOut);
}

#[test]
fn an_account_added_already_is_not_added_again() {
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    let _ = app.update(Message::AddAccount);
    app.login_form.username = " Alex@Proton.me ".into();
    app.login_form.password = "password".to_owned().into();
    assert_eq!(app.update(Message::Submit).units(), 0);
    assert_eq!(app.auth_error, Some(AuthError::AccountAlreadyAdded));
    assert_eq!(app.auth_state, AuthState::SignedOut);

    // Signing in to the account whose session ended is not adding it.
    let mut app = App::signed_in_as("user-1", "alex@proton.me");
    let _ = app.update(Message::Logout);
    app.auth_state = AuthState::SignedOut;
    app.login_form.username = "alex@proton.me".into();
    app.login_form.password = "password".to_owned().into();
    assert!(!app.adds_known_address());
}
