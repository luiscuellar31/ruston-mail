//! Fictional in-memory mailbox for developing the UI without Proton.
//!
//! Nothing here touches the network, the keychain, a Proton session, or disk.
//! All names and addresses are invented; addresses use the reserved
//! `example.*` domains.

use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    ConversationDetail, ConversationPage, ConversationSummary, MailAddress, MailFolder,
    MailMessage, MailboxCounts, MailboxError, MessageBody,
};

/// Small enough that the demo Inbox needs several pages.
pub const PAGE_SIZE: u32 = 10;

/// This folder always fails to load, to exercise the error state.
pub const FAILING_FOLDER: MailFolder = MailFolder::Spam;

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// Time between consecutive messages in a generated thread.
const THREAD_GAP: i64 = 3 * HOUR + 17 * MINUTE;

const DEMO_USER_NAME: &str = "Demo User";
const DEMO_USER_ADDRESS: &str = "demo@example.com";

const PARAGRAPHS: [&str; 6] = [
    "Thanks for the update. I went through the notes and everything looks good on my side.",
    "Could we move this to next week? A couple of items are still waiting on feedback.",
    "Here is a short summary so everyone has the same picture before we meet.",
    "Sounds good to me. Let me know if anything changes and I will adjust the plan.",
    "Following up on this in case it got buried. There is no rush at all.",
    "I added a few comments inline. The main open question is the timeline for the second phase.",
];

const LONG_BODY: &str = "\
Hi Demo,

This message is intentionally long so the reader can be checked with content that does not fit on one screen.

The first part describes the background. The team spent the last few weeks collecting feedback from people who tried the early builds. Most of the comments were about navigation, a few were about wording, and a handful were about how dates are shown in different places.

The second part lists what we agreed to change. Navigation between folders should feel instant. Long names and subjects should never push other content out of view. Dates in the list should stay short, while the reader can show the complete date and time.

The third part is about timing. Nothing here is urgent, and it is fine to pick this up after the current work is finished. If something is unclear, reply to this thread and we can talk it through.

Here is a deliberately long link without spaces, to check that it wraps instead of widening the panel: https://example.com/projects/demo/reader/a-very-long-path-segment-without-any-spaces-that-must-wrap-inside-the-reading-panel?section=layout&check=wrapping

Finally, a list of small follow-ups:
- keep the spacing consistent between messages
- make the collapsed header easy to recognize
- check the layout at a narrow window width
- confirm that scrolling reaches the end of this message

Thanks for reading all the way to the end.

Morgan";

const GARDEN_VOLUNTEERS: &[&str] = &[
    "demo@example.com",
    "volunteers@example.org",
    "sam.chen@example.com",
    "priya.natarajan@example.com",
    "jordan.lee@example.com",
    "casey.kim@example.com",
    "harper.singh@example.com",
    "robin.fischer@example.com",
    "spring-planting-weekend-volunteer-coordination-list@example.org",
];

struct Fixture {
    folder: MailFolder,
    /// Senders, or recipients in Sent and Drafts. Empty means unknown.
    correspondents: &'static str,
    subject: &'static str,
    /// Seconds before now.
    age: i64,
    unread: bool,
    starred: bool,
    messages: u32,
    /// Replaces the generated recipients of every message.
    to: &'static [&'static str],
    /// Replaces the generated body of the newest message.
    body: Option<&'static str>,
}

impl Fixture {
    fn unread(self) -> Self {
        Self {
            unread: true,
            ..self
        }
    }

    fn starred(self) -> Self {
        Self {
            starred: true,
            ..self
        }
    }

    fn thread(self, messages: u32) -> Self {
        Self { messages, ..self }
    }

    fn to(self, to: &'static [&'static str]) -> Self {
        Self { to, ..self }
    }

    fn body(self, body: &'static str) -> Self {
        Self {
            body: Some(body),
            ..self
        }
    }
}

fn mail(
    folder: MailFolder,
    correspondents: &'static str,
    subject: &'static str,
    age: i64,
) -> Fixture {
    Fixture {
        folder,
        correspondents,
        subject,
        age,
        unread: false,
        starred: false,
        messages: 1,
        to: &[],
        body: None,
    }
}

