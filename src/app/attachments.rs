//! Saving one of a message's attachments: where it goes, when asked, then
//! its download and write off the interface's thread, and what came of it.

use std::path::PathBuf;

use super::{App, Effects, Message, Notice, RequestId, SessionEpoch, UiEffect};
use crate::downloads::{self, SaveError};
use crate::mail::{AuthError, MailboxError};

/// Identifies one file operation across a destination dialog and download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentSaveRequest {
    pub(super) id: RequestId,
    pub(super) session: SessionEpoch,
    pub(super) message_id: String,
    pub(super) attachment_id: String,
}

pub(super) struct AttachmentSave {
    pub(super) request: AttachmentSaveRequest,
    pub(super) stage: SaveStage,
}

/// How far a save has come: the person is still choosing where the file
/// goes, or it is being fetched and written.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SaveStage {
    ChoosingDestination,
    Downloading,
}

/// Why an attachment that did not arrive was not saved.
pub(super) fn unsaved(error: MailboxError) -> SaveError {
    match error {
        MailboxError::AttachmentVerificationFailed => SaveError::VerificationFailed,
        MailboxError::SessionExpired => SaveError::SessionExpired,
        MailboxError::SessionUnavailable => SaveError::SessionUnavailable,
        MailboxError::Connection | MailboxError::Service | MailboxError::Unavailable => {
            SaveError::NotFetched
        }
    }
}

impl App {
    /// The attachment now being fetched, so its row can say so.
    pub fn saving_attachment(&self) -> Option<&str> {
        self.session
            .saving_attachment
            .as_ref()
            .map(|save| save.request.attachment_id.as_str())
    }

    /// The attachment whose last save failed, by ID, and why.
    pub fn failed_save(&self) -> Option<(&str, SaveError)> {
        self.session
            .failed_save
            .as_ref()
            .map(|(attachment_id, error)| (attachment_id.as_str(), *error))
    }

    /// Fetches an attachment and writes it off the UI thread.
    pub(super) fn save_attachment(
        &mut self,
        message_id: String,
        attachment_id: String,
        suggested_name: Option<String>,
    ) -> Effects {
        if !self.mailbox_actions_available() || self.session.saving_attachment.is_some() {
            return Effects::none();
        }
        let request = AttachmentSaveRequest {
            id: self.next_request(),
            session: self.session_epoch,
            message_id,
            attachment_id,
        };
        self.session.failed_save = None;
        self.session.saving_attachment = Some(AttachmentSave {
            request: request.clone(),
            stage: if suggested_name.is_some() {
                SaveStage::ChoosingDestination
            } else {
                SaveStage::Downloading
            },
        });
        if let Some(name) = suggested_name {
            Effects::ui(UiEffect::PickAttachmentDestination {
                request,
                suggested_name: ruston_core::mail::attachments::safe_attachment_name(&name),
            })
        } else {
            self.download_attachment(request, None)
        }
    }

    pub(super) fn attachment_destination_chosen(
        &mut self,
        request: AttachmentSaveRequest,
        destination: Option<PathBuf>,
    ) -> Effects {
        let Some(save) = self.current_save(&request, SaveStage::ChoosingDestination) else {
            return Effects::none();
        };
        let Some(path) = destination else {
            self.session.saving_attachment = None;
            return Effects::none();
        };
        save.stage = SaveStage::Downloading;
        self.download_attachment(request, Some(path))
    }

    /// The save under way, when it is `request`'s, belongs to this session
    /// and has come as far as `stage`.
    pub(super) fn current_save(
        &mut self,
        request: &AttachmentSaveRequest,
        stage: SaveStage,
    ) -> Option<&mut AttachmentSave> {
        if !self.is_current_session(request.session) {
            return None;
        }
        self.session
            .saving_attachment
            .as_mut()
            .filter(|save| save.request == *request && save.stage == stage)
    }

    pub(super) fn download_attachment(
        &self,
        request: AttachmentSaveRequest,
        destination: Option<PathBuf>,
    ) -> Effects {
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        let completion = request.clone();

        Effects::perform(
            async move {
                match backend
                    .download_attachment(&request.message_id, &request.attachment_id)
                    .await
                {
                    // File I/O is blocking, so keep it off both the interface
                    // thread and Tokio's asynchronous workers.
                    Ok((name, contents)) => {
                        tokio::task::spawn_blocking(move || match destination {
                            Some(path) => downloads::save_selected(&path, &contents),
                            None => downloads::save(&name, &contents),
                        })
                        .await
                        .unwrap_or(Err(SaveError::Failed))
                    }
                    Err(error) => Err(unsaved(error)),
                }
            },
            move |outcome| Message::AttachmentSaved(completion, outcome),
        )
    }

    /// Says where the attachment `request` saved went, or why it failed,
    /// while that save is still the one under way.
    pub(super) fn finish_attachment_save(
        &mut self,
        request: AttachmentSaveRequest,
        outcome: Result<PathBuf, SaveError>,
    ) {
        if self
            .current_save(&request, SaveStage::Downloading)
            .is_none()
        {
            return;
        }
        self.session.saving_attachment = None;
        // An expired session ends here as it does for any request.
        if matches!(outcome, Err(SaveError::SessionExpired)) {
            self.close_mailbox(Some(AuthError::SessionExpired));
            return;
        }
        // A save that landed after the session ended has nobody left to tell,
        // and its news must not surface in the next one.
        if self.mailbox.is_some() {
            match outcome {
                Ok(path) => self.session.notice = Some(Notice::Saved(path)),
                Err(error) => self.session.failed_save = Some((request.attachment_id, error)),
            }
        }
    }
}
