pub mod demo;
mod html;
mod model;
mod proton;
mod service;

use std::sync::Arc;

pub use model::{
    ConversationDetail, ConversationPage, ConversationSummary, MailAddress, MailFolder,
    MailMessage, MailboxCounts, MailboxError, MessageBody,
};
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

    /// Loads the full conversation through the active backend.
    pub async fn conversation_detail(&self, id: &str) -> Result<ConversationDetail, MailboxError> {
        match self {
            Self::Proton(service) => service.conversation_detail(id).await,
            Self::Demo(service) => service.conversation_detail(id, demo::now()),
        }
    }
}
