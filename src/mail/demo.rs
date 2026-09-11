//! Fictional in-memory mailbox for developing the UI without Proton.
//!
//! Nothing here touches the network, the keychain, a Proton session, or disk.
//! All names and addresses are invented; addresses use the reserved
//! `example.*` domains.

use std::time::{SystemTime, UNIX_EPOCH};

use super::{ConversationPage, ConversationSummary, MailFolder, MailboxCounts, MailboxError};

/// Small enough that the demo Inbox needs several pages.
pub const PAGE_SIZE: u32 = 10;

/// This folder always fails to load, to exercise the error state.
pub const FAILING_FOLDER: MailFolder = MailFolder::Spam;

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

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
        .unread(),
        mail(
            Inbox,
            "Community Garden Club",
            "Volunteer schedule for the spring planting weekend",
            2 * DAY,
        ),
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
    fn fixture_addresses_are_fictional() {
        for fixture in fixtures() {
            for word in fixture.correspondents.split([' ', ',']) {
                if let Some((_, domain)) = word.split_once('@') {
                    assert!(
                        matches!(domain, "example.com" | "example.org" | "example.net"),
                        "non-example domain in demo data: {domain}"
                    );
                }
            }
        }
    }
}
