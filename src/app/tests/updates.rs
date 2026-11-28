//! Checking for updates: nothing asked until it is on, what each check
//! finds, and the notice a newer release gets once.

use super::*;

/// A release after the running one, and one before it.
const NEWER: &str = "999.0.0";
const OLDER: &str = "0.0.1";

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// The demo with checking for updates turned on.
fn checking_app() -> App {
    let (mut app, _) = App::boot(true, Settings::default());
    let _ = app.update(Message::SetCheckForUpdates(true));
    app
}

#[test]
fn nothing_is_asked_until_checking_is_turned_on() {
    let (mut app, _) = App::boot(true, Settings::default());
    assert_eq!(app.update(Message::CheckForUpdates).units(), 0);
    assert_eq!(app.update_status(), None);

    // An answer to a check nobody asked for is not read.
    let _ = app.update(Message::UpdatesChecked(Some(version(NEWER))));
    assert_eq!(app.update_status(), None);
    assert_eq!(app.notice(), None);
}

#[test]
fn turned_on_it_is_remembered_and_a_check_goes_out() {
    let (mut app, _) = App::boot(true, Settings::default());
    let revision = app.settings_revision();
    let _ = app.update(Message::SetCheckForUpdates(true));
    assert!(app.settings().check_for_updates);
    assert_eq!(app.settings_revision(), revision + 1);

    let effects = app.update(Message::CheckForUpdates);
    assert_eq!(effects.units(), 1);
    assert!(matches!(
        effects.into_iter().next(),
        Some(Effect::Future(_))
    ));
    assert_eq!(app.update_status(), Some(UpdateStatus::Checking));
}

#[test]
fn a_newer_release_is_announced_once_and_kept_for_about() {
    let mut app = checking_app();
    let _ = app.update(Message::CheckForUpdates);
    let _ = app.update(Message::UpdatesChecked(Some(version(NEWER))));
    let available = Some(UpdateStatus::Available(version(NEWER)));
    assert_eq!(app.update_status(), available);
    assert_eq!(app.notice(), Some(&Notice::UpdateAvailable(version(NEWER))));

    // The notice goes. While the next day's check asks, About keeps what
    // was found, and finding the same release does not bring the notice back.
    let _ = app.update(Message::DismissNotice(Notice::UpdateAvailable(version(
        NEWER,
    ))));
    let _ = app.update(Message::CheckForUpdates);
    assert_eq!(app.update_status(), available);
    let _ = app.update(Message::UpdatesChecked(Some(version(NEWER))));
    assert_eq!(app.notice(), None);

    // A check that fails keeps the release found...
    let _ = app.update(Message::UpdatesChecked(None));
    assert_eq!(app.update_status(), available);

    // ...and a later release is announced in turn.
    let _ = app.update(Message::UpdatesChecked(Some(version("999.1.0"))));
    assert_eq!(
        app.notice(),
        Some(&Notice::UpdateAvailable(version("999.1.0")))
    );
}

#[test]
fn the_running_release_or_an_older_one_is_the_latest() {
    let mut app = checking_app();
    let _ = app.update(Message::CheckForUpdates);
    for latest in [env!("CARGO_PKG_VERSION"), OLDER] {
        let _ = app.update(Message::UpdatesChecked(Some(version(latest))));
        assert_eq!(app.update_status(), Some(UpdateStatus::Latest), "{latest}");
    }
    assert_eq!(app.notice(), None);
}

#[test]
fn a_check_that_fails_says_so_until_one_answers() {
    let mut app = checking_app();
    let _ = app.update(Message::CheckForUpdates);
    let _ = app.update(Message::UpdatesChecked(None));
    assert_eq!(app.update_status(), Some(UpdateStatus::Failed));

    // The next check shows it asking again...
    let _ = app.update(Message::CheckForUpdates);
    assert_eq!(app.update_status(), Some(UpdateStatus::Checking));
    let _ = app.update(Message::UpdatesChecked(Some(version(OLDER))));
    assert_eq!(app.update_status(), Some(UpdateStatus::Latest));
    // ...and once one has answered, a failure keeps that answer.
    let _ = app.update(Message::UpdatesChecked(None));
    assert_eq!(app.update_status(), Some(UpdateStatus::Latest));
}

#[test]
fn turned_off_what_was_found_goes_and_late_answers_are_not_read() {
    let mut app = checking_app();
    let _ = app.update(Message::CheckForUpdates);
    let _ = app.update(Message::UpdatesChecked(Some(version(OLDER))));
    let _ = app.update(Message::CheckForUpdates);

    let _ = app.update(Message::SetCheckForUpdates(false));
    assert!(!app.settings().check_for_updates);
    assert_eq!(app.update_status(), None);
    let _ = app.update(Message::UpdatesChecked(Some(version(NEWER))));
    assert_eq!(app.update_status(), None);
    assert_eq!(app.notice(), None);
    assert_eq!(app.update(Message::CheckForUpdates).units(), 0);
}

#[test]
fn the_release_page_opens_in_the_browser() {
    let (mut app, _) = App::boot(true, Settings::default());
    let effects = app.update(Message::OpenReleasePage);
    assert_eq!(effects.units(), 1);
    assert!(matches!(
        effects.into_iter().next(),
        Some(Effect::Background(_))
    ));
}