fn fixtures() -> Vec<Fixture> {
    use MailFolder::{Archive, Drafts, Inbox, Sent, Spam};

    vec![
        mail(
            Inbox,
            "Alex Rivera",
            "Design review notes for the settings screen",
            12 * MINUTE,
        )
        .unread()
        .thread(3),
        mail(
            Inbox,
            "noreply@example.org",
            "Your weekly storage summary",
            48 * MINUTE,
        )
        .unread(),
        mail(
            Inbox,
            "Sam Chen, Priya Natarajan, Jordan Lee",
            "Re: Offsite agenda, please add your topics by Friday",
            2 * HOUR,
        )
        .unread()
        .starred()
        .thread(7),
        mail(Inbox, "Taylor Brooks", "", 5 * HOUR),
        mail(
            Inbox,
            "Dr. Maximiliana Konstantinopoulou-Everhart, Office of Interdepartmental Coordination",
            "Room booking confirmed",
            9 * HOUR,
        ),
        mail(
            Inbox,
            "Morgan Patel",
            "A very long subject line that keeps going to check how the conversation list \
             truncates text that does not fit in a single row of the layout",
            DAY,
        )
        .unread()
        .body(LONG_BODY),
        mail(
            Inbox,
            "Community Garden Club",
            "Volunteer schedule for the spring planting weekend",
            2 * DAY,
        )
        .to(GARDEN_VOLUNTEERS),
        mail(Inbox, "Casey Kim", "Lunch on Thursday?", 2 * DAY + 3 * HOUR).starred(),
        mail(
            Inbox,
            "billing@example.com",
            "Invoice #1042 is available",
            3 * DAY,
        ),
        mail(
            Inbox,
            "Riley Park, Alex Rivera",
            "Re: Draft proposal for the translation workflow",
            4 * DAY,
        )
        .thread(5),
        mail(Inbox, "Jamie Ortiz", "Photos from the hiking trip", 6 * DAY).starred(),
        mail(Inbox, "Build Bot", "Nightly build succeeded", 7 * DAY),
        mail(
            Inbox,
            "Avery Quinn",
            "Question about the API rate limits",
            9 * DAY,
        )
        .unread(),
        mail(
            Inbox,
            "Library Notifications",
            "Your reserved book is ready for pickup",
            12 * DAY,
        ),
        mail(
            Inbox,
            "Drew Nakamura",
            "Re: Keyboard shortcuts proposal",
            15 * DAY,
        )
        .thread(4),
        mail(
            Inbox,
            "Neighborhood Newsletter",
            "Monthly updates and upcoming events",
            18 * DAY,
        ),
        mail(
            Inbox,
            "Harper Singh",
            "The recipe you asked about",
            21 * DAY,
        ),
        mail(Inbox, "Sam Chen", "Slides from yesterday's talk", 25 * DAY),
        mail(
            Inbox,
            "Security Team",
            "New sign-in from a desktop app",
            30 * DAY,
        )
        .unread(),
        mail(
            Inbox,
            "Robin Fischer",
            "Re: Apartment viewing times",
            34 * DAY,
        )
        .thread(6),
        mail(
            Inbox,
            "Event Tickets",
            "Your ticket for the Friday concert",
            40 * DAY,
        ),
        mail(
            Inbox,
            "Parker Diaz",
            "Book club: next month's pick",
            47 * DAY,
        ),
        mail(
            Inbox,
            "",
            "Automated message without a sender name",
            55 * DAY,
        ),
        mail(Inbox, "Quinn Adler", "Old laptop for sale", 63 * DAY),
        mail(
            Inbox,
            "Travel Desk",
            "Itinerary changes for your trip",
            72 * DAY,
        )
        .starred(),
        mail(Inbox, "Emerson Cole", "Re: Garden fence repair", 80 * DAY).thread(2),
        mail(
            Inbox,
            "Language Exchange Group",
            "Meeting moved to the cafe on Elm Street",
            95 * DAY,
        ),
        mail(Inbox, "Kai Morgan", "Thanks for the help!", 110 * DAY),
        mail(Inbox, "Frankie Hale", "Podcast recommendations", 130 * DAY),
        mail(Inbox, "Support", "Your ticket has been resolved", 160 * DAY),
        mail(Inbox, "Jordan Lee", "Re: Moving boxes", 200 * DAY).thread(3),
        mail(
            Inbox,
            "Alumni Association",
            "Reunion save-the-date",
            400 * DAY,
        ),
        mail(Inbox, "Sasha Bell", "Scanned documents", 520 * DAY),
        mail(
            Inbox,
            "Old Mailing List",
            "Welcome to the mailing list",
            900 * DAY,
        ),
        mail(
            Drafts,
            "Riley Park",
            "Re: Draft proposal for the translation workflow",
            30 * MINUTE,
        ),
        mail(Drafts, "", "", 3 * DAY),
        mail(
            Sent,
            "Alex Rivera",
            "Re: Design review notes for the settings screen",
            20 * MINUTE,
        ),
        mail(
            Sent,
            "Sam Chen, Priya Natarajan, Jordan Lee",
            "Re: Offsite agenda",
            3 * HOUR,
        ),
        mail(Sent, "Casey Kim", "Re: Lunch on Thursday?", 2 * DAY),
        mail(Sent, "Harper Singh", "Recipe request", 22 * DAY),
        mail(
            Sent,
            "landlord@example.net",
            "Lease renewal questions",
            45 * DAY,
        ),
        mail(
            Archive,
            "Project Updates",
            "First quarter retrospective summary",
            150 * DAY,
        )
        .starred(),
        mail(Archive, "Alex Rivera", "Onboarding checklist", 300 * DAY),
        mail(Archive, "Insurance Office", "Policy documents", 450 * DAY).unread(),
        mail(Archive, "Taylor Brooks", "Re: Conference travel", 600 * DAY).thread(4),
        mail(
            Archive,
            "newsletter@example.org",
            "Year in review",
            700 * DAY,
        ),
        mail(Spam, "Prize Department", "You may have won something", DAY).unread(),
        mail(
            Spam,
            "unknown@example.net",
            "Urgent account notice",
            4 * DAY,
        ),
    ]
}

