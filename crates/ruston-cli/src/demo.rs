//! Offline demo mode providing a self-contained simulated mailbox.
//!
//! Enables testing, CI automation, and CLI demonstrations without contacting Proton
//! or requiring real user credentials. Activated via `--demo` or `RUSTON_DEMO=1`.

use std::fs;
use std::path::{Path, PathBuf};

use ruston_core::model::ConversationLabel;
use ruston_core::model::enums::{label_ids, resolve_folder};
use ruston_core::{
    AddressInfo, Attachment, Contact, ContactEmail, Conversation, Error, Filter, FullMessage,
    Label, LabelCount, MessageMetadata, Recipient, Result, Verdict,
};
use serde_json::json;

use crate::cli::{
    AddressesCmd, AttachmentsCmd, Command, ContactsCmd, ConversationsCmd, Ctx, DraftsCmd,
    FiltersCmd, LabelsCmd, MessagesCmd, SettingsCmd,
};
use crate::render;

/// Fictional user details.
const DEMO_EMAIL: &str = "demo@example.com";
const DEMO_NAME: &str = "Demo User";
const DEMO_ALIAS: &str = "volunteers@example.org";

struct DemoMessage {
    meta: MessageMetadata,
    body: String,
    mime_type: String,
    attachments: Vec<Attachment>,
}

impl DemoMessage {
    fn to_full(&self) -> FullMessage {
        FullMessage {
            meta: self.meta.clone(),
            body: self.body.clone(),
            mime_type: self.mime_type.clone(),
            verdict: Verdict::Verified,
            attachments: self.attachments.clone(),
        }
    }
}

