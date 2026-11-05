//! Starting up, signing in and its prompts, and the demo's sign-in.

use super::*;

fn signing_in_app() -> App {
    let mut app = App::new(Settings::default());
    app.auth_state = AuthState::SigningIn(SignInStep::Credentials);
    app.login_form.username = "username sentinel".into();
    app.login_form.password = "password sentinel".to_owned().into();
    app
}

/// A sign-in in its own `NeedsHumanVerification` step, and the answer the
/// browser check waits for.
fn verifying_app() -> (App, futures::channel::oneshot::Receiver<()>) {
    let mut app = signing_in_app();
    let (done, answer) = crate::mail::PromptAnswer::channel();
    let url = "https://verify.proton.me/?methods=captcha&token=t".to_owned();
    let _ = app.update(Message::SignInPrompt(
        app.auth_attempt,
        SignInPrompt::HumanVerification { url, done },
    ));
    (app, answer)
}

#[test]
fn initial_state_checks_saved_session() {
    let app = App::new(Settings::default());

    assert_eq!(app.auth_state, AuthState::CheckingSession);
}

#[test]
fn only_a_session_found_saved_at_startup_is_opened_as_the_mailbox() {
    let mut app = App::new(Settings::default());
    assert!(!app.opening_saved_session());

    app.session_saved = true;
    assert!(app.opening_saved_session());
    let _ = app.update(Message::SessionChecked(ResumeOutcome::SignedOut));
    assert!(!app.opening_saved_session());

    let (demo, _) = App::boot(true, Settings::default());
    assert!(!demo.opening_saved_session());
}

#[test]
fn normal_startup_checks_the_proton_session() {
    let (app, _) = App::boot(false, Settings::default());

    assert_eq!(app.auth_state, AuthState::CheckingSession);
    assert!(!app.is_demo());
    assert!(app.backend.is_none());
    assert!(app.mailbox.is_none());
}

#[test]
fn demo_startup_opens_mailbox_without_session_resume() {
    let (app, _) = App::boot(true, Settings::default());

    assert!(app.is_demo());
    assert_eq!(app.auth_state, AuthState::Authenticated { email: None });
    let mailbox = app.mailbox().unwrap();
    assert_eq!(mailbox.folder(), &sys(MailFolder::Inbox));
    assert_eq!(mailbox.status(), ListStatus::Loading(1));
}

#[test]
fn exiting_demo_clears_mailbox_and_opens_login() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);

    let _ = app.update(Message::Logout);

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::DemoSignInUnavailable));
    assert!(app.backend.is_none());
    assert!(app.mailbox.is_none());
    assert!(!app.is_demo());
}

#[test]
fn a_demo_process_never_starts_a_proton_sign_in() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);
    let _ = app.update(Message::Logout);
    app.login_form.username = "username sentinel".into();
    app.login_form.password = "password sentinel".to_owned().into();
    let attempt = app.auth_attempt;

    let effects = app.update(Message::Submit);

    assert_eq!(effects.units(), 0);
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::DemoSignInUnavailable));
    assert_eq!(app.auth_attempt, attempt);
    assert!(app.backend.is_none());
}

#[test]
fn a_demo_process_keeps_its_notice_while_the_form_is_edited() {
    let (mut app, _) = App::boot(true, Settings::default());
    deliver_demo_page(&mut app, OPENING_PAGE, 0);
    let _ = app.update(Message::Logout);

    app.login_form.username = "username sentinel".into();
    app.login_edited();

    assert_eq!(app.auth_error, Some(AuthError::DemoSignInUnavailable));
}

#[test]
fn a_normal_process_clears_its_error_when_the_form_is_edited() {
    let (mut app, _) = App::boot(false, Settings::default());
    let _ = app.update(Message::SessionChecked(ResumeOutcome::SignedOut));
    let _ = app.update(Message::Submit);
    assert_eq!(app.auth_error, Some(AuthError::UsernameRequired));

    app.login_form.username = "username sentinel".into();
    app.login_edited();

    assert_eq!(app.auth_error, None);
}

#[test]
fn a_normal_process_still_starts_a_proton_sign_in() {
    let (mut app, _) = App::boot(false, Settings::default());
    let _ = app.update(Message::SessionChecked(ResumeOutcome::SignedOut));
    app.login_form.username = "username sentinel".into();
    app.login_form.password = "password sentinel".to_owned().into();

    let effects = app.update(Message::Submit);

    assert_eq!(effects.units(), 1);
    assert_eq!(
        app.auth_state,
        AuthState::SigningIn(SignInStep::Credentials)
    );
    assert_eq!(app.auth_error, None);
}

