pub mod demo;
mod model;
mod proton;
mod service;

use std::sync::Arc;

pub use model::{ConversationPage, ConversationSummary, MailFolder, MailboxCounts, MailboxError};
pub use proton::{ProtonMailService, ResumeOutcome, SignInOutcome};
pub use service::{AuthError, LoginRequest};

/// The source of mailbox data for an open mailbox.
#[derive(Clone)]
pub enum MailBackend {
    Proton(Arc<ProtonMailService>),
    /// Local fictional mailbox for UI development; never contacts Proton.
    Demo,
}

impl MailBackend {
    pub fn page_size(&self) -> u32 {
        match self {
            Self::Proton(_) => proton::PAGE_SIZE,
            Self::Demo => demo::PAGE_SIZE,
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
            Self::Demo => demo::list_conversations(folder, page, page_size, demo::now()),
        }
    }

    pub async fn conversation_counts(&self) -> Result<MailboxCounts, MailboxError> {
        match self {
            Self::Proton(service) => service.conversation_counts().await,
            Self::Demo => Ok(demo::counts()),
        }
    }
}
