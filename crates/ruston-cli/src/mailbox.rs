//! What the terminal interface reads from a Proton account or from the
//! offline demo, through the same calls, and the changes it makes to a
//! conversation; opened once for the whole session. It returns core's types;
//! how they look is the interface's business.

use crate::demo::{self, DemoSession};
use ruston_core::model::enums::{label_ids, label_type};
use ruston_core::{Client, Conversation, ConversationMessage, Label, LabelCount, Result};
use std::sync::Arc;

/// How many conversations a page of the list holds.
pub(crate) const PAGE_SIZE: u32 = 50;

/// The system folders the desktop shows, in its order.
const SYSTEM_FOLDERS: [(&str, &str); 7] = [
    (label_ids::INBOX, "Inbox"),
    (label_ids::DRAFTS, "Drafts"),
    (label_ids::SENT, "Sent"),
    (label_ids::STARRED, "Starred"),
    (label_ids::ARCHIVE, "Archive"),
    (label_ids::SPAM, "Spam"),
    (label_ids::TRASH, "Trash"),
];

#[derive(Clone)]
pub(crate) enum Mailbox {
    /// A signed-in account, shared by every task the interface runs.
    Proton(Arc<Client>),
    /// The offline demo's fictional mail, as this session has changed it.
    Demo(Arc<DemoSession>),
}

/// A change the interface makes to a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    /// Star it, or take its star away.
    Star(bool),
    /// Mark it read, or unread in the place it is listed in.
    Read(bool),
    /// Move it to the folder with this label ID, such as Archive or Trash.
    Move(String),
}

/// A place to list mail from: a system folder, or one of the account's own
/// folders or labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Place {
    /// The label ID that lists it.
    pub(crate) id: String,
    pub(crate) name: String,
    /// How many folders of the account's own it sits under.
    pub(crate) depth: usize,
    pub(crate) kind: PlaceKind,
    /// How many of its conversations are unread.
    pub(crate) unread: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaceKind {
    System,
    Folder,
    Label,
}

impl Mailbox {
    /// The offline demo, as its fixtures start.
    pub(crate) fn demo() -> Self {
        Self::Demo(Arc::new(DemoSession::new()))
    }

    /// Whose mail it is, for the status bar.
    pub(crate) fn account(&self) -> String {
        match self {
            Self::Proton(client) => client.primary_email().unwrap_or("(unknown)").to_owned(),
            Self::Demo(_) => format!("{} (demo)", demo::DEMO_EMAIL),
        }
    }

    /// The system folders, then the account's folders and labels, each with
    /// how many of its conversations are unread.
    pub(crate) async fn places(&self) -> Result<Vec<Place>> {
        let (folders, labels, counts) = match self {
            Self::Proton(client) => tokio::try_join!(
                client.list_folders(),
                client.list_labels(),
                client.conversation_counts()
            )?,
            Self::Demo(session) => {
                let (folders, labels) = demo::demo_labels()
                    .into_iter()
                    .partition(|label| label.label_type == label_type::MESSAGE_FOLDER);
                (folders, labels, session.counts())
            }
        };
        Ok(places(folders, labels, &counts))
    }

    /// Page `page` of the conversations in the place with label ID `id`,
    /// newest first, with how many there are in all.
    pub(crate) async fn conversations(
        &self,
        id: &str,
        page: u32,
    ) -> Result<(u32, Vec<Conversation>)> {
        match self {
            Self::Proton(client) => client.list_conversations(id, page, PAGE_SIZE, false).await,
            Self::Demo(session) => Ok(session.conversations(id, page, PAGE_SIZE)),
        }
    }

    /// The conversation `id` with its messages, oldest first, each opened
    /// or said to be unreadable. Reading it does not mark it read.
    pub(crate) async fn conversation(
        &self,
        id: &str,
    ) -> Result<(Conversation, Vec<ConversationMessage>)> {
        match self {
            Self::Proton(client) => client.read_conversation_each(id).await,
            Self::Demo(_) => demo::conversation(id),
        }
    }

    /// Makes `change` to the conversation `id`, listed in the place
    /// `place`. The demo keeps it until the interface closes.
    pub(crate) async fn change(&self, id: &str, place: &str, change: &Change) -> Result<()> {
        let ids = [id.to_owned()];
        match self {
            Self::Proton(client) => match change {
                Change::Star(starred) => client.star_conversations(&ids, *starred).await,
                Change::Read(read) => client.mark_conversations_read(&ids, *read, place).await,
                Change::Move(folder) => client.move_conversations(&ids, folder).await,
            },
            Self::Demo(session) => {
                session.change(id, place, change);
                Ok(())
            }
        }
    }
}

