//! The demo's mail, the people it is from and to, its folders, labels and
//! filters: each written once.

use super::{DEMO_ALIAS, DEMO_EMAIL, DEMO_NAME, DemoMessage};
use ruston_core::model::ConversationLabel;
use ruston_core::model::enums::{filter, label_ids, label_type};
use ruston_core::{
    AddressInfo, Attachment, Contact, ContactEmail, Conversation, Filter, Label, LabelCount,
    MessageMetadata, Recipient,
};

/// The demo's mail: its messages and the conversations they are in.
pub(super) struct DemoMailbox {
    pub(super) messages: Vec<DemoMessage>,
    pub(super) conversations: Vec<Conversation>,
}

/// Someone the demo's mail is from or to, or one of its contacts.
pub(super) struct Person {
    name: &'static str,
    address: &'static str,
    /// The number of their contact, `c-` and it, if the demo has one.
    contact: Option<u8>,
}

impl Person {
    pub(super) fn recipient(&self) -> Recipient {
        Recipient {
            name: self.name.to_owned(),
            address: self.address.to_owned(),
            contact_id: self.contact.map(|number| format!("c-{number}")),
            is_proton: Some(1),
        }
    }

    /// The contact the demo keeps for them, with their one address.
    fn contact(&self) -> Contact {
        let number = self.contact.expect("a contact has a number");
        let id = format!("c-{number}");
        Contact {
            id: id.clone(),
            name: self.name.to_owned(),
            emails: vec![ContactEmail {
                id: format!("ce-{number}"),
                email: self.address.to_owned(),
                name: self.name.to_owned(),
                contact_id: id,
            }],
        }
    }
}

/// The demo's user.
pub(super) const USER: Person = Person {
    name: DEMO_NAME,
    address: DEMO_EMAIL,
    contact: None,
};
const ALEX: Person = Person {
    name: "Alex Reed",
    address: "alex.reed@example.com",
    contact: Some(1),
};
const SAM: Person = Person {
    name: "Sam Chen",
    address: "sam.chen@example.com",
    contact: Some(2),
};
const PRIYA: Person = Person {
    name: "Priya Natarajan",
    address: "priya.natarajan@example.com",
    contact: Some(3),
};
const JORDAN: Person = Person {
    name: "Jordan Lee",
    address: "jordan.lee@example.com",
    contact: Some(4),
};
const RILEY: Person = Person {
    name: "Riley Davis",
    address: "riley.davis@example.com",
    contact: Some(5),
};
const DREW: Person = Person {
    name: "Drew Kelly",
    address: "drew.kelly@example.com",
    contact: Some(6),
};
const ROBIN: Person = Person {
    name: "Robin Fischer",
    address: "robin.fischer@example.com",
    contact: Some(7),
};
const EMERSON: Person = Person {
    name: "Emerson Clark",
    address: "emerson.clark@example.com",
    contact: Some(8),
};
const TEAM: Person = Person {
    name: "Engineering Team",
    address: "team@example.com",
    contact: None,
};

/// The people the demo's contacts are, as `contacts list` lists them.
const CONTACTS: [Person; 8] = [ALEX, SAM, PRIYA, JORDAN, RILEY, DREW, ROBIN, EMERSON];

/// A conversation's place: the label or folder `id`, which holds `messages`
/// of its messages, `unread` of them unread, the last of them at `time`.
fn placed(id: &str, messages: i64, unread: i64, time: i64) -> ConversationLabel {
    ConversationLabel {
        id: id.to_owned(),
        context_num_messages: Some(messages),
        context_num_unread: Some(unread),
        context_time: Some(time),
    }
}

