pub mod demo;
mod html;
mod model;
mod outgoing;
mod proton;
mod service;
mod threading;

use std::sync::Arc;

pub use model::{
    BlockKind, ConversationDetail, ConversationPage, ConversationSummary, CustomKind, Folder,
    MailAction, MailAddress, MailAttachment, MailFolder, MailMessage, MailboxCounts, MailboxError,
    MessageBody, RichBlock, RichBody, RichSpan, SummaryKind,
};
pub use outgoing::{BodyFormat, Outgoing, SendError, recipients};
#[cfg(test)]
pub use proton::Reply;
pub use proton::{ProtonMailService, ResumeOutcome, SignInEvent, SignInOutcome, SignInPrompt};
pub use service::{AuthError, LoginRequest};

/// The source of mailbox data for an open mailbox.
#[derive(Clone)]
pub enum MailBackend {
    Proton(Arc<ProtonMailService>),
    /// Local fictional mailbox for UI development; never contacts Proton.
    Demo(demo::DemoMailbox),
}

impl MailBackend {
    pub fn demo() -> Self {
        Self::Demo(demo::DemoMailbox::new())
    }

    pub fn page_size(&self) -> u32 {
        match self {
            Self::Proton(_) => proton::PAGE_SIZE,
            Self::Demo(_) => demo::PAGE_SIZE,
        }
    }

    pub async fn list_conversations(
        &self,
        folder: &Folder,
        page: u32,
        page_size: u32,
    ) -> Result<ConversationPage, MailboxError> {
        match self {
            Self::Proton(service) => service.list_conversations(folder, page, page_size).await,
            Self::Demo(service) => service.list_conversations(folder, page, page_size, demo::now()),
        }
    }

    /// The folders the account made. The demo mailbox has only Proton's own.
    /// Sends a message. The demo mailbox keeps it to itself; nothing about
    /// this path reaches the network without a Proton session.
    pub async fn send(&self, outgoing: &Outgoing) -> Result<(), SendError> {
        match self {
            Self::Proton(service) => service.send(outgoing).await,
            Self::Demo(service) => service.send(outgoing),
        }
    }

    pub async fn list_folders(&self) -> Result<Vec<Folder>, MailboxError> {
        match self {
            Self::Proton(service) => service.list_folders().await,
            Self::Demo(_) => Ok(Vec::new()),
        }
    }

    pub async fn conversation_counts(
        &self,
        folders: &[Folder],
    ) -> Result<MailboxCounts, MailboxError> {
        match self {
            Self::Proton(service) => service.conversation_counts(folders).await,
            Self::Demo(service) => Ok(service.counts()),
        }
    }

    /// Searches every folder for `query`. Unlike a folder listing, the answer
    /// is one complete batch: there are no further pages to ask for.
    pub async fn search(&self, query: &str, limit: u32) -> Result<ConversationPage, MailboxError> {
        match self {
            Self::Proton(service) => service.search(query, limit).await,
            Self::Demo(service) => service.search(query, limit, demo::now()),
        }
    }

    /// Fetches one attachment's contents. The demo mailbox carries no files.
    pub async fn download_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<(String, Vec<u8>), MailboxError> {
        match self {
            Self::Proton(service) => service.download_attachment(message_id, attachment_id).await,
            Self::Demo(_) => Err(MailboxError::Unavailable),
        }
    }

    /// Loads what a mailbox row opens in the reader.
    pub async fn conversation_detail(
        &self,
        kind: SummaryKind,
        id: &str,
    ) -> Result<ConversationDetail, MailboxError> {
        match self {
            Self::Proton(service) => match kind {
                SummaryKind::Conversation => service.conversation_detail(id).await,
                SummaryKind::Message => service.message_detail(id).await,
            },
            // Demo rows are always whole conversations.
            Self::Demo(service) => service.conversation_detail(id, demo::now()),
        }
    }
}
