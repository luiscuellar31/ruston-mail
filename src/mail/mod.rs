pub mod demo;
mod model;
mod proton;
mod service;

use std::sync::Arc;

pub use model::{
    ConversationDetail, ConversationPage, ConversationSummary, MailAddress, MailFolder,
    MailMessage, MailboxCounts, MailboxError, MessageBody,
};
pub use proton::{ProtonMailService, ResumeOutcome, SignInOutcome};
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
        folder: MailFolder,
        page: u32,
        page_size: u32,
    ) -> Result<ConversationPage, MailboxError> {
        match self {
            Self::Proton(service) => service.list_conversations(folder, page, page_size).await,
            Self::Demo(service) => service.list_conversations(folder, page, page_size, demo::now()),
        }
    }

    pub async fn conversation_counts(&self) -> Result<MailboxCounts, MailboxError> {
        match self {
            Self::Proton(service) => service.conversation_counts().await,
            Self::Demo(service) => Ok(service.counts()),
        }
    }

    /// The full conversation for the reader, when this backend can provide it.
    /// Reading conversations from Proton is not implemented yet.
    pub fn conversation_detail(&self, id: &str) -> Option<ConversationDetail> {
        match self {
            Self::Proton(_) => None,
            Self::Demo(service) => service.conversation_detail(id, demo::now()),
        }
    }
}
