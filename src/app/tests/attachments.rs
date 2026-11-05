//! Saving attachments: one at a time, Save as, and what a failed save says.

use super::*;

fn deliver_attachment(app: &mut App, result: Result<PathBuf, SaveError>) {
    let request = app.saving_attachment.as_ref().unwrap().request.clone();
    let _ = app.update(Message::AttachmentSaved(request, result));
}

/// Whether the app neither confirms a saved file nor reports a failed save.
fn nothing_said_about_saves(app: &App) -> bool {
    app.notice().is_none() && app.failed_save().is_none()
}

fn choose_attachment_destination(app: &mut App) -> AttachmentSaveRequest {
    let effects = app.update(Message::SaveAttachmentAs(
        "demo-0".into(),
        "file".into(),
        "../../report.pdf".into(),
    ));
    let mut effects = effects.into_iter();
    let Some(Effect::Ui(UiEffect::PickAttachmentDestination {
        request,
        suggested_name,
    })) = effects.next()
    else {
        panic!("save as should only request a native destination dialog");
    };
    assert_eq!(suggested_name, "report.pdf");
    assert!(
        effects.next().is_none(),
        "no download before the destination is chosen"
    );
    request
}

#[test]
fn a_save_in_flight_does_not_outlive_the_session() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "file".into()));
    let previous_request = app.saving_attachment.as_ref().unwrap().request.clone();
    assert_eq!(app.saving_attachment(), Some("file"));

    let _ = app.update(Message::Logout);
    assert_eq!(app.saving_attachment(), None);
    assert!(nothing_said_about_saves(&app));

    let _ = app.open_mailbox(MailBackend::demo(), None, None);
    let _ = app.update(Message::SaveAttachment("demo-1".into(), "new-file".into()));

    // A late save from the previous session must not replace the new one.
    let _ = app.update(Message::AttachmentSaved(
        previous_request,
        Ok(PathBuf::from("/tmp/x.pdf")),
    ));
    assert!(nothing_said_about_saves(&app));
    assert_eq!(app.saving_attachment(), Some("new-file"));
}

#[test]
fn a_sent_message_and_a_saved_file_are_confirmed_until_their_notice_goes() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::OpenCompose);
    deliver_send(&mut app, Ok(()));
    assert_eq!(app.notice(), Some(&Notice::Sent));

    let saved = PathBuf::from("/tmp/report.pdf");
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "file".into()));
    deliver_attachment(&mut app, Ok(saved.clone()));
    assert_eq!(app.notice(), Some(&Notice::Saved(saved.clone())));
    // The time of an older notice running out takes nothing newer with it.
    let _ = app.update(Message::DismissNotice(Notice::Sent));
    assert_eq!(app.notice(), Some(&Notice::Saved(saved.clone())));
    let _ = app.update(Message::DismissNotice(Notice::Saved(saved)));
    assert_eq!(app.notice(), None);

    // A failed save is kept for its own file instead.
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "file".into()));
    deliver_attachment(&mut app, Err(SaveError::Failed));
    assert_eq!(app.failed_save(), Some(("file", SaveError::Failed)));
    assert_eq!(app.notice(), None);
}

#[test]
fn revealing_an_attachment_starts_a_background_task() {
    let mut app = loaded_demo_app();
    let effects = app.update(Message::RevealAttachment(PathBuf::from("/tmp/report.pdf")));
    assert_eq!(effects.units(), 1);
}

#[test]
fn one_attachment_is_saved_at_a_time() {
    let mut app = loaded_demo_app();

    // A second press must not start a duplicate fetch.
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "file".into()));
    assert_eq!(app.saving_attachment(), Some("file"));

    let _ = app.update(Message::SaveAttachment("demo-0".into(), "other".into()));
    assert_eq!(app.saving_attachment(), Some("file"));

    deliver_attachment(&mut app, Err(SaveError::NotFetched));
    assert_eq!(app.saving_attachment(), None);
    // Once the first one is done, the next press is free to go.
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "other".into()));
    assert_eq!(app.saving_attachment(), Some("other"));
}