fn in_folder(fixture: &Fixture, folder: MailFolder) -> bool {
    fixture.folder == folder || (folder == MailFolder::Starred && fixture.starred)
}

/// One zero-based page of a folder, newest first, relative to `now` (Unix seconds).
pub fn list_conversations(
    folder: MailFolder,
    page: u32,
    page_size: u32,
    now: i64,
) -> Result<ConversationPage, MailboxError> {
    if folder == FAILING_FOLDER {
        return Err(MailboxError::Unavailable);
    }

    let fixtures = fixtures();
    let mut matching: Vec<_> = fixtures
        .iter()
        .enumerate()
        .filter(|(_, fixture)| in_folder(fixture, folder))
        .collect();
    matching.sort_by_key(|(_, fixture)| fixture.age);

    let total = matching.len() as u32;
    let page_size = page_size as usize;
    let conversations = matching
        .into_iter()
        .skip(page as usize * page_size)
        .take(page_size)
        .map(|(index, fixture)| summary(index, fixture, now))
        .collect();

    Ok(ConversationPage {
        conversations,
        total,
    })
}

pub fn counts() -> MailboxCounts {
    let fixtures = fixtures();

    MailFolder::ALL
        .into_iter()
        .map(|folder| {
            let unread = fixtures
                .iter()
                .filter(|fixture| fixture.unread && in_folder(fixture, folder))
                .count();
            (folder, unread as u32)
        })
        .collect()
}