/// Generate static demo fixtures.
pub(super) fn fixtures() -> DemoMailbox {
    let now = 1_714_000_000_i64; // fixed stable epoch for deterministic testing

    let messages = vec![
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-01".to_string(),
                order: 1,
                conversation_id: "conv-demo-01".to_string(),
                subject: "Settings screen review and feedback".to_string(),
                unread: 1,
                sender: ALEX.recipient(),
                to_list: vec![USER.recipient()],
                time: now - 3600,
                size: 1420,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![
                    label_ids::INBOX.to_string(),
                    label_ids::STARRED.to_string(),
                    "label-demo-work".to_string(),
                ],
                external_id: Some("<alex-review-1@example.com>".to_string()),
                ..MessageMetadata::default()
            },
            body: "Hi Demo,\n\nI reviewed the settings screen. The account section reads clearly, but the notification toggles need stronger labels. I left comments on the latest mockup.\n\nCould you update those labels before tomorrow's review?\n\nThanks,\nAlex\n".to_string(),
            ..DemoMessage::default()
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-02".to_string(),
                order: 2,
                conversation_id: "conv-demo-01".to_string(),
                subject: "Re: Settings screen review and feedback".to_string(),
                sender: USER.recipient(),
                to_list: vec![ALEX.recipient()],
                time: now - 1800,
                size: 1850,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::SENT.to_string(), "label-demo-work".to_string()],
                external_id: Some("<demo-review-reply@example.com>".to_string()),
                ..MessageMetadata::default()
            },
            body: "Hi Alex,\n\nI updated the notification labels and grouped the recovery options under Security. I also added helper text for the two settings that were ambiguous.\n\nThe revised mockup is ready for another pass.\n\nThanks,\nDemo\n".to_string(),
            ..DemoMessage::default()
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-03".to_string(),
                order: 3,
                conversation_id: "conv-demo-02".to_string(),
                subject: "Translation workflow proposal".to_string(),
                unread: 1,
                sender: RILEY.recipient(),
                to_list: vec![USER.recipient()],
                cc_list: vec![ALEX.recipient()],
                time: now - 7200,
                size: 28400,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::INBOX.to_string(), "label-demo-work".to_string()],
                external_id: Some("<riley-trans-1@example.com>".to_string()),
                num_attachments: 1,
                ..MessageMetadata::default()
            },
            body: "Hi Demo,\n\nI attached the first proposal for the translation workflow. It covers string extraction, reviewer assignment, and the release cutoff. The glossary we agreed on is at https://example.com/translation-glossary.\n\nCould you check the engineering steps?\n\nThanks,\nRiley\n".to_string(),
            attachments: vec![Attachment {
                id: "att-demo-01".to_string(),
                name: "translation_proposal.pdf".to_string(),
                size: 24576,
                mime_type: Some("application/pdf".to_string()),
                disposition: Some("attachment".to_string()),
                ..Attachment::default()
            }],
            ..DemoMessage::default()
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-04".to_string(),
                order: 4,
                conversation_id: "conv-demo-03".to_string(),
                subject: "Offsite planning and agenda".to_string(),
                sender: SAM.recipient(),
                to_list: vec![USER.recipient()],
                time: now - 86400,
                size: 1200,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::INBOX.to_string()],
                external_id: Some("<sam-offsite-1@example.com>".to_string()),
                ..MessageMetadata::default()
            },
            // In HTML, quoting the message it answers, with a link whose
            // words are not its address.
            body: concat!(
                "<p>Hi everyone,</p>",
                "<p>Please add your topics for the offsite by Friday. I have reserved the morning ",
                "for planning and the afternoon for team discussions. The draft is in ",
                "<a href=\"https://example.com/offsite/agenda\">the shared agenda</a>.</p>",
                "<p>Thanks,<br>Sam</p>",
                "<p>On Monday, Priya Natarajan wrote:</p>",
                "<blockquote><p>Hi Sam,</p>",
                "<p>The venue is booked for the whole day, and lunch is included.</p>",
                "<p>The room has a projector and two whiteboards.</p>",
                "<p>Best,<br>Priya</p></blockquote>",
            )
            .to_string(),
            mime_type: "text/html".to_string(),
            ..DemoMessage::default()
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-05".to_string(),
                order: 5,
                conversation_id: "conv-demo-04".to_string(),
                subject: "Keyboard shortcuts proposal".to_string(),
                sender: DREW.recipient(),
                to_list: vec![USER.recipient()],
                time: now - 172800,
                size: 2100,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::ARCHIVE.to_string()],
                external_id: Some("<drew-shortcuts-1@example.com>".to_string()),
                ..MessageMetadata::default()
            },
            body: "Hi Drew,\n\nI drafted shortcuts for archive, delete, reply, and folder navigation. Let's discuss when you get a chance.\n\nThanks,\nDrew\n".to_string(),
            ..DemoMessage::default()
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-06".to_string(),
                order: 6,
                conversation_id: "conv-demo-05".to_string(),
                subject: "Apartment viewing on Oak Avenue".to_string(),
                sender: ROBIN.recipient(),
                to_list: vec![USER.recipient()],
                time: now - 259200,
                size: 1600,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::INBOX.to_string(), "label-demo-personal".to_string()],
                external_id: Some("<robin-apartment-1@example.com>".to_string()),
                ..MessageMetadata::default()
            },
            body: "Hi Demo,\n\nI can show the apartment on Tuesday at 17:30 or Thursday at 18:15. The viewing takes about 30 minutes.\n\nBest,\nRobin\n".to_string(),
            ..DemoMessage::default()
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-07".to_string(),
                order: 7,
                conversation_id: "conv-demo-06".to_string(),
                subject: "Garden fence repair update".to_string(),
                sender: EMERSON.recipient(),
                to_list: vec![USER.recipient()],
                time: now - 345600,
                size: 980,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::TRASH.to_string()],
                external_id: Some("<emerson-fence-1@example.com>".to_string()),
                ..MessageMetadata::default()
            },
            body: "Hi Demo,\n\nI checked the fence this morning. The posts are sound, so we only need three new boards and exterior screws.\n\nEmerson\n".to_string(),
            ..DemoMessage::default()
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-08".to_string(),
                order: 8,
                conversation_id: "conv-demo-07".to_string(),
                subject: "Draft: Release roadmap 2026".to_string(),
                sender: USER.recipient(),
                to_list: vec![TEAM.recipient()],
                time: now - 600,
                size: 840,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::DRAFTS.to_string(), label_ids::ALL_DRAFTS.to_string()],
                ..MessageMetadata::default()
            },
            body: "Team,\n\nHere is our proposed release schedule for ruston-mail and ruston-cli.\n\nBest,\nDemo\n".to_string(),
            ..DemoMessage::default()
        },
    ];

    let conversations = vec![
        Conversation {
            id: "conv-demo-01".to_string(),
            subject: "Settings screen review and feedback".to_string(),
            time: now - 1800,
            num_messages: 2,
            num_unread: 1,
            senders: vec![ALEX.recipient(), USER.recipient()],
            recipients: vec![USER.recipient(), ALEX.recipient()],
            labels: vec![
                placed(label_ids::INBOX, 1, 1, now - 3600),
                placed(label_ids::SENT, 1, 0, now - 1800),
                placed(label_ids::STARRED, 1, 1, now - 3600),
                placed("label-demo-work", 2, 1, now - 1800),
            ],
            ..Conversation::default()
        },
        Conversation {
            id: "conv-demo-02".to_string(),
            subject: "Translation workflow proposal".to_string(),
            time: now - 7200,
            num_messages: 1,
            num_unread: 1,
            num_attachments: 1,
            senders: vec![RILEY.recipient()],
            recipients: vec![USER.recipient()],
            labels: vec![
                placed(label_ids::INBOX, 1, 1, now - 7200),
                placed("label-demo-work", 1, 1, now - 7200),
            ],
            ..Conversation::default()
        },
        Conversation {
            id: "conv-demo-03".to_string(),
            subject: "Offsite planning and agenda".to_string(),
            time: now - 86400,
            num_messages: 1,
            senders: vec![SAM.recipient()],
            recipients: vec![USER.recipient()],
            labels: vec![placed(label_ids::INBOX, 1, 0, now - 86400)],
            ..Conversation::default()
        },
        Conversation {
            id: "conv-demo-04".to_string(),
            subject: "Keyboard shortcuts proposal".to_string(),
            time: now - 172800,
            num_messages: 1,
            senders: vec![DREW.recipient()],
            recipients: vec![USER.recipient()],
            labels: vec![placed(label_ids::ARCHIVE, 1, 0, now - 172800)],
            ..Conversation::default()
        },
        Conversation {
            id: "conv-demo-05".to_string(),
            subject: "Apartment viewing on Oak Avenue".to_string(),
            time: now - 259200,
            num_messages: 1,
            senders: vec![ROBIN.recipient()],
            recipients: vec![USER.recipient()],
            labels: vec![
                placed(label_ids::INBOX, 1, 0, now - 259200),
                placed("label-demo-personal", 1, 0, now - 259200),
            ],
            ..Conversation::default()
        },
        Conversation {
            id: "conv-demo-06".to_string(),
            subject: "Garden fence repair update".to_string(),
            time: now - 345600,
            num_messages: 1,
            senders: vec![EMERSON.recipient()],
            recipients: vec![USER.recipient()],
            labels: vec![placed(label_ids::TRASH, 1, 0, now - 345600)],
            ..Conversation::default()
        },
        Conversation {
            id: "conv-demo-07".to_string(),
            subject: "Draft: Release roadmap 2026".to_string(),
            time: now - 600,
            num_messages: 1,
            senders: vec![USER.recipient()],
            recipients: vec![TEAM.recipient()],
            labels: vec![
                placed(label_ids::DRAFTS, 1, 0, now - 600),
                placed(label_ids::ALL_DRAFTS, 1, 0, now - 600),
            ],
            ..Conversation::default()
        },
    ];

    DemoMailbox {
        messages,
        conversations,
    }
}