#[test]
fn an_offline_start_keeps_the_saved_session_to_try_again() {
    let mut app = App::new(Settings::default());

    let _ = app.update(Message::SessionChecked(ResumeOutcome::Failed(
        AuthError::Connection,
    )));
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::Connection));
    assert!(app.can_retry_saved_session());

    let retry = app.update(Message::RetrySavedSession);
    assert_eq!(retry.units(), 1);
    assert_eq!(app.auth_state, AuthState::CheckingSession);
    assert_eq!(app.auth_error, None);
    assert!(!app.can_retry_saved_session());
    // A second press while it runs starts nothing more.
    assert_eq!(app.update(Message::RetrySavedSession).units(), 0);
}

#[test]
fn only_a_failure_that_may_pass_offers_the_saved_session_again() {
    for (outcome, retry) in [
        (ResumeOutcome::Failed(AuthError::TimedOut), true),
        (ResumeOutcome::Failed(AuthError::SessionUnavailable), true),
        (
            ResumeOutcome::Failed(AuthError::Service {
                http_status: 503,
                code: 0,
            }),
            true,
        ),
        (
            ResumeOutcome::Failed(AuthError::Service {
                http_status: 422,
                code: 2001,
            }),
            false,
        ),
        (
            ResumeOutcome::Failed(AuthError::SessionAddressMismatch),
            false,
        ),
        (ResumeOutcome::SignedOut, false),
    ] {
        let label = match &outcome {
            ResumeOutcome::Failed(error) => format!("{error:?}"),
            _ => "signed out".to_owned(),
        };
        let mut app = App::new(Settings::default());
        let _ = app.update(Message::SessionChecked(outcome));
        assert_eq!(app.can_retry_saved_session(), retry, "{label}");
    }
}

#[test]
fn signing_in_instead_leaves_nothing_to_retry_and_ignores_a_late_check() {
    let mut app = App::new(Settings::default());
    let _ = app.update(Message::SessionChecked(ResumeOutcome::Failed(
        AuthError::Connection,
    )));
    app.login_form.username = "username sentinel".into();
    app.login_form.password = "password sentinel".to_owned().into();

    let _ = app.update(Message::Submit);
    assert!(!app.can_retry_saved_session());
    assert_eq!(app.update(Message::RetrySavedSession).units(), 0);

    // A check that answers once a sign-in runs changes nothing.
    let _ = app.update(Message::SessionChecked(ResumeOutcome::SignedOut));
    assert_eq!(
        app.auth_state,
        AuthState::SigningIn(SignInStep::Credentials)
    );

    // Back on the form after that sign-in failed, its error is the one
    // shown, with no offer to try the old session beside it.
    let _ = app.update(Message::SignInFinished(
        app.auth_attempt,
        SignInOutcome::Failed(AuthError::Connection),
    ));
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(!app.can_retry_saved_session());
}

#[test]
fn absent_saved_session_opens_login() {
    let mut app = App::new(Settings::default());

    let _ = app.update(Message::SessionChecked(ResumeOutcome::SignedOut));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, None);
}

#[test]
fn totp_prompt_is_answered_within_the_same_sign_in() {
    let mut app = signing_in_app();
    let (reply, mut answer) = crate::mail::PromptAnswer::channel();

    let _ = app.update(Message::SignInPrompt(
        app.auth_attempt,
        SignInPrompt::Totp(reply),
    ));
    assert_eq!(app.auth_state, AuthState::NeedsTotp);
    assert_eq!(app.login_form.username, "username sentinel");
    assert_eq!(app.login_form.password.as_str(), "password sentinel");

    let _ = app.update(Message::Submit);
    assert_eq!(app.auth_error, Some(AuthError::TotpRequired));
    assert!(matches!(answer.try_recv(), Ok(None)));

    *app.totp_mut() = " 123456 ".into();
    app.login_edited();
    let _ = app.update(Message::Submit);
    let code = answer.try_recv().unwrap().unwrap();
    assert_eq!(code.expose_secret(), "123456");
    assert_eq!(app.auth_state, AuthState::SigningIn(SignInStep::Totp));
    assert!(app.login_form.totp.is_empty());
}

