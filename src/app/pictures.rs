//! The pictures a message carries within it. One is fetched and decoded when
//! the reader first shows where it goes, off the interface's thread, and all
//! are forgotten with the conversation they belong to.

use std::collections::HashMap;
use std::sync::Arc;

use super::{App, Effects, Message, ReaderState, SessionEpoch};
use crate::pictures::{self, Picture};

/// The most decoded pictures of one conversation take, in bytes; those past
/// it are not shown.
const MAX_SHOWN_BYTES: usize = 96 * 1024 * 1024;

/// Where showing a picture stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineImage {
    /// On its way.
    Loading,
    Shown(Arc<Picture>),
    /// It could not be fetched, was no picture, or was too large.
    Unavailable,
}

impl App {
    /// The pictures of the open conversation asked for so far, by the ID of
    /// the attachment each is.
    pub fn inline_images(&self) -> &HashMap<String, InlineImage> {
        &self.inline_images
    }

    /// Fetches the picture `attachment_id` of `message_id`, once, when
    /// Settings show pictures and it is one of the open conversation's,
    /// small enough to read.
    pub(super) fn load_inline_image(
        &mut self,
        message_id: String,
        attachment_id: String,
    ) -> Effects {
        let asked = self.inline_images.contains_key(&attachment_id);
        let known = self
            .mailbox
            .as_ref()
            .and_then(|mailbox| match mailbox.reader_state() {
                ReaderState::Loaded(reader) => Some(reader),
                _ => None,
            })
            .is_some_and(|reader| {
                reader.detail().messages.iter().any(|message| {
                    message.id == message_id
                        && message.attachments.iter().any(|file| {
                            file.id == attachment_id && file.size <= pictures::MAX_BYTES
                        })
                })
            });
        if asked || !known || !self.settings.show_inline_images {
            return Effects::none();
        }
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        self.inline_images
            .insert(attachment_id.clone(), InlineImage::Loading);
        let epoch = self.session_epoch;
        let id = attachment_id.clone();
        Effects::perform(
            async move {
                let (_, bytes) = backend
                    .download_attachment(&message_id, &attachment_id)
                    .await
                    .ok()?;
                // Decoding takes a while and what it reads came from a
                // stranger: neither belongs on the interface's thread.
                tokio::task::spawn_blocking(move || pictures::decode(&bytes))
                    .await
                    .ok()
                    .flatten()
                    .map(Arc::new)
            },
            move |picture| Message::InlineImageLoaded(epoch, id, picture),
        )
    }

    /// Keeps a picture that arrived, if its conversation is still the open
    /// one and there is room for it.
    pub(super) fn finish_inline_image(
        &mut self,
        epoch: SessionEpoch,
        attachment_id: &str,
        picture: Option<Arc<Picture>>,
    ) {
        if !self.is_current_session(epoch)
            || self.inline_images.get(attachment_id) != Some(&InlineImage::Loading)
        {
            return;
        }
        let shown: usize = self
            .inline_images
            .values()
            .map(|image| match image {
                InlineImage::Shown(picture) => picture.rgba.len(),
                _ => 0,
            })
            .sum();
        let image = match picture {
            Some(picture) if shown + picture.rgba.len() <= MAX_SHOWN_BYTES => {
                InlineImage::Shown(picture)
            }
            _ => InlineImage::Unavailable,
        };
        self.inline_images.insert(attachment_id.to_owned(), image);
    }
}