/// Generate static demo fixtures.
fn fixtures() -> (Vec<DemoMessage>, Vec<Conversation>) {
    let now = 1_714_000_000_i64; // fixed stable epoch for deterministic testing

    let user_recipient = Recipient {
        name: DEMO_NAME.to_string(),
        address: DEMO_EMAIL.to_string(),
        contact_id: None,
        is_proton: Some(1),
    };

    let alex = Recipient {
        name: "Alex Reed".to_string(),
        address: "alex.reed@example.com".to_string(),
        contact_id: Some("c-1".to_string()),
        is_proton: Some(1),
    };

    let riley = Recipient {
        name: "Riley Davis".to_string(),
        address: "riley.davis@example.com".to_string(),
        contact_id: Some("c-5".to_string()),
        is_proton: Some(1),
    };

    let sam = Recipient {
        name: "Sam Chen".to_string(),
        address: "sam.chen@example.com".to_string(),
        contact_id: Some("c-2".to_string()),
        is_proton: Some(1),
    };

    let drew = Recipient {
        name: "Drew Kelly".to_string(),
        address: "drew.kelly@example.com".to_string(),
        contact_id: Some("c-6".to_string()),
        is_proton: Some(1),
    };

    let robin = Recipient {
        name: "Robin Fischer".to_string(),
        address: "robin.fischer@example.com".to_string(),
        contact_id: Some("c-7".to_string()),
        is_proton: Some(1),
    };

    let emerson = Recipient {
        name: "Emerson Clark".to_string(),
        address: "emerson.clark@example.com".to_string(),
        contact_id: Some("c-8".to_string()),
        is_proton: Some(1),
    };

    let team = Recipient {
        name: "Engineering Team".to_string(),
        address: "team@example.com".to_string(),
        contact_id: None,
        is_proton: Some(1),
    };

    let messages = vec![
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-01".to_string(),
                order: 1,
                conversation_id: "conv-demo-01".to_string(),
                subject: "Settings screen review and feedback".to_string(),
                unread: 1,
                sender: alex.clone(),
                to_list: vec![user_recipient.clone()],
                cc_list: Vec::new(),
                bcc_list: Vec::new(),
                time: now - 3600,
                size: 1420,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![
                    label_ids::INBOX.to_string(),
                    label_ids::STARRED.to_string(),
                    "label-demo-work".to_string(),
                ],
                external_id: Some("<alex-review-1@example.com>".to_string()),
                num_attachments: 0,
                flags: 0,
                expiration_time: None,
            },
            body: "Hi Demo,\n\nI reviewed the settings screen. The account section reads clearly, but the notification toggles need stronger labels. I left comments on the latest mockup.\n\nCould you update those labels before tomorrow's review?\n\nThanks,\nAlex\n".to_string(),
            mime_type: "text/plain".to_string(),
            attachments: Vec::new(),
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-02".to_string(),
                order: 2,
                conversation_id: "conv-demo-01".to_string(),
                subject: "Re: Settings screen review and feedback".to_string(),
                unread: 0,
                sender: user_recipient.clone(),
                to_list: vec![alex.clone()],
                cc_list: Vec::new(),
                bcc_list: Vec::new(),
                time: now - 1800,
                size: 1850,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::SENT.to_string(), "label-demo-work".to_string()],
                external_id: Some("<demo-review-reply@example.com>".to_string()),
                num_attachments: 0,
                flags: 0,
                expiration_time: None,
            },
            body: "Hi Alex,\n\nI updated the notification labels and grouped the recovery options under Security. I also added helper text for the two settings that were ambiguous.\n\nThe revised mockup is ready for another pass.\n\nThanks,\nDemo\n".to_string(),
            mime_type: "text/plain".to_string(),
            attachments: Vec::new(),
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-03".to_string(),
                order: 3,
                conversation_id: "conv-demo-02".to_string(),
                subject: "Translation workflow proposal".to_string(),
                unread: 1,
                sender: riley.clone(),
                to_list: vec![user_recipient.clone()],
                cc_list: vec![alex.clone()],
                bcc_list: Vec::new(),
                time: now - 7200,
                size: 28400,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::INBOX.to_string(), "label-demo-work".to_string()],
                external_id: Some("<riley-trans-1@example.com>".to_string()),
                num_attachments: 1,
                flags: 0,
                expiration_time: None,
            },
            body: "Hi Demo,\n\nI attached the first proposal for the translation workflow. It covers string extraction, reviewer assignment, and the release cutoff.\n\nCould you check the engineering steps?\n\nThanks,\nRiley\n".to_string(),
            mime_type: "text/plain".to_string(),
            attachments: vec![Attachment {
                id: "att-demo-01".to_string(),
                name: "translation_proposal.pdf".to_string(),
                size: 24576,
                key_packets: None,
                mime_type: Some("application/pdf".to_string()),
                disposition: Some("attachment".to_string()),
                signature: None,
            }],
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-04".to_string(),
                order: 4,
                conversation_id: "conv-demo-03".to_string(),
                subject: "Offsite planning and agenda".to_string(),
                unread: 0,
                sender: sam.clone(),
                to_list: vec![user_recipient.clone()],
                cc_list: Vec::new(),
                bcc_list: Vec::new(),
                time: now - 86400,
                size: 1200,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::INBOX.to_string()],
                external_id: Some("<sam-offsite-1@example.com>".to_string()),
                num_attachments: 0,
                flags: 0,
                expiration_time: None,
            },
            body: "Hi everyone,\n\nPlease add your topics for the offsite by Friday. I have reserved the morning for planning and the afternoon for team discussions.\n\nThanks,\nSam\n".to_string(),
            mime_type: "text/plain".to_string(),
            attachments: Vec::new(),
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-05".to_string(),
                order: 5,
                conversation_id: "conv-demo-04".to_string(),
                subject: "Keyboard shortcuts proposal".to_string(),
                unread: 0,
                sender: drew.clone(),
                to_list: vec![user_recipient.clone()],
                cc_list: Vec::new(),
                bcc_list: Vec::new(),
                time: now - 172800,
                size: 2100,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::ARCHIVE.to_string()],
                external_id: Some("<drew-shortcuts-1@example.com>".to_string()),
                num_attachments: 0,
                flags: 0,
                expiration_time: None,
            },
            body: "Hi Drew,\n\nI drafted shortcuts for archive, delete, reply, and folder navigation. Let's discuss when you get a chance.\n\nThanks,\nDrew\n".to_string(),
            mime_type: "text/plain".to_string(),
            attachments: Vec::new(),
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-06".to_string(),
                order: 6,
                conversation_id: "conv-demo-05".to_string(),
                subject: "Apartment viewing on Oak Avenue".to_string(),
                unread: 0,
                sender: robin.clone(),
                to_list: vec![user_recipient.clone()],
                cc_list: Vec::new(),
                bcc_list: Vec::new(),
                time: now - 259200,
                size: 1600,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::INBOX.to_string(), "label-demo-personal".to_string()],
                external_id: Some("<robin-apartment-1@example.com>".to_string()),
                num_attachments: 0,
                flags: 0,
                expiration_time: None,
            },
            body: "Hi Demo,\n\nI can show the apartment on Tuesday at 17:30 or Thursday at 18:15. The viewing takes about 30 minutes.\n\nBest,\nRobin\n".to_string(),
            mime_type: "text/plain".to_string(),
            attachments: Vec::new(),
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-07".to_string(),
                order: 7,
                conversation_id: "conv-demo-06".to_string(),
                subject: "Garden fence repair update".to_string(),
                unread: 0,
                sender: emerson.clone(),
                to_list: vec![user_recipient.clone()],
                cc_list: Vec::new(),
                bcc_list: Vec::new(),
                time: now - 345600,
                size: 980,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::TRASH.to_string()],
                external_id: Some("<emerson-fence-1@example.com>".to_string()),
                num_attachments: 0,
                flags: 0,
                expiration_time: None,
            },
            body: "Hi Demo,\n\nI checked the fence this morning. The posts are sound, so we only need three new boards and exterior screws.\n\nEmerson\n".to_string(),
            mime_type: "text/plain".to_string(),
            attachments: Vec::new(),
        },
        DemoMessage {
            meta: MessageMetadata {
                id: "msg-demo-08".to_string(),
                order: 8,
                conversation_id: "conv-demo-07".to_string(),
                subject: "Draft: Release roadmap 2026".to_string(),
                unread: 0,
                sender: user_recipient.clone(),
                to_list: vec![team.clone()],
                cc_list: Vec::new(),
                bcc_list: Vec::new(),
                time: now - 600,
                size: 840,
                address_id: "addr-demo-01".to_string(),
                label_ids: vec![label_ids::DRAFTS.to_string(), label_ids::ALL_DRAFTS.to_string()],
                external_id: None,
                num_attachments: 0,
                flags: 0,
                expiration_time: None,
            },
            body: "Team,\n\nHere is our proposed release schedule for ruston-mail and ruston-cli.\n\nBest,\nDemo\n".to_string(),
            mime_type: "text/plain".to_string(),
            attachments: Vec::new(),
        },
    ];

    let conversations = vec![
        Conversation {
            id: "conv-demo-01".to_string(),
            subject: "Settings screen review and feedback".to_string(),
            time: now - 1800,
            num_messages: 2,
            num_unread: 1,
            num_attachments: 0,
            senders: vec![alex.clone(), user_recipient.clone()],
            recipients: vec![user_recipient.clone(), alex.clone()],
            labels: vec![
                ConversationLabel {
                    id: label_ids::INBOX.to_string(),
                    context_num_messages: Some(1),
                    context_num_unread: Some(1),
                    context_time: Some(now - 3600),
                },
                ConversationLabel {
                    id: label_ids::SENT.to_string(),
                    context_num_messages: Some(1),
                    context_num_unread: Some(0),
                    context_time: Some(now - 1800),
                },
            ],
            expiration_time: None,
        },
        Conversation {
            id: "conv-demo-02".to_string(),
            subject: "Translation workflow proposal".to_string(),
            time: now - 7200,
            num_messages: 1,
            num_unread: 1,
            num_attachments: 1,
            senders: vec![riley.clone()],
            recipients: vec![user_recipient.clone()],
            labels: vec![ConversationLabel {
                id: label_ids::INBOX.to_string(),
                context_num_messages: Some(1),
                context_num_unread: Some(1),
                context_time: Some(now - 7200),
            }],
            expiration_time: None,
        },
        Conversation {
            id: "conv-demo-03".to_string(),
            subject: "Offsite planning and agenda".to_string(),
            time: now - 86400,
            num_messages: 1,
            num_unread: 0,
            num_attachments: 0,
            senders: vec![sam.clone()],
            recipients: vec![user_recipient.clone()],
            labels: vec![ConversationLabel {
                id: label_ids::INBOX.to_string(),
                context_num_messages: Some(1),
                context_num_unread: Some(0),
                context_time: Some(now - 86400),
            }],
            expiration_time: None,
        },
        Conversation {
            id: "conv-demo-04".to_string(),
            subject: "Keyboard shortcuts proposal".to_string(),
            time: now - 172800,
            num_messages: 1,
            num_unread: 0,
            num_attachments: 0,
            senders: vec![drew.clone()],
            recipients: vec![user_recipient.clone()],
            labels: vec![ConversationLabel {
                id: label_ids::ARCHIVE.to_string(),
                context_num_messages: Some(1),
                context_num_unread: Some(0),
                context_time: Some(now - 172800),
            }],
            expiration_time: None,
        },
        Conversation {
            id: "conv-demo-05".to_string(),
            subject: "Apartment viewing on Oak Avenue".to_string(),
            time: now - 259200,
            num_messages: 1,
            num_unread: 0,
            num_attachments: 0,
            senders: vec![robin.clone()],
            recipients: vec![user_recipient.clone()],
            labels: vec![ConversationLabel {
                id: label_ids::INBOX.to_string(),
                context_num_messages: Some(1),
                context_num_unread: Some(0),
                context_time: Some(now - 259200),
            }],
            expiration_time: None,
        },
        Conversation {
            id: "conv-demo-06".to_string(),
            subject: "Garden fence repair update".to_string(),
            time: now - 345600,
            num_messages: 1,
            num_unread: 0,
            num_attachments: 0,
            senders: vec![emerson.clone()],
            recipients: vec![user_recipient.clone()],
            labels: vec![ConversationLabel {
                id: label_ids::TRASH.to_string(),
                context_num_messages: Some(1),
                context_num_unread: Some(0),
                context_time: Some(now - 345600),
            }],
            expiration_time: None,
        },
    ];

    (messages, conversations)
}

