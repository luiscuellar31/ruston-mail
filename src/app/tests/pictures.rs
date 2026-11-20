//! The pictures a message carries: fetched once, for the open conversation
//! alone, and forgotten with it.

use super::*;
use crate::pictures::Picture;

/// Opens the first demo conversation with a picture among its first
/// message's parts, and says which message that is.
fn open_with_a_picture(app: &mut App, size: u64) -> String {
    let _ = app.update(Message::SelectConversation("demo-0".into()));
    let request = pending_reader_request(app);
    let mut detail = demo_service(app)
        .conversation_detail(&request.conversation_id, NOW)
        .unwrap();
    detail.messages[0].attachments = vec![crate::mail::MailAttachment {
        id: "logo".into(),
        name: "logo.png".into(),
        size,
        inline: true,
        content_id: Some("logo@example.com".into()),
    }];
    let message = detail.messages[0].id.clone();
    let _ = app.update(Message::ConversationLoaded(request, Ok(detail)));
    message
}

fn picture() -> Arc<Picture> {
    Arc::new(Picture {
        size: [1, 1],
        rgba: vec![0; 4],
    })
}

#[test]
fn a_picture_is_fetched_once_for_the_open_conversation_and_forgotten_with_it() {
    let mut app = loaded_demo_app();
    let message = open_with_a_picture(&mut app, 2_048);
    let ask = |app: &mut App, attachment: &str| {
        app.update(Message::LoadInlineImage(message.clone(), attachment.into()))
            .units()
    };

    // Not a part of this message: nothing is fetched on the reader's word.
    assert_eq!(ask(&mut app, "other"), 0);
    assert_eq!(ask(&mut app, "logo"), 1);
    assert_eq!(app.inline_images().get("logo"), Some(&InlineImage::Loading));
    assert_eq!(ask(&mut app, "logo"), 0, "once is enough");

    // One from a session since ended changes nothing.
    let epoch = app.session_epoch;
    let _ = app.update(Message::InlineImageLoaded(
        epoch + 1,
        "logo".into(),
        Some(picture()),
    ));
    assert_eq!(app.inline_images().get("logo"), Some(&InlineImage::Loading));
    let _ = app.update(Message::InlineImageLoaded(
        epoch,
        "logo".into(),
        Some(picture()),
    ));
    assert_eq!(
        app.inline_images().get("logo"),
        Some(&InlineImage::Shown(picture()))
    );

    // Another conversation has none of these pictures, and one that arrives
    // late for the one left is not kept.
    let _ = app.update(Message::SelectConversation("demo-1".into()));
    assert!(app.inline_images().is_empty());
    let _ = app.update(Message::InlineImageLoaded(
        epoch,
        "logo".into(),
        Some(picture()),
    ));
    assert!(app.inline_images().is_empty());
}

#[test]
fn a_picture_that_cannot_be_shown_is_not_asked_for_again() {
    let mut app = loaded_demo_app();
    let message = open_with_a_picture(&mut app, 2_048);
    let _ = app.update(Message::LoadInlineImage(message.clone(), "logo".into()));
    let epoch = app.session_epoch;
    // It could not be fetched, or was no picture.
    let _ = app.update(Message::InlineImageLoaded(epoch, "logo".into(), None));
    assert_eq!(
        app.inline_images().get("logo"),
        Some(&InlineImage::Unavailable)
    );
    assert_eq!(
        app.update(Message::LoadInlineImage(message, "logo".into()))
            .units(),
        0
    );
}

#[test]
fn no_picture_is_fetched_when_settings_show_none_or_it_is_too_large() {
    let mut app = loaded_demo_app();
    let message = open_with_a_picture(&mut app, crate::pictures::MAX_BYTES + 1);
    assert_eq!(
        app.update(Message::LoadInlineImage(message, "logo".into()))
            .units(),
        0
    );

    let mut app = loaded_demo_app();
    let message = open_with_a_picture(&mut app, 2_048);
    let _ = app.update(Message::SetShowInlineImages(false));
    assert_eq!(
        app.update(Message::LoadInlineImage(message, "logo".into()))
            .units(),
        0
    );
    assert!(app.inline_images().is_empty());
}