pub(super) fn demo_contacts() -> Vec<Contact> {
    CONTACTS.iter().map(Person::contact).collect()
}

pub(crate) fn demo_addresses() -> Vec<AddressInfo> {
    vec![
        AddressInfo {
            id: "addr-demo-01".to_string(),
            email: DEMO_EMAIL.to_string(),
            can_send: true,
        },
        AddressInfo {
            id: "addr-demo-02".to_string(),
            email: DEMO_ALIAS.to_string(),
            can_send: true,
        },
    ]
}

pub(crate) fn demo_labels() -> Vec<Label> {
    vec![
        Label {
            id: "label-demo-work".to_string(),
            name: "Work".to_string(),
            color: "#0066cc".to_string(),
            path: "Work".to_string(),
            label_type: label_type::MESSAGE_LABEL,
            order: 1,
            ..Label::default()
        },
        Label {
            id: "label-demo-personal".to_string(),
            name: "Personal".to_string(),
            color: "#00aa55".to_string(),
            path: "Personal".to_string(),
            label_type: label_type::MESSAGE_LABEL,
            order: 2,
            ..Label::default()
        },
        Label {
            id: "folder-demo-projects".to_string(),
            name: "Projects".to_string(),
            color: "#ff8800".to_string(),
            path: "Projects".to_string(),
            label_type: label_type::MESSAGE_FOLDER,
            order: 3,
            ..Label::default()
        },
    ]
}

/// What `counts` reports: the conversations or the messages each place
/// holds, worked out from the demo's mail, as the terminal interface's are.
pub(crate) fn demo_counts(conversations: bool) -> Vec<LabelCount> {
    let mailbox = fixtures();
    if conversations {
        super::conversation_counts(&mailbox.conversations)
    } else {
        super::message_counts(&mailbox.messages)
    }
}

pub(super) fn demo_filters() -> Vec<Filter> {
    vec![Filter {
        id: "filter-demo-01".to_string(),
        name: "Auto-label Work".to_string(),
        status: filter::ENABLED,
        version: filter::SIEVE_VERSION,
    }]
}