fn demo_contacts() -> Vec<Contact> {
    vec![
        Contact {
            id: "c-1".to_string(),
            name: "Alex Reed".to_string(),
            emails: vec![ContactEmail {
                id: "ce-1".to_string(),
                email: "alex.reed@example.com".to_string(),
                name: "Alex Reed".to_string(),
                contact_id: "c-1".to_string(),
            }],
        },
        Contact {
            id: "c-2".to_string(),
            name: "Sam Chen".to_string(),
            emails: vec![ContactEmail {
                id: "ce-2".to_string(),
                email: "sam.chen@example.com".to_string(),
                name: "Sam Chen".to_string(),
                contact_id: "c-2".to_string(),
            }],
        },
        Contact {
            id: "c-3".to_string(),
            name: "Priya Natarajan".to_string(),
            emails: vec![ContactEmail {
                id: "ce-3".to_string(),
                email: "priya.natarajan@example.com".to_string(),
                name: "Priya Natarajan".to_string(),
                contact_id: "c-3".to_string(),
            }],
        },
        Contact {
            id: "c-4".to_string(),
            name: "Jordan Lee".to_string(),
            emails: vec![ContactEmail {
                id: "ce-4".to_string(),
                email: "jordan.lee@example.com".to_string(),
                name: "Jordan Lee".to_string(),
                contact_id: "c-4".to_string(),
            }],
        },
        Contact {
            id: "c-5".to_string(),
            name: "Riley Davis".to_string(),
            emails: vec![ContactEmail {
                id: "ce-5".to_string(),
                email: "riley.davis@example.com".to_string(),
                name: "Riley Davis".to_string(),
                contact_id: "c-5".to_string(),
            }],
        },
        Contact {
            id: "c-6".to_string(),
            name: "Drew Kelly".to_string(),
            emails: vec![ContactEmail {
                id: "ce-6".to_string(),
                email: "drew.kelly@example.com".to_string(),
                name: "Drew Kelly".to_string(),
                contact_id: "c-6".to_string(),
            }],
        },
    ]
}