#[test]
fn cancelling_save_as_does_not_download_or_change_a_later_save() {
    let mut app = loaded_demo_app();
    let first = choose_attachment_destination(&mut app);
    assert_eq!(
        app.update(Message::SaveAttachment("demo-0".into(), "other".into()))
            .units(),
        0
    );
    assert_eq!(
        app.update(Message::AttachmentDestinationChosen(first.clone(), None))
            .units(),
        0
    );
    assert!(app.saving_attachment().is_none());
    assert!(nothing_said_about_saves(&app));

    let second = choose_attachment_destination(&mut app);
    assert_ne!(first, second);
    for (case, stale) in [
        (
            "a destination for the earlier download",
            Message::AttachmentDestinationChosen(
                first.clone(),
                Some(std::env::temp_dir().join("stale.pdf")),
            ),
        ),
        (
            "no destination for the earlier download",
            Message::AttachmentDestinationChosen(first.clone(), None),
        ),
        (
            "the earlier download's result",
            Message::AttachmentSaved(first, Err(SaveError::Failed)),
        ),
        // A completion cannot skip the destination-selection phase.
        (
            "a result before its destination",
            Message::AttachmentSaved(second, Err(SaveError::Failed)),
        ),
    ] {
        assert_eq!(app.update(stale).units(), 0, "{case}");
        assert_eq!(app.saving_attachment(), Some("file"), "{case}");
        assert!(nothing_said_about_saves(&app), "{case}");
    }
}

#[tokio::test]
async fn chosen_destination_starts_one_download_and_failure_creates_no_file() {
    let mut app = loaded_demo_app();
    let request = choose_attachment_destination(&mut app);
    let path = std::env::temp_dir().join(format!("ruston-unfetched-{}.pdf", std::process::id()));
    assert!(!path.exists());
    let effects = app.update(Message::AttachmentDestinationChosen(
        request.clone(),
        Some(path.clone()),
    ));
    assert_eq!(effects.units(), 1);
    assert_eq!(
        app.update(Message::AttachmentDestinationChosen(
            request,
            Some(path.clone())
        ))
        .units(),
        0
    );
    let Some(Effect::Future(task)) = effects.into_iter().next() else {
        panic!("expected attachment download");
    };
    let _ = app.update(task.await);
    assert!(app.saving_attachment().is_none());
    assert_eq!(app.failed_save(), Some(("file", SaveError::NotFetched)));
    assert!(
        !path.exists(),
        "failed downloads must not create the selected file"
    );
}

#[test]
fn save_as_dialog_results_after_sign_out_cannot_start_a_download() {
    let mut app = loaded_demo_app();
    let previous = choose_attachment_destination(&mut app);
    let _ = app.update(Message::Logout);
    let _ = app.open_mailbox(MailBackend::demo(), None, None);
    let current = choose_attachment_destination(&mut app);
    assert_ne!(previous.session, current.session);
    assert_eq!(
        app.update(Message::AttachmentDestinationChosen(
            previous,
            Some(std::env::temp_dir().join("stale.pdf"))
        ))
        .units(),
        0
    );
    assert_eq!(app.saving_attachment.as_ref().unwrap().request, current);
}

#[test]
fn an_attachment_save_that_finds_the_session_expired_signs_out() {
    let mut app = loaded_demo_app();
    let _ = app.update(Message::SaveAttachment("demo-0".into(), "file".into()));

    deliver_attachment(&mut app, Err(SaveError::SessionExpired));

    assert_eq!(app.auth_state, AuthState::SignedOut);
    assert_eq!(app.auth_error, Some(AuthError::SessionExpired));
    assert!(app.mailbox.is_none());
}

#[test]
fn a_failed_download_says_why_the_file_was_not_saved() {
    for (error, saved) in [
        (
            MailboxError::AttachmentVerificationFailed,
            SaveError::VerificationFailed,
        ),
        (MailboxError::SessionExpired, SaveError::SessionExpired),
        (
            MailboxError::SessionUnavailable,
            SaveError::SessionUnavailable,
        ),
        (MailboxError::Connection, SaveError::NotFetched),
        (MailboxError::Service, SaveError::NotFetched),
        (MailboxError::Unavailable, SaveError::NotFetched),
    ] {
        assert_eq!(super::unsaved(error), saved, "{error:?}");
    }
}
