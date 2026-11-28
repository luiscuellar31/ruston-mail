//! Helpers the desktop's tests share.

use crate::app::RequestId;
use crate::mail::{ConversationSummary, Folder, MailFolder, SummaryKind};

/// One of Proton's own folders, as a place to list from.
pub(crate) fn sys(folder: MailFolder) -> Folder {
    Folder::System(folder)
}

/// A row of a list, `id`, with nothing said of it: one message, read, with
/// no subject, sender, date, star, file or label. A test names what it needs
/// over it.
pub(crate) fn summary(id: &str) -> ConversationSummary {
    ConversationSummary {
        id: id.to_owned(),
        kind: SummaryKind::Conversation,
        subject: None,
        correspondents: None,
        participants: Vec::new(),
        preview: None,
        time: None,
        unread: false,
        starred: false,
        message_count: 1,
        has_attachments: false,
        labels: Vec::new(),
    }
}

/// The requests a mailbox opens with in the tests, at boot and in their
/// `open` helpers: its first page, then its counts.
pub(crate) const OPENING_PAGE: RequestId = 1;
pub(crate) const OPENING_COUNTS: RequestId = 2;
/// A request ID no test issues: a reply to it answers nothing pending.
pub(crate) const NEVER_ISSUED: RequestId = 99;
