//! What the terminal interface reads from a Proton account or from the
//! offline demo, through the same calls, and the changes it makes to a
//! conversation; opened once for the whole session. It returns core's types;
//! how they look is the interface's business.

use crate::demo::{self, DemoSession};
use futures::StreamExt;
use ruston_core::mail::attachments::save_under_free_name;
use ruston_core::model::conversations_of;
use ruston_core::model::enums::{label_ids, label_type};
use ruston_core::{
    AddressInfo, Client, Conversation, ConversationMessage, Error, Label, LabelCount, Quote,
    RecipientProtection, Result, SearchOpts, SendOptions,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How many conversations a page of the list holds.
pub(crate) const PAGE_SIZE: u32 = 50;

/// How many messages a search of the local index finds at most.
const LOCAL_LIMIT: u32 = 200;

/// How many recipients' keys are looked up at once, as the desktop does.
const PROTECTION_LOOKUPS: usize = 4;

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
    /// Put the label with this ID on it.
    Label(String),
    /// Take the label with this ID off it.
    Unlabel(String),
    /// Delete it forever from the place it is listed in, Trash or Spam.
    Delete,
}

/// What a message answers, if anything. Core writes an answer's subject and
/// quotes the original under its text, unless told to leave it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OutgoingKind {
    New,
    /// A reply to the message `message_id`, with `everyone` to all it
    /// reached as well as its sender.
    Reply {
        message_id: String,
        everyone: bool,
        quote: Quote,
    },
    /// The message `message_id` passed on, with its files but those whose
    /// IDs are in `leave_out`.
    Forward {
        message_id: String,
        quote: Quote,
        leave_out: Vec<String>,
    },
}

impl OutgoingKind {
    /// Whether the original goes under the text; a new message answers
    /// nothing.
    pub(crate) fn quote(&self) -> Quote {
        match self {
            Self::New => Quote::default(),
            Self::Reply { quote, .. } | Self::Forward { quote, .. } => *quote,
        }
    }
}

/// A message ready to leave, as plain text: from the account's primary
/// address, or for an answer, from the address the original reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outgoing {
    pub(crate) kind: OutgoingKind,
    pub(crate) to: Vec<String>,
    pub(crate) cc: Vec<String>,
    pub(crate) bcc: Vec<String>,
    pub(crate) subject: String,
    pub(crate) body: String,
    /// Files of this computer it carries.
    pub(crate) attachments: Vec<PathBuf>,
}

impl Outgoing {
    /// Everyone it goes to, each once whatever the case of their address,
    /// To before Cc before Bcc.
    pub(crate) fn recipients(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.to
            .iter()
            .chain(&self.cc)
            .chain(&self.bcc)
            .filter(|address| seen.insert(address.to_ascii_lowercase()))
            .cloned()
            .collect()
    }
}