#[test]
fn mailbox_password_prompt_is_answered_within_the_same_sign_in() {
    let mut app = signing_in_app();
    let attempt = app.auth_attempt;
    let (reply, mut answer) = crate::mail::PromptAnswer::channel();

    let _ = app.update(Message::SignInPrompt(
        attempt,
        SignInPrompt::MailboxPassword(reply),
    ));
    assert_eq!(app.auth_state, AuthState::NeedsMailboxPassword);
    assert_eq!(app.login_form.password.as_str(), "password sentinel");

    let _ = app.update(Message::Submit);
    assert_eq!(app.auth_error, Some(AuthError::MailboxPasswordRequired));
    assert!(matches!(answer.try_recv(), Ok(None)));

    *app.mailbox_password_mut() = " two words ".into();
    app.login_edited();
    let _ = app.update(Message::Submit);
    // Taken exactly as typed, spaces included.
    let password = answer.try_recv().unwrap().unwrap();
    assert_eq!(password.expose_secret(), " two words ");
    assert_eq!(
        app.auth_state,
        AuthState::SigningIn(SignInStep::MailboxPassword)
    );
    // The same sign-in goes on: no new attempt asks for the password or a
    // code again.
    assert_eq!(app.auth_attempt, attempt);
    assert!(app.login_form.mailbox_password.is_empty());
}

#[test]
fn human_verification_waits_for_confirmation() {
    let mut app = signing_in_app();
    let (done, mut answer) = crate::mail::PromptAnswer::channel();
    let url = "https://verify.proton.me/?methods=captcha&token=t".to_owned();

    let _ = app.update(Message::SignInPrompt(
        app.auth_attempt,
        SignInPrompt::HumanVerification {
            url: url.clone(),
            done,
        },
    ));
    assert_eq!(app.auth_state, AuthState::NeedsHumanVerification { url });
    assert_eq!(answer.try_recv(), Ok(None));

    let _ = app.update(Message::Submit);
    assert_eq!(answer.try_recv(), Ok(Some(())));
    assert!(matches!(app.auth_state, AuthState::SigningIn(_)));
}

#[test]
fn a_sign_in_waiting_for_the_browser_check_is_still_running() {
    let (mut app, mut answer) = verifying_app();
    let effects = app.update(Message::CancelChallenge);
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(answer.try_recv().is_err());
    assert!(matches!(
        effects.into_iter().next(),
        Some(Effect::CancelSignIn)
    ));

    // Its outcome still applies while the check is open.
    let (mut app, _answer) = verifying_app();
    let _ = app.update(Message::SignInFinished(
        app.auth_attempt,
        SignInOutcome::Failed(AuthError::Connection),
    ));
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::Connection));
}

#[test]
fn going_back_cancels_the_pending_prompt() {
    let mut app = signing_in_app();
    let attempt = app.auth_attempt;
    let (reply, mut answer) = crate::mail::PromptAnswer::<SecretString>::channel();
    let _ = app.update(Message::SignInPrompt(attempt, SignInPrompt::Totp(reply)));

    let effects = app.update(Message::CancelChallenge);
    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(app.login_form.password.is_empty());
    assert!(answer.try_recv().is_err());
    assert!(matches!(
        effects.into_iter().next(),
        Some(Effect::CancelSignIn)
    ));

    *app.password_mut() = "new password".into();
    let _ = app.update(Message::Submit);
    assert!(matches!(app.auth_state, AuthState::SigningIn(_)));
    assert_ne!(app.auth_attempt, attempt);

    let _ = app.update(Message::SignInFinished(
        attempt,
        SignInOutcome::Failed(AuthError::Connection),
    ));
    assert!(matches!(app.auth_state, AuthState::SigningIn(_)));
    assert_eq!(app.auth_error, None);
}

#[test]
fn prompts_without_a_running_sign_in_are_cancelled() {
    let mut app = App::signed_out();
    let (reply, mut answer) = crate::mail::PromptAnswer::<SecretString>::channel();

    let _ = app.update(Message::SignInPrompt(
        app.auth_attempt,
        SignInPrompt::Totp(reply),
    ));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(answer.try_recv().is_err());
}

#[test]
fn terminal_sign_in_failure_clears_secrets() {
    let mut app = App::new(Settings::default());
    app.auth_state = AuthState::SigningIn(SignInStep::Credentials);
    app.login_form.password = "password sentinel".to_owned().into();
    app.login_form.totp = "totp sentinel".to_owned().into();
    app.login_form.mailbox_password = "mailbox password sentinel".to_owned().into();

    let _ = app.update(Message::SignInFinished(
        app.auth_attempt,
        SignInOutcome::Failed(AuthError::Connection),
    ));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert!(app.login_form.password.is_empty());
    assert!(app.login_form.totp.is_empty());
    assert!(app.login_form.mailbox_password.is_empty());
}