fn demo_addresses() -> Vec<AddressInfo> {
    vec![
        AddressInfo {
            id: "addr-demo-01".to_string(),
            email: DEMO_EMAIL.to_string(),
        },
        AddressInfo {
            id: "addr-demo-02".to_string(),
            email: DEMO_ALIAS.to_string(),
        },
    ]
}

fn demo_labels() -> Vec<Label> {
    vec![
        Label {
            id: "label-demo-work".to_string(),
            name: "Work".to_string(),
            color: "#0066cc".to_string(),
            path: "Work".to_string(),
            label_type: 1,
            parent_id: None,
            order: 1,
            notify: None,
        },
        Label {
            id: "label-demo-personal".to_string(),
            name: "Personal".to_string(),
            color: "#00aa55".to_string(),
            path: "Personal".to_string(),
            label_type: 1,
            parent_id: None,
            order: 2,
            notify: None,
        },
        Label {
            id: "folder-demo-projects".to_string(),
            name: "Projects".to_string(),
            color: "#ff8800".to_string(),
            path: "Projects".to_string(),
            label_type: 3,
            parent_id: None,
            order: 3,
            notify: None,
        },
    ]
}

fn demo_counts(conversations: bool) -> Vec<LabelCount> {
    if conversations {
        vec![
            LabelCount {
                label_id: label_ids::INBOX.to_string(),
                total: 4,
                unread: 2,
            },
            LabelCount {
                label_id: label_ids::TRASH.to_string(),
                total: 1,
                unread: 0,
            },
            LabelCount {
                label_id: label_ids::ARCHIVE.to_string(),
                total: 1,
                unread: 0,
            },
            LabelCount {
                label_id: label_ids::ALL_MAIL.to_string(),
                total: 6,
                unread: 2,
            },
            LabelCount {
                label_id: label_ids::STARRED.to_string(),
                total: 1,
                unread: 1,
            },
        ]
    } else {
        vec![
            LabelCount {
                label_id: label_ids::INBOX.to_string(),
                total: 4,
                unread: 2,
            },
            LabelCount {
                label_id: label_ids::ALL_DRAFTS.to_string(),
                total: 1,
                unread: 0,
            },
            LabelCount {
                label_id: label_ids::ALL_SENT.to_string(),
                total: 1,
                unread: 0,
            },
            LabelCount {
                label_id: label_ids::TRASH.to_string(),
                total: 1,
                unread: 0,
            },
            LabelCount {
                label_id: label_ids::SPAM.to_string(),
                total: 0,
                unread: 0,
            },
            LabelCount {
                label_id: label_ids::ALL_MAIL.to_string(),
                total: 8,
                unread: 2,
            },
            LabelCount {
                label_id: label_ids::ARCHIVE.to_string(),
                total: 1,
                unread: 0,
            },
            LabelCount {
                label_id: label_ids::SENT.to_string(),
                total: 1,
                unread: 0,
            },
            LabelCount {
                label_id: label_ids::DRAFTS.to_string(),
                total: 1,
                unread: 0,
            },
            LabelCount {
                label_id: label_ids::STARRED.to_string(),
                total: 1,
                unread: 1,
            },
        ]
    }
}

fn demo_filters() -> Vec<Filter> {
    vec![Filter {
        id: "filter-demo-01".to_string(),
        name: "Auto-label Work".to_string(),
        status: 1,
        version: 1,
    }]
}

/// Sanitizes an attachment filename to prevent directory traversal and Windows DOS reserved names.
fn safe_attachment_name(name: &str) -> String {
    let basename = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("attachment.bin");

    let clean: String = basename
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .collect();

    let stem = clean.split('.').next().unwrap_or(&clean);
    let upper = stem.to_ascii_uppercase();
    let is_reserved = matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );

    if clean.is_empty() || clean == "." || clean == ".." || is_reserved {
        format!("safe_{clean}")
    } else {
        clean
    }
}