/// Why a message did not go, or may not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SendFailure {
    /// Proton may have taken it, but that could not be confirmed.
    Unconfirmed,
    /// It was not sent, for this reason.
    Failed(String),
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
            Self::Proton(_) => self.sender(),
            Self::Demo(_) => format!("{} (demo)", demo::DEMO_EMAIL),
        }
    }

    /// The address a new message goes from: the account's primary one,
    /// which core sends from when told no other.
    pub(crate) fn sender(&self) -> String {
        match self {
            Self::Proton(client) => client.primary_email().unwrap_or("(unknown)").to_owned(),
            Self::Demo(_) => demo::DEMO_EMAIL.to_owned(),
        }
    }

    /// Every address of the account: a reply leaves them out of whom it
    /// goes to, and goes from the one the original reached.
    pub(crate) fn own_addresses(&self) -> Vec<AddressInfo> {
        match self {
            Self::Proton(client) => client.addresses(),
            Self::Demo(_) => demo::demo_addresses(),
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

    /// Page `page` of the conversations Proton's search finds for `opts`, its
    /// folder named by label ID, newest first, with how many there are in
    /// all; the demo finds them as its `conversations search` does.
    pub(crate) async fn search(
        &self,
        opts: &SearchOpts,
        page: u32,
    ) -> Result<(u32, Vec<Conversation>)> {
        match self {
            Self::Proton(client) => {
                client
                    .search_conversations_page(opts, page, PAGE_SIZE)
                    .await
            }
            Self::Demo(session) => session.search(opts, page, PAGE_SIZE),
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

    /// Downloads the attachment `attachment_id` of the message `message_id`,
    /// which must verify when it is signed, and saves it in `folder` under
    /// its name made safe, never over a file or through a link already
    /// there; returns where it landed. The demo saves a stand-in, as its
    /// `attachments download` does.
    pub(crate) async fn save_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
        folder: &Path,
    ) -> Result<PathBuf> {
        let (name, contents) = match self {
            Self::Proton(client) => {
                client
                    .download_attachment(message_id, attachment_id)
                    .await?
            }
            Self::Demo(session) => session.download_attachment(message_id, attachment_id)?,
        };
        let folder = folder.to_owned();
        tokio::task::spawn_blocking(move || save_under_free_name(&folder, &name, &contents))
            .await
            .map_err(|error| Error::Other(format!("saving stopped: {error}")))?
            .map_err(Error::from)
    }

    /// The conversation `id` with its messages, oldest first, each opened
    /// or said to be unreadable. Reading it does not mark it read.
    pub(crate) async fn conversation(
        &self,
        id: &str,
    ) -> Result<(Conversation, Vec<ConversationMessage>)> {
        match self {
            Self::Proton(client) => client.read_conversation_each(id).await,
            Self::Demo(session) => session.conversation(id),
        }
    }

    /// How Proton would protect a message to each of `addresses`, in their
    /// order: the key lookup sending makes, a few at a time. The demo
    /// encrypts end to end for Proton's own domains alone.
    pub(crate) async fn protection(
        &self,
        addresses: &[String],
    ) -> Vec<(String, std::result::Result<RecipientProtection, String>)> {
        match self {
            Self::Proton(client) => {
                // Built before the await, so the future is plainly `Send`.
                let lookups: Vec<_> = addresses
                    .iter()
                    .map(|address| async move {
                        let protection = client
                            .recipient_protection(address)
                            .await
                            .map_err(|error| error.to_string());
                        (address.clone(), protection)
                    })
                    .collect();
                futures::stream::iter(lookups)
                    .buffered(PROTECTION_LOOKUPS)
                    .collect()
                    .await
            }
            Self::Demo(_) => addresses
                .iter()
                .map(|address| (address.clone(), Ok(demo::protection(address))))
                .collect(),
        }
    }

    /// Sends `outgoing` as plain text: a new message from the account's
    /// primary address, an answer through core's `reply` or `forward`. The
    /// demo keeps it until the interface closes.
    pub(crate) async fn send(&self, outgoing: &Outgoing) -> std::result::Result<(), SendFailure> {
        match self {
            Self::Proton(client) => {
                let options = SendOptions {
                    to: outgoing.to.clone(),
                    cc: outgoing.cc.clone(),
                    bcc: outgoing.bcc.clone(),
                    subject: outgoing.subject.clone(),
                    body: outgoing.body.clone(),
                    attachments: outgoing.attachments.clone(),
                    quote: outgoing.kind.quote(),
                    leave_out: match &outgoing.kind {
                        OutgoingKind::Forward { leave_out, .. } => leave_out.clone(),
                        OutgoingKind::New | OutgoingKind::Reply { .. } => Vec::new(),
                    },
                    ..SendOptions::default()
                };
                let sent = match &outgoing.kind {
                    OutgoingKind::New => client.send(&options).await,
                    OutgoingKind::Reply {
                        message_id,
                        everyone,
                        ..
                    } => client.reply(message_id, *everyone, &options).await,
                    OutgoingKind::Forward { message_id, .. } => {
                        client.forward(message_id, &options).await
                    }
                };
                match sent {
                    Ok(_) => Ok(()),
                    Err(Error::SendUnconfirmed { .. }) => Err(SendFailure::Unconfirmed),
                    Err(error) => Err(SendFailure::Failed(error.to_string())),
                }
            }
            Self::Demo(session) => {
                session.send(outgoing);
                Ok(())
            }
        }
    }

    /// Waits until no token refresh is under way, so quitting cuts none
    /// short and loses the session; the demo has none.
    pub(crate) async fn settle(&self) {
        if let Self::Proton(client) = self {
            client.settle().await;
        }
    }

    /// Makes `change` to the conversations `ids`, listed in the place
    /// `place`, all in one request; deleting forever only from Trash or
    /// Spam. The demo keeps it until the interface closes.
    pub(crate) async fn change(&self, ids: &[String], place: &str, change: &Change) -> Result<()> {
        if *change == Change::Delete {
            only_thrown_away(place)?;
        }
        match self {
            Self::Proton(client) => match change {
                Change::Star(starred) => client.star_conversations(ids, *starred).await,
                Change::Read(read) => client.mark_conversations_read(ids, *read, place).await,
                Change::Move(folder) => client.move_conversations(ids, folder).await,
                Change::Label(label) => client.label_conversations(ids, label).await,
                Change::Unlabel(label) => client.unlabel_conversations(ids, label).await,
                Change::Delete => client.delete_conversations(ids, place).await,
            },
            Self::Demo(session) => {
                for id in ids {
                    session.change(id, place, change);
                }
                Ok(())
            }
        }
    }

    /// Deletes forever everything in the place `place`, which must be Trash
    /// or Spam. The demo keeps it until the interface closes.
    pub(crate) async fn empty(&self, place: &str) -> Result<()> {
        only_thrown_away(place)?;
        match self {
            Self::Proton(client) => client.empty_folder(place).await,
            Self::Demo(session) => {
                session.empty(place);
                Ok(())
            }
        }
    }
}

/// Whether mail in the place with label ID `id` can be deleted forever: only
/// in Trash and Spam, which hold mail already thrown away, as `messages
/// empty` empties only them.
pub(crate) fn deletes_forever(id: &str) -> bool {
    id == label_ids::TRASH || id == label_ids::SPAM
}

/// Refuses to delete mail forever from anywhere but Trash and Spam, which
/// the interface never asks for.
fn only_thrown_away(place: &str) -> Result<()> {
    if deletes_forever(place) {
        Ok(())
    } else {
        Err(Error::Other(
            "only mail in Trash or Spam is deleted forever".into(),
        ))
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
    async fn an_attachment_saves_under_a_free_name_never_over_a_file() {
        let folder = std::env::temp_dir().join(format!(
            "ruston-tui-save-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let mailbox = Mailbox::demo();
        let first = mailbox
            .save_attachment("msg-demo-03", "att-demo-01", &folder)
            .await
            .unwrap();
        assert_eq!(first, folder.join("translation_proposal.pdf"));
        let second = mailbox
            .save_attachment("msg-demo-03", "att-demo-01", &folder)
            .await
            .unwrap();
        assert_eq!(second, folder.join("translation_proposal (1).pdf"));
        let saved = std::fs::read_to_string(&first).unwrap();
        assert!(saved.contains("translation_proposal.pdf"), "{saved}");
        assert!(
            mailbox
                .save_attachment("msg-demo-03", "nope", &folder)
                .await
                .is_err()
        );
        std::fs::remove_dir_all(folder).unwrap();
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
        let agenda = SearchOpts {
            keyword: Some("AGENDA".into()),
            ..SearchOpts::default()
        };
        let (found, matching) = mailbox.search(&agenda, 0).await.unwrap();
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
            .change(&[page[0].id.clone()], label_ids::INBOX, &Change::Read(true))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn the_demo_keeps_what_it_sends_in_sent_and_protects_proton_mail_alone() {
        let mailbox = Mailbox::demo();
        assert_eq!(mailbox.sender(), demo::DEMO_EMAIL);
        let addresses = ["sam@proton.me".to_owned(), "alex@example.test".to_owned()];
        assert_eq!(
            mailbox.protection(&addresses).await,
            [
                (addresses[0].clone(), Ok(RecipientProtection::EndToEnd)),
                (addresses[1].clone(), Ok(RecipientProtection::Unencrypted)),
            ]
        );
        let outgoing = Outgoing {
            kind: OutgoingKind::New,
            to: vec![addresses[0].clone()],
            cc: Vec::new(),
            bcc: vec![addresses[1].clone()],
            subject: "Plans".into(),
            body: "See you there.".into(),
            attachments: Vec::new(),
        };
        mailbox.send(&outgoing).await.unwrap();
        // The newest, so the first in Sent.
        let (_, sent) = mailbox.conversations(label_ids::SENT, 0).await.unwrap();
        assert_eq!(sent[0].subject, "Plans");
        let (_, messages) = mailbox.conversation(&sent[0].id).await.unwrap();
        let [ConversationMessage::Opened(message)] = &messages[..] else {
            panic!("{messages:?}");
        };
        assert_eq!(message.body, "See you there.");
        assert_eq!(message.meta.sender.address, demo::DEMO_EMAIL);
        assert_eq!(message.meta.bcc_list[0].address, addresses[1]);
    }

    #[tokio::test]
    async fn a_demo_answer_joins_its_conversation_with_core_s_subject_and_quote() {
        let mailbox = Mailbox::demo();
        let answer = |kind, body: &str| Outgoing {
            kind,
            to: vec!["sam.chen@example.com".into()],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body: body.into(),
            attachments: Vec::new(),
        };
        let reply = OutgoingKind::Reply {
            message_id: "msg-demo-04".into(),
            everyone: false,
            quote: Quote::Original,
        };
        mailbox.send(&answer(reply, "Count me in.")).await.unwrap();
        let (conversation, messages) = mailbox.conversation("conv-demo-03").await.unwrap();
        assert_eq!(conversation.num_messages, 2);
        let Some(ConversationMessage::Opened(sent)) = messages.last() else {
            panic!("{messages:?}");
        };
        assert_eq!(sent.meta.subject, "Re: Offsite planning and agenda");
        assert!(
            sent.body
                .starts_with("Count me in.\n\n------- Original Message -------\nOn Sam Chen <"),
            "{}",
            sent.body
        );
        assert!(sent.body.contains("\n> "), "{}", sent.body);
        // Sent lists the conversation it answered.
        let (_, listed) = mailbox.conversations(label_ids::SENT, 0).await.unwrap();
        assert_eq!(listed[0].id, "conv-demo-03");

        let forward = OutgoingKind::Forward {
            message_id: "msg-demo-04".into(),
            quote: Quote::Omitted,
            leave_out: Vec::new(),
        };
        mailbox.send(&answer(forward, "FYI")).await.unwrap();
        let (_, messages) = mailbox.conversation("conv-demo-03").await.unwrap();
        let Some(ConversationMessage::Opened(sent)) = messages.last() else {
            panic!("{messages:?}");
        };
        assert_eq!(sent.meta.subject, "Fw: Offsite planning and agenda");
        assert_eq!(sent.body, "FYI");
    }

    #[tokio::test]
    async fn the_demo_sends_files_and_forwards_the_original_s_but_those_left_out() {
        let mailbox = Mailbox::demo();
        let file =
            std::env::temp_dir().join(format!("ruston-tui-demo-file-{}.txt", std::process::id()));
        std::fs::write(&file, b"twelve bytes").unwrap();
        let forward = |leave_out: Vec<String>| Outgoing {
            kind: OutgoingKind::Forward {
                message_id: "msg-demo-03".into(),
                quote: Quote::Omitted,
                leave_out,
            },
            to: vec!["sam.chen@example.com".into()],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body: "FYI".into(),
            attachments: vec![file.clone()],
        };
        let names = |messages: &[ConversationMessage]| -> Vec<(String, u64)> {
            let Some(ConversationMessage::Opened(sent)) = messages.last() else {
                panic!("{messages:?}");
            };
            sent.attachments
                .iter()
                .map(|attachment| (attachment.name.clone(), attachment.size))
                .collect()
        };
        mailbox.send(&forward(Vec::new())).await.unwrap();
        let (_, messages) = mailbox.conversation("conv-demo-02").await.unwrap();
        let file_name = file.file_name().unwrap().to_string_lossy().into_owned();
        let attached = names(&messages);
        assert_eq!(attached[0].0, "translation_proposal.pdf");
        assert_eq!(attached[1], (file_name.clone(), 12));
        // Left out, the original's goes no more; what was sent saves.
        mailbox
            .send(&forward(vec!["att-demo-01".into()]))
            .await
            .unwrap();
        let (_, messages) = mailbox.conversation("conv-demo-02").await.unwrap();
        assert_eq!(names(&messages), [(file_name, 12)]);
        std::fs::remove_file(file).unwrap();
    }

    #[tokio::test]
    async fn mail_is_deleted_forever_from_trash_and_spam_alone() {
        let mailbox = Mailbox::demo();
        let ids = |(_, page): (u32, Vec<Conversation>)| -> Vec<String> {
            page.into_iter()
                .map(|conversation| conversation.id)
                .collect()
        };
        let inbox = ids(mailbox.conversations(label_ids::INBOX, 0).await.unwrap());
        assert!(
            mailbox
                .change(&inbox, label_ids::INBOX, &Change::Delete)
                .await
                .is_err()
        );
        assert!(mailbox.empty(label_ids::INBOX).await.is_err());
        assert!(mailbox.empty(label_ids::ALL_MAIL).await.is_err());
        assert_eq!(
            ids(mailbox.conversations(label_ids::INBOX, 0).await.unwrap()),
            inbox
        );

        let trash = ids(mailbox.conversations(label_ids::TRASH, 0).await.unwrap());
        assert!(!trash.is_empty());
        mailbox
            .change(&trash, label_ids::TRASH, &Change::Delete)
            .await
            .unwrap();
        assert!(ids(mailbox.conversations(label_ids::TRASH, 0).await.unwrap()).is_empty());
        mailbox
            .change(
                &inbox[..1],
                label_ids::INBOX,
                &Change::Move(label_ids::SPAM.into()),
            )
            .await
            .unwrap();
        mailbox.empty(label_ids::SPAM).await.unwrap();
        assert!(ids(mailbox.conversations(label_ids::SPAM, 0).await.unwrap()).is_empty());
        assert_eq!(
            ids(mailbox.conversations(label_ids::INBOX, 0).await.unwrap()),
            inbox[1..]
        );
    }
}
