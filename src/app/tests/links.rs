//! Links: which open, the prompt before they do, and copying addresses.

use super::*;

#[test]
fn a_link_skips_the_prompt_only_when_confirmation_is_off() {
    let mut app = loaded_demo_app();

    let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
    assert!(
        app.pending_link().is_some(),
        "confirmation is on by default"
    );
    let _ = app.update(Message::DismissLink);

    let _ = app.update(Message::SetConfirmLinks(false));
    let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
    assert!(
        app.pending_link().is_none(),
        "the link opens without asking"
    );

    // Refusing what is not a web or mail link does not depend on the
    // setting: it is a rule, not a preference.
    let _ = app.update(Message::LinkClicked("javascript:alert(1)".into()));
    assert!(app.pending_link().is_none());
}

#[test]
fn copying_a_links_address_needs_no_prompt_and_only_takes_web_and_mail_links() {
    let mut app = loaded_demo_app();
    let effects = app.update(Message::CopyLinkAddress("https://example.com/a".into()));
    let effect = effects.into_iter().next().expect("a copy");
    assert!(matches!(
        effect,
        Effect::Ui(UiEffect::CopyText(text)) if text == "https://example.com/a"
    ));
    assert!(app.pending_link().is_none());

    let effects = app.update(Message::CopyLinkAddress("javascript:alert(1)".into()));
    assert_eq!(effects.units(), 0);
}

#[test]
fn copied_addresses_are_written_as_a_recipient_field_takes_them() {
    let mut app = loaded_demo_app();
    let effects = app.update(Message::CopyAddresses(vec![
        "team@example.org".into(),
        "ops@example.org".into(),
    ]));
    let effect = effects.into_iter().next().expect("a copy");
    assert!(matches!(
        effect,
        Effect::Ui(UiEffect::CopyText(text)) if text == "team@example.org, ops@example.org"
    ));
}

#[test]
fn only_web_and_mail_links_are_offered() {
    let web = PendingLink::parse(" https://Example.com/path?x=1 ").unwrap();
    assert_eq!(web.target, "example.com");
    assert_eq!(web.url, "https://example.com/path?x=1");
    assert_eq!(
        PendingLink::parse("mailto:team@example.org")
            .unwrap()
            .target,
        "team@example.org"
    );
    // The real host shows even when the link hides it behind a user name.
    assert_eq!(
        PendingLink::parse("https://bank.example@evil.example/login")
            .unwrap()
            .target,
        "evil.example"
    );
    // So does every address a mail link writes to, past its `?` too.
    let target = |link| PendingLink::parse(link).unwrap().target;
    assert_eq!(
        target("mailto:support@bank.example?to=x@evil.example&subject=Hi"),
        "support@bank.example, x@evil.example"
    );
    assert_eq!(target("mailto:?to=x@evil.example"), "x@evil.example");
    assert_eq!(
        target("mailto:a@example.org,b@example.org?to=c@example.org,d@example.org,e@example.org"),
        "a@example.org, b@example.org, c@example.org and 2 more"
    );

    for rejected in [
        "javascript:alert(1)",
        "file:///etc/passwd",
        "data:text/html,x",
        "/relative",
        "not a link",
    ] {
        assert_eq!(PendingLink::parse(rejected), None, "{rejected}");
    }
}

#[test]
fn clicked_links_wait_for_confirmation_and_clear() {
    let mut app = loaded_demo_app();

    let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
    assert_eq!(
        app.pending_link().map(|link| link.target.as_str()),
        Some("example.com")
    );
    let _ = app.update(Message::DismissLink);
    assert!(app.pending_link().is_none());

    let _ = app.update(Message::LinkClicked("javascript:alert(1)".into()));
    assert!(app.pending_link().is_none());

    let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
    let _ = app.update(Message::OpenLink);
    assert!(app.pending_link().is_none());

    let _ = app.update(Message::LinkClicked("https://example.com/a".into()));
    let _ = app.update(Message::SelectFolder(sys(MailFolder::Sent)));
    assert!(app.pending_link().is_none());
}
