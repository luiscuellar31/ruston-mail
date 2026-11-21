//! The pictures a message carries within it. One is fetched and decoded when
//! the reader first shows where it goes, off the interface's thread, and all
//! are forgotten with the conversation they belong to.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Semaphore;

use super::{App, Effects, Message, ReaderState, SessionEpoch};
use crate::pictures::{self, Picture};

/// The most decoded pictures of one conversation take, in bytes; those past
/// it are not shown.
const MAX_SHOWN_BYTES: usize = 96 * 1024 * 1024;

/// How many pictures are fetched and decoded at once. A message asks for all
/// of its pictures as it opens, and decoding one may take hundreds of
/// megabytes for a moment, whatever size it is shown at.
const AT_ONCE: usize = 2;

/// The turns the pictures of one conversation take: [`AT_ONCE`] of them on
/// their way, and none started once the conversation has all it may show or
/// is no longer the open one.
#[derive(Clone)]
pub(super) struct Turns {
    waiting: Arc<Semaphore>,
    closed: Arc<AtomicBool>,
}

impl Default for Turns {
    fn default() -> Self {
        Self {
            waiting: Arc::new(Semaphore::new(AT_ONCE)),
            closed: Arc::default(),
        }
    }
}

impl Turns {
    /// Lets no picture still waiting for its turn start.
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }

    fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// What `load` gives once its turn comes, or nothing if the turns closed
    /// before it did.
    async fn take<T>(&self, load: impl Future<Output = Option<T>>) -> Option<T> {
        let _turn = self.waiting.acquire().await.ok()?;
        if self.closed() {
            return None;
        }
        load.await
    }
}

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
        // With no room left, one more is not fetched only to be dropped.
        if self.picture_turns.closed() {
            self.inline_images
                .insert(attachment_id, InlineImage::Unavailable);
            return Effects::none();
        }
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        self.inline_images
            .insert(attachment_id.clone(), InlineImage::Loading);
        let epoch = self.session_epoch;
        let id = attachment_id.clone();
        let turns = self.picture_turns.clone();
        Effects::perform(
            async move {
                turns
                    .take(async {
                        let (_, bytes) = backend
                            .download_attachment(&message_id, &attachment_id)
                            .await
                            .ok()?;
                        // Decoding takes a while and what it reads came from
                        // a stranger: neither belongs on the interface's
                        // thread.
                        tokio::task::spawn_blocking(move || pictures::decode(&bytes))
                            .await
                            .ok()
                            .flatten()
                            .map(Arc::new)
                    })
                    .await
            },
            move |picture| Message::InlineImageLoaded(epoch, id, picture),
        )
    }

    /// Forgets the pictures of the conversation being left, and lets none of
    /// them still waiting for its turn start.
    pub(super) fn forget_inline_images(&mut self) {
        self.picture_turns.close();
        self.picture_turns = Turns::default();
        self.inline_images.clear();
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
            // The first that does not fit is the last one fetched.
            Some(_) => {
                self.picture_turns.close();
                InlineImage::Unavailable
            }
            None => InlineImage::Unavailable,
        };
        self.inline_images.insert(attachment_id.to_owned(), image);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn pictures_take_turns_and_none_starts_once_the_turns_close() {
        let turns = Turns::default();
        let (running, most, started) = (
            AtomicUsize::new(0),
            AtomicUsize::new(0),
            AtomicUsize::new(0),
        );
        let load = |close: bool| {
            let (turns, running, most, started) = (&turns, &running, &most, &started);
            async move {
                turns
                    .take(async {
                        started.fetch_add(1, Ordering::SeqCst);
                        most.fetch_max(
                            running.fetch_add(1, Ordering::SeqCst) + 1,
                            Ordering::SeqCst,
                        );
                        tokio::task::yield_now().await;
                        running.fetch_sub(1, Ordering::SeqCst);
                        if close {
                            turns.close();
                        }
                        Some(())
                    })
                    .await
            }
        };
        // Eight ask at once, as the pictures of a message do when it opens.
        let first: Vec<_> = futures::future::join_all((0..8).map(|_| load(false))).await;
        assert_eq!(first, [Some(()); 8]);
        assert_eq!(most.load(Ordering::SeqCst), AT_ONCE);

        // The one that fills the room closes the turns: those that had not
        // started never do.
        let after: Vec<_> = futures::future::join_all((0..8).map(|index| load(index == 0))).await;
        assert_eq!(after.iter().flatten().count(), AT_ONCE);
        assert_eq!(started.load(Ordering::SeqCst), 8 + AT_ONCE);
    }
}