/// The places to list: the system folders, then the account's folders in
/// the order of their paths, so each sits under its parent, then its labels
/// as Proton orders them.
fn places(mut folders: Vec<Label>, labels: Vec<Label>, counts: &[LabelCount]) -> Vec<Place> {
    let unread = |id: &str| {
        counts
            .iter()
            .find(|count| count.label_id == id)
            .map_or(0, |count| count.unread)
    };
    folders.sort_by(|a, b| a.path.cmp(&b.path));
    let system = SYSTEM_FOLDERS.iter().map(|(id, name)| Place {
        id: (*id).to_owned(),
        name: (*name).to_owned(),
        depth: 0,
        kind: PlaceKind::System,
        unread: unread(id),
    });
    let folders = folders.into_iter().map(|folder| Place {
        unread: unread(&folder.id),
        depth: folder.path.matches('/').count(),
        id: folder.id,
        name: folder.name,
        kind: PlaceKind::Folder,
    });
    let labels = labels.into_iter().map(|label| Place {
        unread: unread(&label.id),
        depth: 0,
        id: label.id,
        name: label.name,
        kind: PlaceKind::Label,
    });
    system.chain(folders).chain(labels).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(id: &str, path: &str) -> Label {
        Label {
            id: id.into(),
            name: path.rsplit('/').next().unwrap().into(),
            path: path.into(),
            ..Label::default()
        }
    }

    #[test]
    fn system_folders_come_first_then_folders_under_their_parents_then_labels() {
        let folders = vec![
            label("f-work", "Work"),
            label("f-taxes-2024", "Taxes/2024"),
            label("f-taxes", "Taxes"),
        ];
        let labels = vec![label("l-later", "Later"), label("l-family", "Family")];
        let counts = [
            LabelCount {
                label_id: label_ids::INBOX.into(),
                total: 9,
                unread: 3,
            },
            LabelCount {
                label_id: "f-taxes-2024".into(),
                total: 2,
                unread: 1,
            },
        ];

        let places = places(folders, labels, &counts);
        let names: Vec<(&str, usize, i64)> = places
            .iter()
            .map(|place| (place.name.as_str(), place.depth, place.unread))
            .collect();
        assert_eq!(
            names,
            [
                ("Inbox", 0, 3),
                ("Drafts", 0, 0),
                ("Sent", 0, 0),
                ("Starred", 0, 0),
                ("Archive", 0, 0),
                ("Spam", 0, 0),
                ("Trash", 0, 0),
                ("Taxes", 0, 0),
                ("2024", 1, 1),
                ("Work", 0, 0),
                ("Later", 0, 0),
                ("Family", 0, 0),
            ]
        );
        assert_eq!(places[7].kind, PlaceKind::Folder);
        assert_eq!(places[10].kind, PlaceKind::Label);
    }

    #[tokio::test]
    async fn the_demo_lists_its_places_and_conversations_as_its_commands_do() {
        let mailbox = Mailbox::demo();
        let places = mailbox.places().await.unwrap();
        let inbox = &places[0];
        assert_eq!(inbox.id, label_ids::INBOX);
        let counted = demo::demo_counts(true)
            .into_iter()
            .find(|count| count.label_id == label_ids::INBOX)
            .unwrap();
        assert_eq!(inbox.unread, counted.unread);
        assert!(
            places
                .iter()
                .any(|place| place.name == "Projects" && place.kind == PlaceKind::Folder)
        );
        assert!(
            places
                .iter()
                .any(|place| place.name == "Work" && place.kind == PlaceKind::Label)
        );

        let (total, page) = mailbox.conversations(label_ids::INBOX, 0).await.unwrap();
        assert!(
            total > 0 && page.len() as u32 == total,
            "{total} {}",
            page.len()
        );
        assert!(page.windows(2).all(|pair| pair[0].time >= pair[1].time));
        let (_, past_the_end) = mailbox.conversations(label_ids::INBOX, 9).await.unwrap();
        assert!(past_the_end.is_empty());

        let (conversation, messages) = mailbox.conversation(&page[0].id).await.unwrap();
        assert_eq!(conversation.id, page[0].id);
        assert!(!messages.is_empty());
        assert!(
            messages
                .iter()
                .all(|message| matches!(message, ConversationMessage::Opened(_)))
        );
        assert!(mailbox.conversation("no-such-conversation").await.is_err());
        mailbox
            .change(&page[0].id, label_ids::INBOX, &Change::Read(true))
            .await
            .unwrap();
    }
}