/// Dispatcher for CLI commands in offline demo mode.
pub async fn dispatch(ctx: &Ctx, cmd: Command) -> Result<()> {
    let (fixtures_messages, fixtures_convs) = fixtures();

    match cmd {
        Command::Login => {
            if ctx.json {
                render::json_out(&json!({
                    "status": "ok",
                    "email": DEMO_EMAIL,
                    "mode": "demo"
                }));
            } else {
                println!("Logged in as {DEMO_EMAIL} (offline demo mode)");
            }
            Ok(())
        }
        Command::Logout => {
            if ctx.json {
                render::json_out(&json!({
                    "status": "ok",
                    "message": "Demo session ended"
                }));
            } else {
                println!("Logged out of demo session.");
            }
            Ok(())
        }
        Command::Whoami => {
            if ctx.json {
                render::json_out(&json!({
                    "email": DEMO_EMAIL,
                    "mode": "demo"
                }));
            } else {
                println!("{DEMO_EMAIL} (demo)");
            }
            Ok(())
        }
        Command::Messages { cmd } => match cmd {
            MessagesCmd::List {
                folder,
                page,
                page_size,
                unread,
                cached: _,
            } => {
                let target_label = resolve_folder(&folder);
                let is_all = target_label == label_ids::ALL_MAIL
                    || folder.eq_ignore_ascii_case("all")
                    || folder.eq_ignore_ascii_case("allmail");

                let mut matching: Vec<MessageMetadata> = fixtures_messages
                    .into_iter()
                    .filter(|m| is_all || m.meta.label_ids.contains(&target_label))
                    .filter(|m| !unread || m.meta.unread != 0)
                    .map(|m| m.meta)
                    .collect();

                matching.sort_by_key(|a| std::cmp::Reverse(a.time));
                let total = matching.len() as u32;
                let start = (page * page_size) as usize;
                let paged: Vec<MessageMetadata> = matching
                    .into_iter()
                    .skip(start)
                    .take(page_size as usize)
                    .collect();

                render::messages_list(ctx.json, total, &paged);
                Ok(())
            }
            MessagesCmd::Search(args) => {
                let mut results: Vec<MessageMetadata> = fixtures_messages
                    .into_iter()
                    .filter(|m| {
                        if let Some(kw) = &args.keyword {
                            let kw = kw.to_lowercase();
                            if !m.meta.subject.to_lowercase().contains(&kw)
                                && !m.body.to_lowercase().contains(&kw)
                            {
                                return false;
                            }
                        }
                        if let Some(from) = &args.from
                            && !m
                                .meta
                                .sender
                                .address
                                .to_lowercase()
                                .contains(&from.to_lowercase())
                            && !m
                                .meta
                                .sender
                                .name
                                .to_lowercase()
                                .contains(&from.to_lowercase())
                        {
                            return false;
                        }
                        if let Some(subject) = &args.subject
                            && !m
                                .meta
                                .subject
                                .to_lowercase()
                                .contains(&subject.to_lowercase())
                        {
                            return false;
                        }
                        if args.unread && m.meta.unread == 0 {
                            return false;
                        }
                        true
                    })
                    .map(|m| m.meta)
                    .collect();

                results.sort_by_key(|a| std::cmp::Reverse(a.time));
                if let Some(limit) = args.limit {
                    results.truncate(limit as usize);
                }
                render::messages_list(ctx.json, results.len() as u32, &results);
                Ok(())
            }
            MessagesCmd::Read {
                reference,
                format,
                body_only,
                output,
            } => {
                let msg = fixtures_messages
                    .into_iter()
                    .find(|m| {
                        m.meta.id == reference
                            || m.meta.id.contains(&reference)
                            || m.meta
                                .subject
                                .to_lowercase()
                                .contains(&reference.to_lowercase())
                    })
                    .ok_or_else(|| {
                        Error::Other(format!("demo message not found for ref '{reference}'"))
                    })?;

                let full = msg.to_full();
                if let Some(path) = output {
                    fs::write(&path, &full.body)?;
                    if !ctx.json {
                        println!("Wrote {} bytes to {}", full.body.len(), path.display());
                    }
                } else {
                    render::full_message(ctx.json, &full, format, body_only);
                }
                Ok(())
            }
            MessagesCmd::Send(args) => {
                let id = "msg-demo-sent-101";
                if ctx.json {
                    render::sent(true, id);
                } else {
                    println!("Sent (id {id}) to {}", args.to.join(", "));
                }
                Ok(())
            }
            MessagesCmd::Reply { reference, .. } => {
                let id = format!("msg-demo-reply-{reference}");
                if ctx.json {
                    render::sent(true, &id);
                } else {
                    println!("Reply sent (id {id})");
                }
                Ok(())
            }
            MessagesCmd::Forward { reference, to, .. } => {
                let id = format!("msg-demo-fwd-{reference}");
                if ctx.json {
                    render::sent(true, &id);
                } else {
                    println!("Forwarded to {} (id {id})", to.join(", "));
                }
                Ok(())
            }
            MessagesCmd::CancelSend { reference } => {
                render::action_result(ctx.json, "cancel-send", &[reference]);
                Ok(())
            }
            MessagesCmd::Trash { references } => {
                render::action_result(ctx.json, "trash", &references);
                Ok(())
            }
            MessagesCmd::Delete { references } => {
                render::action_result(ctx.json, "delete", &references);
                Ok(())
            }
            MessagesCmd::Spam { references } => {
                render::action_result(ctx.json, "spam", &references);
                Ok(())
            }
            MessagesCmd::Ham { references } => {
                render::action_result(ctx.json, "ham", &references);
                Ok(())
            }
            MessagesCmd::Unsubscribe { reference } => {
                render::action_result(ctx.json, "unsubscribe", &[reference]);
                Ok(())
            }
            MessagesCmd::Empty { folder } => {
                render::action_result(ctx.json, &format!("empty-{folder}"), &[]);
                Ok(())
            }
            MessagesCmd::Label {
                action,
                label_id,
                references,
            } => {
                render::action_result(ctx.json, &format!("{action:?}-{label_id}"), &references);
                Ok(())
            }
            MessagesCmd::Undelete { ids } => {
                render::action_result(ctx.json, "undelete", &ids);
                Ok(())
            }
            MessagesCmd::Receipt { reference } => {
                render::action_result(ctx.json, "receipt", &[reference]);
                Ok(())
            }
            MessagesCmd::Move { references, dest } => {
                render::action_result(ctx.json, &format!("move-to-{dest}"), &references);
                Ok(())
            }
            MessagesCmd::Mark { state, references } => {
                render::action_result(ctx.json, &format!("mark-{state:?}"), &references);
                Ok(())
            }
            MessagesCmd::Star { references } => {
                render::action_result(ctx.json, "star", &references);
                Ok(())
            }
            MessagesCmd::Unstar { references } => {
                render::action_result(ctx.json, "unstar", &references);
                Ok(())
            }
        },
        Command::Conversations { cmd } => match cmd {
            ConversationsCmd::List {
                folder,
                page,
                page_size,
                unread,
            } => {
                let target_label = resolve_folder(&folder);
                let is_all = target_label == label_ids::ALL_MAIL
                    || folder.eq_ignore_ascii_case("all")
                    || folder.eq_ignore_ascii_case("allmail");

                let mut matching: Vec<Conversation> = fixtures_convs
                    .into_iter()
                    .filter(|c| {
                        is_all
                            || c.labels.iter().any(|l| {
                                l.id == target_label && l.context_num_messages.unwrap_or(0) > 0
                            })
                    })
                    .filter(|c| !unread || c.num_unread > 0)
                    .collect();

                matching.sort_by_key(|a| std::cmp::Reverse(a.time));
                let total = matching.len() as u32;
                let start = (page * page_size) as usize;
                let paged: Vec<Conversation> = matching
                    .into_iter()
                    .skip(start)
                    .take(page_size as usize)
                    .collect();

                render::conversations_list(ctx.json, total, &paged);
                Ok(())
            }
            ConversationsCmd::Search(args) => {
                let mut results: Vec<Conversation> = fixtures_convs
                    .into_iter()
                    .filter(|c| {
                        if let Some(kw) = &args.keyword
                            && !c.subject.to_lowercase().contains(&kw.to_lowercase())
                        {
                            return false;
                        }
                        if let Some(subject) = &args.subject
                            && !c.subject.to_lowercase().contains(&subject.to_lowercase())
                        {
                            return false;
                        }
                        if args.unread && c.num_unread == 0 {
                            return false;
                        }
                        true
                    })
                    .collect();

                results.sort_by_key(|a| std::cmp::Reverse(a.time));
                if let Some(limit) = args.limit {
                    results.truncate(limit as usize);
                }
                render::conversations_list(ctx.json, results.len() as u32, &results);
                Ok(())
            }
            ConversationsCmd::Read { id } => {
                let conv = fixtures_convs
                    .into_iter()
                    .find(|c| c.id == id || c.id.contains(&id))
                    .ok_or_else(|| {
                        Error::Other(format!("demo conversation not found for id '{id}'"))
                    })?;

                let conv_messages: Vec<FullMessage> = fixtures_messages
                    .into_iter()
                    .filter(|m| m.meta.conversation_id == conv.id)
                    .map(|m| m.to_full())
                    .collect();

                render::conversation_read(ctx.json, &conv, &conv_messages);
                Ok(())
            }
            ConversationsCmd::Move { ids, dest } => {
                render::action_result(ctx.json, &format!("move-to-{dest}"), &ids);
                Ok(())
            }
            ConversationsCmd::Trash { ids } => {
                render::action_result(ctx.json, "trash", &ids);
                Ok(())
            }
            ConversationsCmd::Mark {
                state,
                folder: _,
                ids,
            } => {
                render::action_result(ctx.json, &format!("mark-{state:?}"), &ids);
                Ok(())
            }
            ConversationsCmd::Star { ids } => {
                render::action_result(ctx.json, "star", &ids);
                Ok(())
            }
            ConversationsCmd::Unstar { ids } => {
                render::action_result(ctx.json, "unstar", &ids);
                Ok(())
            }
            ConversationsCmd::Snooze { ids, .. } => {
                render::action_result(ctx.json, "snooze", &ids);
                Ok(())
            }
            ConversationsCmd::Unsnooze { ids } => {
                render::action_result(ctx.json, "unsnooze", &ids);
                Ok(())
            }
        },
        Command::Attachments { cmd } => match cmd {
            AttachmentsCmd::List {
                message,
                include_inline: _,
            } => {
                let msg = fixtures_messages
                    .into_iter()
                    .find(|m| m.meta.id == message || m.meta.id.contains(&message))
                    .ok_or_else(|| {
                        Error::Other(format!("demo message not found for ref '{message}'"))
                    })?;

                render::attachments_list(ctx.json, &msg.attachments);
                Ok(())
            }
            AttachmentsCmd::Download {
                message,
                attachment,
                output_dir,
                all: _,
                include_inline: _,
            } => {
                let msg = fixtures_messages
                    .into_iter()
                    .find(|m| m.meta.id == message || m.meta.id.contains(&message))
                    .ok_or_else(|| {
                        Error::Other(format!("demo message not found for ref '{message}'"))
                    })?;

                let atts: Vec<&Attachment> = if let Some(att_id) = &attachment {
                    msg.attachments.iter().filter(|a| a.id == *att_id).collect()
                } else {
                    msg.attachments.iter().collect()
                };

                if atts.is_empty() {
                    return Err(Error::Other("no attachments found to download".into()));
                }

                let out_dir = output_dir.unwrap_or_else(|| PathBuf::from("."));
                fs::create_dir_all(&out_dir)?;

                for a in &atts {
                    let filename = safe_attachment_name(&a.name);
                    let target = out_dir.join(filename);
                    let sample_content = format!(
                        "%PDF-1.4\n% Demo simulated attachment content for {}\n",
                        a.name
                    );
                    fs::write(&target, sample_content.as_bytes())?;
                    if !ctx.json {
                        println!("Saved attachment '{}' to {}", a.name, target.display());
                    }
                }

                if ctx.json {
                    render::json_out(&json!({
                        "status": "ok",
                        "downloaded": atts.len(),
                        "directory": out_dir.display().to_string(),
                    }));
                }
                Ok(())
            }
        },
        Command::Drafts { cmd } => match cmd {
            DraftsCmd::List { page, page_size } => {
                let drafts: Vec<MessageMetadata> = fixtures_messages
                    .into_iter()
                    .filter(|m| m.meta.label_ids.contains(&label_ids::DRAFTS.to_string()))
                    .map(|m| m.meta)
                    .collect();

                let total = drafts.len() as u32;
                let start = (page * page_size) as usize;
                let paged: Vec<MessageMetadata> = drafts
                    .into_iter()
                    .skip(start)
                    .take(page_size as usize)
                    .collect();

                render::messages_list(ctx.json, total, &paged);
                Ok(())
            }
            DraftsCmd::Save(args) => {
                let id = "draft-demo-saved-01";
                if ctx.json {
                    render::sent(true, id);
                } else {
                    println!("Draft saved (id {id}) for subject: '{}'", args.subject);
                }
                Ok(())
            }
            DraftsCmd::Edit { id, args } => {
                if ctx.json {
                    render::sent(true, &id);
                } else {
                    println!("Draft updated (id {id}) for subject: '{}'", args.subject);
                }
                Ok(())
            }
            DraftsCmd::Delete { ids } => {
                render::action_result(ctx.json, "delete-draft", &ids);
                Ok(())
            }
        },
        Command::Filters { cmd } => match cmd {
            FiltersCmd::List => {
                render::filters_list(ctx.json, &demo_filters());
                Ok(())
            }
            FiltersCmd::Check { sieve } => {
                if ctx.json {
                    render::json_out(&json!({ "valid": true, "sieve": sieve }));
                } else {
                    println!("Sieve script is valid syntax.");
                }
                Ok(())
            }
            FiltersCmd::Create { name, sieve: _ } => {
                let f = Filter {
                    id: "filter-demo-02".to_string(),
                    name,
                    status: 1,
                    version: 1,
                };
                render::filter_created(ctx.json, &f);
                Ok(())
            }
            FiltersCmd::Delete { id } => {
                render::action_result(ctx.json, "delete-filter", &[id]);
                Ok(())
            }
            FiltersCmd::Enable { id } => {
                render::action_result(ctx.json, "enable-filter", &[id]);
                Ok(())
            }
            FiltersCmd::Disable { id } => {
                render::action_result(ctx.json, "disable-filter", &[id]);
                Ok(())
            }
        },
        Command::Contacts { cmd } => match cmd {
            ContactsCmd::List { page, page_size } => {
                let contacts = demo_contacts();
                let total = contacts.len() as u32;
                let start = (page * page_size) as usize;
                let paged: Vec<Contact> = contacts
                    .into_iter()
                    .skip(start)
                    .take(page_size as usize)
                    .collect();
                render::contacts_list(ctx.json, total, &paged);
                Ok(())
            }
            ContactsCmd::Emails { email } => {
                let emails: Vec<ContactEmail> = demo_contacts()
                    .into_iter()
                    .flat_map(|c| c.emails)
                    .filter(|e| {
                        if let Some(target) = &email {
                            e.email.to_lowercase().contains(&target.to_lowercase())
                        } else {
                            true
                        }
                    })
                    .collect();
                render::contact_emails(ctx.json, &emails);
                Ok(())
            }
        },
        Command::Addresses { cmd } => match cmd {
            AddressesCmd::List => {
                render::addresses_list(ctx.json, &demo_addresses());
                Ok(())
            }
            AddressesCmd::Update {
                id,
                display_name,
                signature: _,
            } => {
                if ctx.json {
                    render::json_out(&json!({ "status": "ok", "id": id }));
                } else {
                    println!(
                        "Updated address {id} with display name '{:?}'",
                        display_name
                    );
                }
                Ok(())
            }
        },
        Command::Settings { cmd } => match cmd {
            SettingsCmd::Get => {
                render::json_out(&json!({
                    "DisplayName": DEMO_NAME,
                    "Sign": 1,
                    "AttachPublicKey": 0,
                    "AutoSaveContacts": 1
                }));
                Ok(())
            }
            SettingsCmd::Sign { value } => {
                if ctx.json {
                    render::json_out(&json!({ "status": "ok", "sign": value.as_bool() }));
                } else {
                    println!("Set sign outgoing mail to: {value:?}");
                }
                Ok(())
            }
            SettingsCmd::AttachPublicKey { value } => {
                if ctx.json {
                    render::json_out(
                        &json!({ "status": "ok", "attach_public_key": value.as_bool() }),
                    );
                } else {
                    println!("Set attach public key to: {value:?}");
                }
                Ok(())
            }
        },
        Command::Counts { conversations } => {
            render::counts(ctx.json, &demo_counts(conversations));
            Ok(())
        }
        Command::Labels { cmd } => match cmd {
            LabelsCmd::List { folders } => {
                let labels: Vec<Label> = demo_labels()
                    .into_iter()
                    .filter(|l| {
                        if folders {
                            l.label_type == 3
                        } else {
                            l.label_type == 1
                        }
                    })
                    .collect();
                render::labels_list(ctx.json, &labels, folders);
                Ok(())
            }
            LabelsCmd::Create {
                name,
                color,
                folder,
                parent,
            } => {
                let l = Label {
                    id: "label-demo-created".to_string(),
                    name,
                    color,
                    path: "Custom".to_string(),
                    label_type: if folder { 3 } else { 1 },
                    parent_id: parent,
                    order: 99,
                    notify: None,
                };
                render::label_created(ctx.json, &l);
                Ok(())
            }
            LabelsCmd::Delete { ids } => {
                render::action_result(ctx.json, "delete-label", &ids);
                Ok(())
            }
            LabelsCmd::Update { id, .. } => {
                render::action_result(ctx.json, "update-label", &[id]);
                Ok(())
            }
        },
        Command::Export { folder, out, max } => {
            fs::create_dir_all(&out)?;
            let target_label = resolve_folder(&folder);
            let is_all = target_label == label_ids::ALL_MAIL || folder.eq_ignore_ascii_case("all");

            let exported: Vec<&DemoMessage> = fixtures_messages
                .iter()
                .filter(|m| is_all || m.meta.label_ids.contains(&target_label))
                .take(max as usize)
                .collect();

            for m in &exported {
                let eml_path = out.join(format!("{}.eml", m.meta.id));
                let eml_content = format!(
                    "From: {}\nTo: {}\nSubject: {}\nDate: {}\nMessage-ID: {}\n\n{}",
                    m.meta.sender.address,
                    m.meta
                        .to_list
                        .iter()
                        .map(|r| r.address.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    m.meta.subject,
                    render::fmt_time(m.meta.time),
                    m.meta.id,
                    m.body
                );
                fs::write(&eml_path, eml_content.as_bytes())?;
            }

            if ctx.json {
                render::json_out(&json!({
                    "status": "ok",
                    "exported": exported.len(),
                    "folder": folder,
                    "output_dir": out.display().to_string(),
                }));
            } else {
                println!(
                    "Exported {} demo message(s) from folder '{}' to {}",
                    exported.len(),
                    folder,
                    out.display()
                );
            }
            Ok(())
        }
        Command::Search { query, limit } => {
            let mut results: Vec<MessageMetadata> = fixtures_messages
                .into_iter()
                .filter(|m| {
                    m.meta
                        .subject
                        .to_lowercase()
                        .contains(&query.to_lowercase())
                        || m.body.to_lowercase().contains(&query.to_lowercase())
                })
                .map(|m| m.meta)
                .collect();

            results.truncate(limit as usize);
            render::messages_list(ctx.json, results.len() as u32, &results);
            Ok(())
        }
        Command::Sync { backfill, .. } => {
            if ctx.json {
                render::json_out(&json!({
                    "status": "ok",
                    "synced": true,
                    "backfill": backfill,
                    "new_events": 0
                }));
            } else {
                println!("Demo cache sync complete (0 new events, offline mode).");
            }
            Ok(())
        }
        Command::Index { folder, .. } => {
            if ctx.json {
                render::json_out(&json!({
                    "status": "ok",
                    "indexed": 8,
                    "folder": folder
                }));
            } else {
                println!("Indexed 8 demo messages for folder '{folder}'.");
            }
            Ok(())
        }
        Command::Watch { interval, folder } => {
            if ctx.json {
                render::json_out(&json!({
                    "status": "ok",
                    "watch": "demo_tick",
                    "interval": interval,
                    "folder": folder
                }));
            } else {
                println!(
                    "[demo] Simulated watch event stream (interval: {interval}s). 0 new events."
                );
            }
            Ok(())
        }
    }
}
