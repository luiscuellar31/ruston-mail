//! What the terminal interface reads from a Proton account or from the
//! offline demo, through the same calls, and the changes it makes to a
//! conversation; opened once for the whole session. It returns core's types;
//! how they look is the interface's business.

use crate::demo::{self, DemoSession};
use ruston_core::model::ConversationLabel;
use ruston_core::model::enums::{label_ids, label_type};
use ruston_core::{
    Client, Conversation, ConversationMessage, Error, Label, LabelCount, MessageMetadata, Result,
    SearchOpts,
};
use std::sync::Arc;

/// How many conversations a page of the list holds.
pub(crate) const PAGE_SIZE: u32 = 50;

/// How many messages a search of the local index finds at most.
const LOCAL_LIMIT: u32 = 200;

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

    /// Page `page` of the conversations that `words` find in all mail,
    /// newest first, with how many there are in all; the demo looks in
    /// their subjects, as its `conversations search` does.
    pub(crate) async fn search(&self, words: &str, page: u32) -> Result<(u32, Vec<Conversation>)> {
        match self {
            Self::Proton(client) => {
                let opts = SearchOpts {
                    keyword: Some(words.to_owned()),
                    ..SearchOpts::default()
                };
                client
                    .search_conversations_page(&opts, page, PAGE_SIZE)
                    .await
            }
            Self::Demo(session) => Ok(session.search(words, page, PAGE_SIZE)),
        }
    }

    /// The conversations of the messages that `words` find in the local
    /// index `ruston-cli index` builds, best matches first, with how many
    /// there are: a search that needs no connection. The demo looks in its
    /// messages, as its `search` does.
    pub(crate) async fn search_local(&self, words: &str) -> Result<(u32, Vec<Conversation>)> {
        let messages = match self {
            Self::Proton(client) => {
                // SQLite, which may first bring an older index up to date.
                let (client, words) = (Arc::clone(client), words.to_owned());
                tokio::task::spawn_blocking(move || client.search_local(&words, LOCAL_LIMIT))
                    .await
                    .map_err(|error| Error::Other(format!("local search stopped: {error}")))??
            }
            Self::Demo(_) => demo::search_local(words, LOCAL_LIMIT),
        };
        let conversations = conversations_of(messages);
        Ok((conversations.len() as u32, conversations))
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

    /// Waits until no token refresh is under way, so quitting cuts none
    /// short and loses the session; the demo has none.
    pub(crate) async fn settle(&self) {
        if let Self::Proton(client) = self {
            client.settle().await;
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

/// The conversations `messages` belong to, in the order the first message of
/// each comes, made of what those messages say, as the list shows them: a
/// search of the local index finds messages, and has nothing else offline.
fn conversations_of(messages: Vec<MessageMetadata>) -> Vec<Conversation> {
    let mut conversations: Vec<Conversation> = Vec::new();
    for message in messages {
        if message.conversation_id.is_empty() {
            continue;
        }
        let at = conversations
            .iter()
            .position(|conversation| conversation.id == message.conversation_id)
            .unwrap_or_else(|| {
                conversations.push(Conversation {
                    id: message.conversation_id.clone(),
                    subject: message.subject.clone(),
                    ..Conversation::default()
                });
                conversations.len() - 1
            });
        let conversation = &mut conversations[at];
        let unread = i64::from(message.unread > 0);
        conversation.num_messages += 1;
        conversation.num_unread += unread;
        conversation.num_attachments += message.num_attachments;
        conversation.time = conversation.time.max(message.time);
        if !conversation
            .senders
            .iter()
            .any(|sender| sender.address == message.sender.address)
        {
            conversation.senders.push(message.sender.clone());
        }
        for id in &message.label_ids {
            match conversation.labels.iter_mut().find(|label| &label.id == id) {
                Some(label) => {
                    label.context_num_messages = Some(label.context_num_messages.unwrap_or(0) + 1);
                    label.context_num_unread = Some(label.context_num_unread.unwrap_or(0) + unread);
                }
                None => conversation.labels.push(ConversationLabel {
                    id: id.clone(),
                    context_num_messages: Some(1),
                    context_num_unread: Some(unread),
                    context_time: Some(message.time),
                }),
            }
        }
    }
    conversations
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

    fn message(id: &str, conversation: &str, sender: &str, unread: i64) -> MessageMetadata {
        MessageMetadata {
            id: id.into(),
            conversation_id: conversation.into(),
            subject: format!("About {conversation}"),
            sender: ruston_core::Recipient::new(sender),
            unread,
            time: 1_700_000_000 + id.len() as i64,
            label_ids: vec![label_ids::INBOX.into()],
            ..MessageMetadata::default()
        }
    }

    #[test]
    fn messages_found_offline_list_as_their_conversations_in_the_order_found() {
        let messages = vec![
            message("m1", "c-b", "alex@example.test", 1),
            message("m2", "c-a", "sam@example.test", 0),
            message("m-three", "c-b", "sam@example.test", 0),
            message("m4", "", "lost@example.test", 1),
        ];
        let conversations = conversations_of(messages);
        let ids: Vec<&str> = conversations.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["c-b", "c-a"]);
        let first = &conversations[0];
        assert_eq!((first.num_messages, first.num_unread), (2, 1));
        assert_eq!(first.time, 1_700_000_007);
        assert_eq!(first.senders.len(), 2);
        assert_eq!(first.labels[0].context_num_unread, Some(1));
        assert_eq!(first.subject, "About c-b");
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
        let (found, matching) = mailbox.search("AGENDA", 0).await.unwrap();
        assert_eq!(found, 1);
        assert_eq!(matching[0].subject, "Offsite planning and agenda");
        // The demo's local search looks in bodies too.
        let (found, matching) = mailbox.search_local("glossary").await.unwrap();
        assert_eq!(found, 1);
        assert_eq!(matching[0].id, "conv-demo-02");

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