/// The full thread of a demo conversation, oldest message first, generated
/// deterministically from its fixture.
pub fn conversation_detail(id: &str, now: i64) -> Option<ConversationDetail> {
    let index: usize = id.strip_prefix("demo-")?.parse().ok()?;
    let fixture = fixtures().into_iter().nth(index)?;
    let participants: Vec<MailAddress> = fixture
        .correspondents
        .split(", ")
        .filter(|participant| !participant.is_empty())
        .map(participant_address)
        .collect();
    let count = fixture.messages as usize;
    let messages = (0..count)
        .map(|position| message(&fixture, &participants, index, position, count, now))
        .collect();

    Some(ConversationDetail {
        id: id.to_owned(),
        subject: non_empty(fixture.subject),
        messages,
    })
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

fn summary(index: usize, fixture: &Fixture, now: i64) -> ConversationSummary {
    ConversationSummary {
        id: format!("demo-{index}"),
        subject: non_empty(fixture.subject),
        correspondents: non_empty(fixture.correspondents),
        time: Some(now - fixture.age),
        unread: fixture.unread,
        starred: fixture.starred,
        message_count: fixture.messages,
    }
}

fn message(
    fixture: &Fixture,
    participants: &[MailAddress],
    index: usize,
    position: usize,
    count: usize,
    now: i64,
) -> MailMessage {
    let newer = count - 1 - position;
    // Messages alternate between the demo user and the other participants;
    // the newest one is the demo user's only in Sent and Drafts.
    let outgoing = matches!(fixture.folder, MailFolder::Sent | MailFolder::Drafts);
    let from_demo_user = newer.is_multiple_of(2) == outgoing;

    let (sender, mut recipients) = if from_demo_user {
        (demo_user(), participants.to_vec())
    } else {
        let sender = if participants.is_empty() {
            MailAddress::default()
        } else {
            participants[(position / 2) % participants.len()].clone()
        };
        let recipients = std::iter::once(demo_user())
            .chain(
                participants
                    .iter()
                    .filter(|participant| **participant != sender)
                    .cloned(),
            )
            .collect();
        (sender, recipients)
    };
    if !fixture.to.is_empty() {
        recipients = fixture
            .to
            .iter()
            .map(|address| participant_address(address))
            .collect();
    }

    let body = match fixture.body {
        Some(body) if newer == 0 => body.to_owned(),
        _ => generated_body(index * 7 + position, &sender, &recipients),
    };

    MailMessage {
        id: format!("demo-{index}-{position}"),
        sender,
        recipients,
        time: Some(now - fixture.age - newer as i64 * THREAD_GAP),
        body: MessageBody::PlainText(body),
    }
}

fn demo_user() -> MailAddress {
    MailAddress {
        name: Some(DEMO_USER_NAME.to_owned()),
        address: DEMO_USER_ADDRESS.to_owned(),
    }
}

/// A bare address has no display name; a name gets an invented
/// `first.last@example.com` address.
fn participant_address(participant: &str) -> MailAddress {
    if participant.contains('@') {
        return MailAddress {
            name: None,
            address: participant.to_owned(),
        };
    }

    let local_part = participant
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(".");

    MailAddress {
        name: Some(participant.to_owned()),
        address: format!("{local_part}@example.com"),
    }
}

fn generated_body(seed: usize, sender: &MailAddress, recipients: &[MailAddress]) -> String {
    let greeting = match recipients.first().and_then(first_name) {
        Some(name) => format!("Hi {name},"),
        None => "Hello,".to_owned(),
    };
    let first = PARAGRAPHS[seed % PARAGRAPHS.len()];
    let second = PARAGRAPHS[(seed + 2) % PARAGRAPHS.len()];

    let mut body = format!("{greeting}\n\n{first}\n\n{second}");
    if let Some(name) = first_name(sender) {
        body.push_str("\n\nThanks,\n");
        body.push_str(name);
    }
    body
}

/// The first word of a display name, skipping titles such as "Dr.".
fn first_name(address: &MailAddress) -> Option<&str> {
    address
        .name
        .as_deref()?
        .split_whitespace()
        .find(|word| !word.ends_with('.'))
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_789_000_000;

    fn page(folder: MailFolder, page: u32) -> ConversationPage {
        list_conversations(folder, page, PAGE_SIZE, NOW).unwrap()
    }

    fn detail(index: usize) -> ConversationDetail {
        conversation_detail(&format!("demo-{index}"), NOW).unwrap()
    }

    #[test]
    fn counts_are_deterministic() {
        let counts = counts();

        assert_eq!(counts, super::counts());
        assert_eq!(counts.unread(MailFolder::Inbox), Some(6));
        assert_eq!(counts.unread(MailFolder::Starred), Some(1));
        assert_eq!(counts.unread(MailFolder::Archive), Some(1));
        assert_eq!(counts.unread(MailFolder::Sent), Some(0));
        assert_eq!(counts.unread(MailFolder::Trash), Some(0));
    }

    #[test]
    fn first_inbox_page_is_deterministic_and_newest_first() {
        let first = page(MailFolder::Inbox, 0);

        assert_eq!(first.total, 34);
        assert_eq!(first.conversations.len(), 10);
        assert_eq!(first.conversations[0].id, "demo-0");
        assert_eq!(first.conversations[0].time, Some(NOW - 12 * MINUTE));
        assert!(
            first
                .conversations
                .windows(2)
                .all(|pair| pair[0].time >= pair[1].time)
        );
        assert_eq!(
            first.conversations,
            page(MailFolder::Inbox, 0).conversations
        );
    }

    #[test]
    fn pages_continue_and_end() {
        let second = page(MailFolder::Inbox, 1);
        let last = page(MailFolder::Inbox, 3);

        assert_eq!(second.conversations[0].id, "demo-10");
        assert_eq!(last.conversations.len(), 4);
        assert!(page(MailFolder::Inbox, 4).conversations.is_empty());
    }

    #[test]
    fn trash_is_empty_and_spam_fails() {
        let trash = page(MailFolder::Trash, 0);

        assert_eq!(trash.total, 0);
        assert!(trash.conversations.is_empty());
        assert_eq!(
            list_conversations(MailFolder::Spam, 0, PAGE_SIZE, NOW).unwrap_err(),
            MailboxError::Unavailable
        );
    }

    #[test]
    fn starred_collects_starred_conversations_from_other_folders() {
        let starred = page(MailFolder::Starred, 0);

        assert_eq!(starred.total, 5);
        assert!(starred.conversations.iter().all(|c| c.starred));
    }

    #[test]
    fn fixtures_cover_edge_cases() {
        let inbox: Vec<_> = (0..4)
            .flat_map(|index| page(MailFolder::Inbox, index).conversations)
            .collect();

        assert!(inbox.iter().any(|c| c.subject.is_none()));
        assert!(inbox.iter().any(|c| c.correspondents.is_none()));
        assert!(inbox.iter().any(|c| c.message_count > 1));
        assert!(inbox.iter().any(|c| c.unread) && inbox.iter().any(|c| !c.unread));
    }

    #[test]
    fn every_conversation_has_a_matching_detail() {
        for (index, fixture) in fixtures().iter().enumerate() {
            let detail = detail(index);

            assert_eq!(detail.id, format!("demo-{index}"));
            assert_eq!(detail.messages.len(), fixture.messages as usize);
            assert!(
                detail
                    .messages
                    .windows(2)
                    .all(|pair| pair[0].time < pair[1].time)
            );
            assert_eq!(detail, super::conversation_detail(&detail.id, NOW).unwrap());
        }
        assert_eq!(conversation_detail("demo-999", NOW), None);
        assert_eq!(conversation_detail("other", NOW), None);
    }

    #[test]
    fn newest_message_matches_the_list_time() {
        let detail = detail(0);

        assert_eq!(
            detail.messages.last().unwrap().time,
            Some(NOW - 12 * MINUTE)
        );
    }

    #[test]
    fn details_cover_reader_edge_cases() {
        let noreply = &detail(1).messages[0];
        assert_eq!(noreply.sender.name, None);
        assert_eq!(noreply.sender.display_name(), Some("noreply@example.org"));

        let unknown = &detail(22).messages[0];
        assert_eq!(unknown.sender.display_name(), None);

        let garden = &detail(6).messages[0];
        assert_eq!(garden.recipients.len(), GARDEN_VOLUNTEERS.len());

        let long = &detail(5).messages[0];
        assert_eq!(long.body, MessageBody::PlainText(LONG_BODY.to_owned()));

        assert_eq!(detail(3).subject, None);
    }

    #[test]
    fn threads_alternate_with_the_demo_user() {
        let inbox = detail(0);
        let sent = detail(36);

        assert_eq!(
            inbox.messages[2].sender.name.as_deref(),
            Some("Alex Rivera")
        );
        assert_eq!(inbox.messages[1].sender.address, DEMO_USER_ADDRESS);
        assert_eq!(sent.messages[0].sender.address, DEMO_USER_ADDRESS);
    }

    #[test]
    fn fixture_addresses_are_fictional() {
        for (index, _) in fixtures().iter().enumerate() {
            for message in detail(index).messages {
                for address in std::iter::once(&message.sender).chain(&message.recipients) {
                    if let Some((_, domain)) = address.address.rsplit_once('@') {
                        assert!(
                            matches!(domain, "example.com" | "example.org" | "example.net"),
                            "non-example domain in demo data: {domain}"
                        );
                    }
                }
            }
        }
    }
}
