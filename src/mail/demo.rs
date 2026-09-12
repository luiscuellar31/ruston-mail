//! Fictional in-memory mailbox for developing the UI without Proton.
//!
//! Nothing here touches the network, the keychain, a Proton session, or disk.
//! All names and addresses are invented; addresses use the reserved
//! `example.*` domains.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    ConversationDetail, ConversationPage, ConversationSummary, Folder, MailAddress, MailFolder,
    MailMessage, MailboxCounts, MailboxError, MessageBody, SummaryKind,
};

/// Small enough that the demo Inbox needs several pages.
pub const PAGE_SIZE: u32 = 10;

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// Time between consecutive messages in a generated thread.
const THREAD_GAP: i64 = 3 * HOUR + 17 * MINUTE;

const DEMO_USER_NAME: &str = "Demo User";
const DEMO_USER_ADDRESS: &str = "demo@example.com";

const DESIGN_REVIEW_THREAD: [&str; 3] = [
    "Hi Demo,\n\nI reviewed the settings screen. The account section reads clearly, but the notification toggles need stronger labels. I left comments on the latest mockup.\n\nCould you update those labels before tomorrow's review?\n\nThanks,\nAlex",
    "Hi Alex,\n\nI updated the notification labels and grouped the recovery options under Security. I also added helper text for the two settings that were ambiguous.\n\nThe revised mockup is ready for another pass.\n\nThanks,\nDemo",
    "Hi Demo,\n\nThe revised settings screen looks good. The new labels remove the ambiguity, and the Security grouping feels natural.\n\nI marked the review complete. No further changes from me.\n\nThanks,\nAlex",
];

const OFFSITE_THREAD: [&str; 7] = [
    "Hi everyone,\n\nPlease add your topics for the offsite by Friday. I have reserved the morning for planning and the afternoon for team discussions.\n\nThanks,\nSam",
    "Hi Sam,\n\nPlease add a 30-minute onboarding retrospective. I would like to capture what helped recent hires and what we should change.\n\nThanks,\nDemo",
    "Hi all,\n\nI would like 45 minutes for customer interview themes. I can bring three recordings and a short summary of recurring feedback.\n\nPriya",
    "Hi Priya,\n\nI added the customer interview session after lunch. The agenda still has a 30-minute open slot before the closing discussion.\n\nThanks,\nDemo",
    "Hi everyone,\n\nCould we use the open slot for next quarter's technical risks? I can facilitate and send three prompts beforehand.\n\nJordan",
    "Hi Jordan,\n\nAdded. I also moved the break forward by 15 minutes so the afternoon sessions do not run together.\n\nThanks,\nDemo",
    "Hi all,\n\nThe agenda is now final. We will meet in Cedar Room at 09:00; breakfast and coffee arrive at 08:45.\n\nSee you Friday,\nSam",
];

const TRANSLATION_THREAD: [&str; 5] = [
    "Hi Demo,\n\nI attached the first proposal for the translation workflow. It covers string extraction, reviewer assignment, and the release cutoff.\n\nCould you check the engineering steps?\n\nThanks,\nRiley",
    "Hi Riley,\n\nThe flow looks workable. I added comments about plural forms and requested that screenshots include the string key.\n\nThanks,\nDemo",
    "Hi Demo,\n\nI reviewed the proposal too. Please add a step for documenting character limits before translators begin. That should prevent late layout fixes.\n\nAlex",
    "Hi Alex,\n\nGood point. I added character limits to the handoff checklist and clarified who approves exceptions.\n\nThanks,\nDemo",
    "Hi both,\n\nI incorporated the comments and marked the remaining decisions. The proposal is ready for Friday's workflow review.\n\nThanks,\nRiley",
];

const KEYBOARD_SHORTCUTS_THREAD: [&str; 4] = [
    "Hi Drew,\n\nI drafted shortcuts for archive, delete, reply, and folder navigation. I avoided single-letter shortcuts while the cursor is inside a text field.\n\nCould you review the key choices?\n\nThanks,\nDemo",
    "Hi Demo,\n\nThe list is a good start. I suggest using bracket keys for conversation navigation and keeping J/K available for the message list.\n\nDrew",
    "Hi Drew,\n\nI revised the proposal and added a conflict table for macOS, Windows, and Linux. J/K now moves through the list as suggested.\n\nThanks,\nDemo",
    "Hi Demo,\n\nThe revised shortcuts look consistent across platforms. I approved the proposal and opened one follow-up for screen-reader announcements.\n\nThanks,\nDrew",
];

const APARTMENT_VIEWING_THREAD: [&str; 6] = [
    "Hi Robin,\n\nI am interested in the apartment on Oak Avenue. Do you have any viewing times after 17:00 this week?\n\nThanks,\nDemo",
    "Hi Demo,\n\nI can show it Tuesday at 17:30 or Thursday at 18:15. The viewing takes about 30 minutes.\n\nBest,\nRobin",
    "Hi Robin,\n\nTuesday at 17:30 works for me. Is there bicycle parking in the building?\n\nThanks,\nDemo",
    "Hi Demo,\n\nYes, there is a locked bicycle room beside the rear entrance. Please bring a photo ID for the front desk.\n\nRobin",
    "Hi Robin,\n\nGreat, I will bring my ID. I expect to arrive a few minutes early.\n\nThanks,\nDemo",
    "Hi Demo,\n\nYou are confirmed for Tuesday at 17:30. Use the north entrance and ask the front desk for apartment 4B.\n\nBest,\nRobin",
];

const GARDEN_FENCE_THREAD: [&str; 2] = [
    "Hi Emerson,\n\nTwo boards on the shared garden fence came loose in last night's wind. Could you look at them this weekend and estimate the repair?\n\nThanks,\nDemo",
    "Hi Demo,\n\nI checked the fence this morning. The posts are sound, so we only need three new boards and exterior screws. I can repair it Saturday after 10:00.\n\nEmerson",
];

const MOVING_BOXES_THREAD: [&str; 3] = [
    "Hi Demo,\n\nI saved twelve medium moving boxes and a roll of packing paper for you. They are folded in my garage.\n\nJordan",
    "Hi Jordan,\n\nThat is perfect. Could I pick them up Wednesday around 18:30?\n\nThanks,\nDemo",
    "Hi Demo,\n\nWednesday at 18:30 works. I will leave the garage light on and bring the boxes to the driveway.\n\nJordan",
];

const CONFERENCE_TRAVEL_THREAD: [&str; 4] = [
    "Hi Taylor,\n\nI found two train options for the conference. The 08:10 service arrives before registration and costs less than the later express.\n\nWhich one do you prefer?\n\nThanks,\nDemo",
    "Hi Demo,\n\nPlease book the 08:10 train. I will travel with one carry-on bag and do not need a flexible ticket.\n\nTaylor",
    "Hi Taylor,\n\nBooked. Your outbound seat is 12A, and the return train leaves at 18:40 on Friday.\n\nThanks,\nDemo",
    "Hi Demo,\n\nI received the tickets and added both trips to my calendar. Thanks for arranging everything.\n\nTaylor",
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
    /// Whether the demo user sent the original conversation.
    outgoing: bool,
    /// Senders, or recipients in Sent and Drafts. Empty means unknown.
    correspondents: &'static str,
    subject: &'static str,
    /// Seconds before now.
    age: i64,
    unread: bool,
    starred: bool,
    /// Message bodies, oldest first. The length is the conversation count.
    bodies: Vec<&'static str>,
    /// Replaces the generated recipients of every message.
    to: &'static [&'static str],
}

/// Process-local mutable state for the offline mailbox.
#[derive(Clone)]
pub struct DemoMailbox {
    fixtures: Arc<Mutex<Vec<Fixture>>>,
}

impl Default for DemoMailbox {
    fn default() -> Self {
        Self::new()
    }
}

impl DemoMailbox {
    pub fn new() -> Self {
        Self {
            fixtures: Arc::new(Mutex::new(fixtures())),
        }
    }

    pub fn list_conversations(
        &self,
        folder: &Folder,
        page: u32,
        page_size: u32,
        now: i64,
    ) -> Result<ConversationPage, MailboxError> {
        let fixtures = self.fixtures.lock().expect("demo mailbox lock poisoned");
        Ok(list_from(&fixtures, folder, page, page_size, now))
    }

    /// Searches every folder, the way the server does for a real account.
    pub fn search(
        &self,
        query: &str,
        limit: u32,
        now: i64,
    ) -> Result<ConversationPage, MailboxError> {
        let fixtures = self.fixtures.lock().expect("demo mailbox lock poisoned");
        Ok(search_from(&fixtures, query, limit, now))
    }

    pub fn counts(&self) -> MailboxCounts {
        let fixtures = self.fixtures.lock().expect("demo mailbox lock poisoned");
        counts_from(&fixtures)
    }

    pub fn conversation_detail(
        &self,
        id: &str,
        now: i64,
    ) -> Result<ConversationDetail, MailboxError> {
        let fixtures = self.fixtures.lock().expect("demo mailbox lock poisoned");
        detail_from(&fixtures, id, now).ok_or(MailboxError::Unavailable)
    }

    pub fn snapshot(
        &self,
        folder: &Folder,
        page_size: u32,
        now: i64,
    ) -> (ConversationPage, MailboxCounts) {
        let fixtures = self.fixtures.lock().expect("demo mailbox lock poisoned");
        (
            list_from(&fixtures, folder, 0, page_size, now),
            counts_from(&fixtures),
        )
    }

    pub fn set_unread(&self, id: &str, unread: bool) -> bool {
        self.update(id, |fixture| fixture.unread = unread)
    }

    pub fn set_starred(&self, id: &str, starred: bool) -> bool {
        self.update(id, |fixture| fixture.starred = starred)
    }

    /// Moves a row between Proton's own folders. The fictional mailbox has no
    /// folders of its own, so a move into one changes nothing.
    pub fn move_to(&self, id: &str, folder: &Folder) -> bool {
        let Some(folder) = folder.system() else {
            return false;
        };

        self.update(id, |fixture| fixture.folder = folder)
    }

    fn update(&self, id: &str, update: impl FnOnce(&mut Fixture)) -> bool {
        let Some(index) = fixture_index(id) else {
            return false;
        };
        let mut fixtures = self.fixtures.lock().expect("demo mailbox lock poisoned");
        let Some(fixture) = fixtures.get_mut(index) else {
            return false;
        };

        update(fixture);
        true
    }
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

    fn thread(self, bodies: &[&'static str]) -> Self {
        Self {
            bodies: bodies.to_vec(),
            ..self
        }
    }

    fn to(self, to: &'static [&'static str]) -> Self {
        Self { to, ..self }
    }

    fn message_count(&self) -> usize {
        self.bodies.len()
    }
}

fn mail(
    folder: MailFolder,
    correspondents: &'static str,
    subject: &'static str,
    age: i64,
    body: &'static str,
) -> Fixture {
    Fixture {
        folder,
        outgoing: matches!(folder, MailFolder::Sent | MailFolder::Drafts),
        correspondents,
        subject,
        age,
        unread: false,
        starred: false,
        bodies: vec![body],
        to: &[],
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
            DESIGN_REVIEW_THREAD[0],
        )
        .unread()
        .thread(&DESIGN_REVIEW_THREAD),
        mail(
            Inbox,
            "noreply@example.org",
            "Your weekly storage summary",
            48 * MINUTE,
            "You are using 1.8 GB of your 5 GB mailbox storage. That is 36% of your current plan.\n\nArchive or delete large messages to free space. No action is required at this time.",
        )
        .unread(),
        mail(
            Inbox,
            "Sam Chen, Priya Natarajan, Jordan Lee",
            "Re: Offsite agenda, please add your topics by Friday",
            2 * HOUR,
            OFFSITE_THREAD[0],
        )
        .unread()
        .starred()
        .thread(&OFFSITE_THREAD),
        mail(
            Inbox,
            "Taylor Brooks",
            "",
            5 * HOUR,
            "Hi Demo,\n\nYou left your blue notebook in the meeting room. I put it in the top drawer beside the printer so it does not get lost.\n\nTaylor",
        ),
        mail(
            Inbox,
            "Dr. Maximiliana Konstantinopoulou-Everhart, Office of Interdepartmental Coordination",
            "Room booking confirmed",
            9 * HOUR,
            "Hello Demo,\n\nCedar Room is confirmed for September 18 from 09:00 to 12:00. The room seats 18 people and includes a display, speakerphone, and whiteboard.\n\nPlease cancel by noon the previous day if your plans change.\n\nRegards,\nMaximiliana",
        ),
        mail(
            Inbox,
            "Morgan Patel",
            "A very long subject line that keeps going to check how the conversation list \
             truncates text that does not fit in a single row of the layout",
            DAY,
            LONG_BODY,
        )
        .unread(),
        mail(
            Inbox,
            "Community Garden Club",
            "Volunteer schedule for the spring planting weekend",
            2 * DAY,
            "Hello volunteers,\n\nThe spring planting weekend runs Saturday from 08:30 to 13:00. Meet beside the tool shed for assignments.\n\nTeams will prepare beds, spread compost, plant seedlings, and repair irrigation lines. Bring gloves, a water bottle, and clothes suitable for light rain.\n\nReply by Thursday if your availability changed.\n\nCommunity Garden Club",
        )
        .to(GARDEN_VOLUNTEERS),
        mail(
            Inbox,
            "Casey Kim",
            "Lunch on Thursday?",
            2 * DAY + 3 * HOUR,
            "Hi Demo,\n\nAre you free for lunch Thursday at 12:30? The new noodle place near the station has outdoor tables.\n\nCasey",
        )
        .starred(),
        mail(
            Inbox,
            "billing@example.com",
            "Invoice #1042 is available",
            3 * DAY,
            "Hello,\n\nInvoice #1042 for $48.00 is ready. Payment is due September 25.\n\nThis invoice covers your September workspace subscription. Sign in to your account to view the itemized invoice or update your payment method.",
        ),
        mail(
            Inbox,
            "Riley Park, Alex Rivera",
            "Re: Draft proposal for the translation workflow",
            4 * DAY,
            TRANSLATION_THREAD[0],
        )
        .thread(&TRANSLATION_THREAD),
        mail(
            Inbox,
            "Jamie Ortiz",
            "Photos from the hiking trip",
            6 * DAY,
            "Hi Demo,\n\nI uploaded the photos from Saturday's hike. The folder includes the ridge viewpoint, the old fire tower, and our very muddy finish.\n\nMy favorite is IMG_1842. Feel free to add yours to the same album.\n\nJamie",
        )
        .starred(),
        mail(
            Inbox,
            "Build Bot",
            "Nightly build succeeded",
            7 * DAY,
            "Nightly build 1847 completed successfully.\n\nTarget: main\nCommit: 8f31c2a\nTests: 76 passed\nWarnings: 0\nDuration: 4m 12s\n\nArtifacts remain available for 14 days.",
        ),
        mail(
            Inbox,
            "Avery Quinn",
            "Question about the API rate limits",
            9 * DAY,
            "Hi Demo,\n\nI am integrating the reports endpoint and want to confirm the rate limit. Does the limit apply per API token or across the whole account?\n\nWe expect short bursts of about 20 requests when a dashboard opens, followed by one refresh every five minutes.\n\nThanks,\nAvery",
        )
        .unread(),
        mail(
            Inbox,
            "Library Notifications",
            "Your reserved book is ready for pickup",
            12 * DAY,
            "Your hold is ready for pickup.\n\nTitle: The Left Hand of Darkness\nPickup location: Central Library\nHold shelf: C-17\nCollect by: September 16\n\nBring your library card or photo ID. The hold will return to circulation after the collection date.",
        ),
        mail(
            Inbox,
            "Drew Nakamura",
            "Re: Keyboard shortcuts proposal",
            15 * DAY,
            KEYBOARD_SHORTCUTS_THREAD[0],
        )
        .thread(&KEYBOARD_SHORTCUTS_THREAD),
        mail(
            Inbox,
            "Neighborhood Newsletter",
            "Monthly updates and upcoming events",
            18 * DAY,
            "Hello neighbors,\n\nThis month, crews will resurface Pine Street from September 21 to 23. Street parking will be unavailable between 07:00 and 17:00.\n\nThe autumn swap meet is Saturday, September 26, in the community hall. Table reservations close next Monday.\n\nThe next neighborhood meeting is October 2 at 19:00.",
        ),
        mail(
            Inbox,
            "Harper Singh",
            "The recipe you asked about",
            21 * DAY,
            "Hi Demo,\n\nHere is the lentil soup recipe. Cook one diced onion with cumin, add red lentils, tomatoes, and vegetable stock, then simmer for 25 minutes.\n\nFinish with lemon juice and chopped parsley. I usually add smoked paprika when the tomatoes are mild.\n\nHarper",
        ),
        mail(
            Inbox,
            "Sam Chen",
            "Slides from yesterday's talk",
            25 * DAY,
            "Hi Demo,\n\nI attached the slides from yesterday's accessibility talk. Slides 18 through 24 contain the keyboard testing checklist we discussed afterward.\n\nThe speaker also shared the contrast tool here: https://example.org/tools/contrast-checker\n\nSam",
        ),
        mail(
            Inbox,
            "Security Team",
            "New sign-in from a desktop app",
            30 * DAY,
            "A new sign-in to your account was detected.\n\nApplication: Ruston desktop\nPlatform: macOS\nApproximate location: Mexico City, Mexico\nTime: August 12 at 14:36\n\nIf this was you, no action is needed. If you do not recognize this sign-in, revoke the session and change your password.",
        )
        .unread(),
        mail(
            Inbox,
            "Robin Fischer",
            "Re: Apartment viewing times",
            34 * DAY,
            APARTMENT_VIEWING_THREAD[0],
        )
        .thread(&APARTMENT_VIEWING_THREAD),
        mail(
            Inbox,
            "Event Tickets",
            "Your ticket for the Friday concert",
            40 * DAY,
            "Your mobile ticket is ready.\n\nEvent: The Paper Satellites\nDate: Friday, August 7 at 20:00\nVenue: North Hall\nSection: Floor\nEntry: Door B\n\nOpen the ticket before arriving. Screenshots cannot refresh if the event code changes.",
        ),
        mail(
            Inbox,
            "Parker Diaz",
            "Book club: next month's pick",
            47 * DAY,
            "Hi everyone,\n\nNext month's book is Piranesi by Susanna Clarke. We will meet September 3 at 19:00 in the upstairs room at Page & Cup.\n\nI will send three discussion questions the week before.\n\nParker",
        ),
        mail(
            Inbox,
            "",
            "Automated message without a sender name",
            55 * DAY,
            "This automated message intentionally has no sender display name.\n\nReference: DEMO-2048\nStatus: Delivery complete\nRecorded at: 10:42 UTC\n\nNo reply is monitored for this address.",
        ),
        mail(
            Inbox,
            "Quinn Adler",
            "Old laptop for sale",
            63 * DAY,
            "Hi Demo,\n\nI am selling my 13-inch laptop from 2021. It has 16 GB of memory, a 512 GB drive, and a fresh battery replacement.\n\nI am asking $420 and can bring it to the office if you want to inspect it.\n\nQuinn",
        ),
        mail(
            Inbox,
            "Travel Desk",
            "Itinerary changes for your trip",
            72 * DAY,
            "Hello Demo,\n\nYour outbound flight now departs at 16:20, 55 minutes later than scheduled. The flight number and arrival terminal are unchanged.\n\nYour airport transfer has been moved to 13:15. The updated itinerary is attached.\n\nTravel Desk",
        )
        .starred(),
        mail(
            Inbox,
            "Emerson Cole",
            "Re: Garden fence repair",
            80 * DAY,
            GARDEN_FENCE_THREAD[0],
        )
        .thread(&GARDEN_FENCE_THREAD),
        mail(
            Inbox,
            "Language Exchange Group",
            "Meeting moved to the cafe on Elm Street",
            95 * DAY,
            "Hello everyone,\n\nTuesday's language exchange has moved to Elm Street Cafe because the library closes early. We will meet at 18:30 at the large table upstairs.\n\nThis week's themes are travel problems and asking for directions. New participants are welcome.",
        ),
        mail(
            Inbox,
            "Kai Morgan",
            "Thanks for the help!",
            110 * DAY,
            "Hi Demo,\n\nThanks for helping me recover the presentation before the client call. The exported copy opened correctly, and we finished with time to spare.\n\nI owe you coffee.\n\nKai",
        ),
        mail(
            Inbox,
            "Frankie Hale",
            "Podcast recommendations",
            130 * DAY,
            "Hi Demo,\n\nYou asked for podcasts for long walks. Try Articles of Interest for design stories, Cautionary Tales for history, and Twenty Thousand Hertz for sound.\n\nStart with the episodes about pockets, the cobra effect, and notification sounds.\n\nFrankie",
        ),
        mail(
            Inbox,
            "Support",
            "Your ticket has been resolved",
            160 * DAY,
            "Hello Demo,\n\nSupport ticket #5821 has been resolved. We corrected the duplicate charge and returned $12.00 to your original payment method.\n\nThe credit may take three to five business days to appear. Reply within seven days if the issue remains.\n\nSupport Team",
        ),
        mail(
            Inbox,
            "Jordan Lee",
            "Re: Moving boxes",
            200 * DAY,
            MOVING_BOXES_THREAD[0],
        )
        .thread(&MOVING_BOXES_THREAD),
        mail(
            Inbox,
            "Alumni Association",
            "Reunion save-the-date",
            400 * DAY,
            "Save the date for the Class of 2016 reunion.\n\nSaturday, November 14\n18:00 to 22:00\nRiverside Alumni Hall\n\nRegistration and hotel details will follow in August. Please update your contact information before June 30.",
        ),
        mail(
            Inbox,
            "Sasha Bell",
            "Scanned documents",
            520 * DAY,
            "Hi Demo,\n\nI scanned the signed lease addendum and the two receipts from the repair shop. The PDF has six pages in the same order as the originals.\n\nLet me know if you need a higher-resolution copy.\n\nSasha",
        ),
        mail(
            Inbox,
            "Old Mailing List",
            "Welcome to the mailing list",
            900 * DAY,
            "Welcome to the Riverside Makers mailing list.\n\nYou will receive workshop announcements, monthly meeting notes, and occasional requests for volunteers. Messages are usually sent twice a month.\n\nTo introduce yourself, reply with the projects or tools you are interested in.",
        ),
        mail(
            Drafts,
            "Riley Park",
            "Re: Draft proposal for the translation workflow",
            30 * MINUTE,
            "Hi Riley,\n\nI reviewed the latest translation workflow. The ownership section is clear now. I still want to discuss how emergency string changes enter the queue after the release cutoff.\n\nCould we cover that in Friday's review?\n\nThanks,\nDemo",
        ),
        mail(
            Drafts,
            "",
            "",
            3 * DAY,
            "Notes for next week:\n\n- confirm the dentist appointment\n- send the utility reading\n- ask Casey about Saturday\n\nThis unfinished draft intentionally has no subject or recipient.",
        ),
        mail(
            Sent,
            "Alex Rivera",
            "Re: Design review notes for the settings screen",
            20 * MINUTE,
            "Hi Alex,\n\nI made the final label changes and uploaded the approved settings mockup. The handoff includes focus states and notes for narrow windows.\n\nThanks for the careful review.\n\nDemo",
        ),
        mail(
            Sent,
            "Sam Chen, Priya Natarajan, Jordan Lee",
            "Re: Offsite agenda",
            3 * HOUR,
            "Hi everyone,\n\nI added the onboarding retrospective, customer interview themes, and technical risks to the offsite agenda. The final schedule is attached.\n\nPlease send dietary changes by Wednesday noon.\n\nThanks,\nDemo",
        ),
        mail(
            Sent,
            "Casey Kim",
            "Re: Lunch on Thursday?",
            2 * DAY,
            "Hi Casey,\n\nThursday at 12:30 works. I will meet you outside the noodle place near the station.\n\nSee you then,\nDemo",
        ),
        mail(
            Sent,
            "Harper Singh",
            "Recipe request",
            22 * DAY,
            "Hi Harper,\n\nCould you send me the lentil soup recipe you made last weekend? I especially want to know which spices you used.\n\nThanks,\nDemo",
        ),
        mail(
            Sent,
            "landlord@example.net",
            "Lease renewal questions",
            45 * DAY,
            "Hello,\n\nI received the lease renewal offer and have two questions before signing. Does the new rent include the storage unit, and can the renewal begin on October 1 instead of September 15?\n\nThank you,\nDemo",
        ),
        mail(
            Archive,
            "Project Updates",
            "First quarter retrospective summary",
            150 * DAY,
            "Hello team,\n\nThe first quarter retrospective is complete. We shipped three planned releases, reduced average review time from 2.4 days to 1.6 days, and closed 41 customer issues.\n\nThe main follow-ups are smaller release batches, earlier accessibility checks, and clearer ownership for migration work. Owners and dates are in the attached summary.",
        )
        .starred(),
        mail(
            Archive,
            "Alex Rivera",
            "Onboarding checklist",
            300 * DAY,
            "Hi Demo,\n\nHere is the onboarding checklist we used for the support team. It covers account access, security training, product walkthroughs, and the first-week check-in.\n\nThe checklist is a good baseline, but update the team-specific links before reusing it.\n\nAlex",
        ),
        mail(
            Archive,
            "Insurance Office",
            "Policy documents",
            450 * DAY,
            "Hello Demo,\n\nYour renewed renter's insurance policy is attached. Coverage begins June 1 and continues through May 31 next year.\n\nThe insured address and deductible are unchanged. Keep the declaration page with your lease records.\n\nInsurance Office",
        )
        .unread(),
        mail(
            Archive,
            "Taylor Brooks",
            "Re: Conference travel",
            600 * DAY,
            CONFERENCE_TRAVEL_THREAD[0],
        )
        .thread(&CONFERENCE_TRAVEL_THREAD),
        mail(
            Archive,
            "newsletter@example.org",
            "Year in review",
            700 * DAY,
            "This year, our community library welcomed 3,420 new members, hosted 186 events, and lent more than 210,000 items.\n\nDonations funded a larger children's reading area and 12 new public computers. Thank you to every volunteer, donor, and visitor who made the work possible.",
        ),
        mail(
            Spam,
            "Prize Department",
            "You may have won something",
            DAY,
            "Congratulations! Your address was selected for a possible promotional reward. Submit your personal details before midnight to check eligibility.\n\nThis fictional message exists only to demonstrate the Spam folder error state.",
        )
        .unread(),
        mail(
            Spam,
            "unknown@example.net",
            "Urgent account notice",
            4 * DAY,
            "Your account will supposedly close today unless you confirm it through an unfamiliar link.\n\nThis fictional message demonstrates suspicious urgency and is not a real account notice.",
        ),
    ]
}

/// Rows matching `query` from anywhere in the mailbox, newest first. Folders
/// are ignored on purpose: a search that stopped at the open folder would not
/// be a search.
fn search_from(fixtures: &[Fixture], query: &str, limit: u32, now: i64) -> ConversationPage {
    let query = query.trim().to_lowercase();
    let mut matching: Vec<_> = fixtures
        .iter()
        .enumerate()
        .filter(|(_, fixture)| matches_query(fixture, &query))
        .collect();
    matching.sort_by_key(|(_, fixture)| fixture.age);

    let conversations: Vec<_> = matching
        .into_iter()
        .take(limit as usize)
        .map(|(index, fixture)| summary(index, fixture, now))
        .collect();

    ConversationPage {
        total: conversations.len() as u32,
        conversations,
    }
}

/// The same places the mailbox search looks: who wrote it, what it is about,
/// and what it says.
fn matches_query(fixture: &Fixture, query: &str) -> bool {
    if query.is_empty() {
        return false;
    }

    [fixture.subject, fixture.correspondents]
        .into_iter()
        .chain(fixture.bodies.iter().copied())
        .any(|value| value.to_lowercase().contains(query))
}

/// The fictional mailbox has only Proton's own folders, so a folder the
/// account made never holds any of it.
fn in_folder(fixture: &Fixture, folder: &Folder) -> bool {
    let Some(folder) = folder.system() else {
        return false;
    };

    fixture.folder == folder || (folder == MailFolder::Starred && fixture.starred)
}

fn list_from(
    fixtures: &[Fixture],
    folder: &Folder,
    page: u32,
    page_size: u32,
    now: i64,
) -> ConversationPage {
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

    ConversationPage {
        conversations,
        total,
    }
}

fn counts_from(fixtures: &[Fixture]) -> MailboxCounts {
    MailFolder::ALL
        .into_iter()
        .map(Folder::System)
        .map(|folder| {
            let unread = fixtures
                .iter()
                .filter(|fixture| fixture.unread && in_folder(fixture, &folder))
                .count();
            (folder, unread as u32)
        })
        .collect()
}

fn detail_from(fixtures: &[Fixture], id: &str, now: i64) -> Option<ConversationDetail> {
    let index = fixture_index(id)?;
    let fixture = fixtures.get(index)?;
    let participants: Vec<MailAddress> = fixture
        .correspondents
        .split(", ")
        .filter(|participant| !participant.is_empty())
        .map(participant_address)
        .collect();
    let count = fixture.message_count();
    let messages = (0..count)
        .map(|position| message(fixture, &participants, index, position, count, now))
        .collect();

    Some(ConversationDetail {
        id: id.to_owned(),
        subject: non_empty(fixture.subject),
        messages,
    })
}

fn fixture_index(id: &str) -> Option<usize> {
    id.strip_prefix("demo-")?.parse().ok()
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
        kind: SummaryKind::Conversation,
        subject: non_empty(fixture.subject),
        correspondents: non_empty(fixture.correspondents),
        participants: fixture_participants(fixture),
        preview: fixture
            .bodies
            .last()
            .and_then(|body| non_empty(&preview(body))),
        time: Some(now - fixture.age),
        unread: fixture.unread,
        starred: fixture.starred,
        message_count: u32::try_from(fixture.message_count()).unwrap_or(u32::MAX),
        // The fictional mailbox carries no files.
        has_attachments: false,
    }
}

fn fixture_participants(fixture: &Fixture) -> Vec<MailAddress> {
    std::iter::once(demo_user())
        .chain(
            fixture
                .correspondents
                .split(", ")
                .filter(|participant| !participant.is_empty())
                .map(participant_address),
        )
        .chain(
            fixture
                .to
                .iter()
                .map(|address| participant_address(address)),
        )
        .collect()
}

fn preview(body: &str) -> String {
    body.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
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
    let from_demo_user = newer.is_multiple_of(2) == fixture.outgoing;

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

    MailMessage {
        id: format!("demo-{index}-{position}"),
        sender,
        recipients,
        time: Some(now - fixture.age - newer as i64 * THREAD_GAP),
        body: MessageBody::PlainText(fixture.bodies[position].to_owned()),
        attachments: Vec::new(),
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

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_789_000_000;

    /// One of Proton's own folders, as a place to list from.
    fn sys(folder: MailFolder) -> Folder {
        Folder::System(folder)
    }

    #[test]
    fn search_reaches_every_folder() {
        let mailbox = DemoMailbox::new();
        let inbox = mailbox
            .list_conversations(&sys(MailFolder::Inbox), 0, PAGE_SIZE, NOW)
            .unwrap();
        // Taken from the data rather than written here, so the test survives
        // any rewrite of the fictional mail.
        let row = inbox
            .conversations
            .iter()
            .find(|row| row.subject.is_some())
            .expect("the demo inbox has mail with subjects");
        let id = row.id.clone();
        let word = row
            .subject
            .as_deref()
            .unwrap()
            .split_whitespace()
            .find(|word| word.len() >= 4)
            .expect("a subject has a word to search for")
            .to_owned();

        let found = mailbox.search(&word, PAGE_SIZE, NOW).unwrap();
        assert!(found.conversations.iter().any(|found| found.id == id));

        // The row keeps turning up once it has left the Inbox: a search that
        // stopped at the open folder would not be a search.
        assert!(mailbox.move_to(&id, &sys(MailFolder::Archive)));
        let found = mailbox.search(&word, PAGE_SIZE, NOW).unwrap();
        assert!(found.conversations.iter().any(|found| found.id == id));

        // An empty query matches nothing, and the limit is honoured.
        assert!(
            mailbox
                .search("   ", PAGE_SIZE, NOW)
                .unwrap()
                .conversations
                .is_empty()
        );
        assert!(mailbox.search(&word, 1, NOW).unwrap().conversations.len() <= 1);
    }

    fn page(folder: MailFolder, page: u32) -> ConversationPage {
        DemoMailbox::new()
            .list_conversations(&sys(folder), page, PAGE_SIZE, NOW)
            .unwrap()
    }

    fn detail(index: usize) -> ConversationDetail {
        DemoMailbox::new()
            .conversation_detail(&format!("demo-{index}"), NOW)
            .unwrap()
    }

    #[test]
    fn counts_are_deterministic() {
        let counts = DemoMailbox::new().counts();

        assert_eq!(counts, DemoMailbox::new().counts());
        assert_eq!(counts.unread(&sys(MailFolder::Inbox)), Some(6));
        assert_eq!(counts.unread(&sys(MailFolder::Starred)), Some(1));
        assert_eq!(counts.unread(&sys(MailFolder::Archive)), Some(1));
        assert_eq!(counts.unread(&sys(MailFolder::Sent)), Some(0));
        assert_eq!(counts.unread(&sys(MailFolder::Trash)), Some(0));
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
    fn trash_starts_empty_and_spam_is_available() {
        let trash = page(MailFolder::Trash, 0);
        let spam = page(MailFolder::Spam, 0);

        assert_eq!(trash.total, 0);
        assert!(trash.conversations.is_empty());
        assert_eq!(spam.total, 2);
        assert_eq!(spam.conversations.len(), 2);
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
    fn every_conversation_uses_its_fixture_bodies() {
        for (index, fixture) in fixtures().iter().enumerate() {
            let detail = detail(index);

            assert_eq!(detail.id, format!("demo-{index}"));
            assert_eq!(detail.messages.len(), fixture.message_count());
            for (message, expected) in detail.messages.iter().zip(&fixture.bodies) {
                assert!(!expected.trim().is_empty());
                assert_eq!(message.body, MessageBody::PlainText((*expected).to_owned()));
            }
            assert!(
                detail
                    .messages
                    .windows(2)
                    .all(|pair| pair[0].time < pair[1].time)
            );
            assert_eq!(
                detail,
                DemoMailbox::new()
                    .conversation_detail(&detail.id, NOW)
                    .unwrap()
            );
        }
        let mailbox = DemoMailbox::new();
        assert_eq!(
            mailbox.conversation_detail("demo-999", NOW),
            Err(MailboxError::Unavailable)
        );
        assert_eq!(
            mailbox.conversation_detail("other", NOW),
            Err(MailboxError::Unavailable)
        );
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
    fn summaries_keep_only_a_bounded_preview() {
        let summary = state_page(&DemoMailbox::new(), MailFolder::Inbox)
            .conversations
            .into_iter()
            .find(|conversation| conversation.id == "demo-5")
            .unwrap();
        let preview = summary.preview.unwrap();
        let MessageBody::PlainText(body) = &detail(5).messages[0].body else {
            panic!("demo bodies are plain text");
        };

        assert!(preview.chars().count() <= 200);
        assert!(body.len() > preview.len());
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

    #[test]
    fn read_actions_update_state_and_counts_idempotently() {
        let mailbox = DemoMailbox::new();

        assert_eq!(mailbox.counts().unread(&sys(MailFolder::Inbox)), Some(6));
        assert!(mailbox.set_unread("demo-0", false));
        assert!(!state_page(&mailbox, MailFolder::Inbox).conversations[0].unread);
        assert_eq!(mailbox.counts().unread(&sys(MailFolder::Inbox)), Some(5));

        assert!(mailbox.set_unread("demo-0", false));
        assert_eq!(mailbox.counts().unread(&sys(MailFolder::Inbox)), Some(5));
        assert!(mailbox.set_unread("demo-0", true));
        assert!(mailbox.set_unread("demo-0", true));
        assert_eq!(mailbox.counts().unread(&sys(MailFolder::Inbox)), Some(6));
    }

    #[test]
    fn star_actions_update_visibility_and_counts_idempotently() {
        let mailbox = DemoMailbox::new();

        assert!(mailbox.set_starred("demo-1", true));
        assert!(mailbox.set_starred("demo-1", true));
        let starred = state_page(&mailbox, MailFolder::Starred);
        assert_eq!(starred.total, 6);
        assert!(starred.conversations.iter().any(|c| c.id == "demo-1"));
        assert_eq!(mailbox.counts().unread(&sys(MailFolder::Starred)), Some(2));

        assert!(mailbox.set_starred("demo-1", false));
        assert!(mailbox.set_starred("demo-1", false));
        assert_eq!(state_page(&mailbox, MailFolder::Starred).total, 5);
        assert_eq!(mailbox.counts().unread(&sys(MailFolder::Starred)), Some(1));
    }

    #[test]
    fn archive_moves_conversation_and_preserves_content() {
        let mailbox = DemoMailbox::new();
        let detail = mailbox.conversation_detail("demo-0", NOW).unwrap();

        assert!(mailbox.move_to("demo-0", &sys(MailFolder::Archive)));

        assert!(!contains(&mailbox, MailFolder::Inbox, "demo-0"));
        assert!(contains(&mailbox, MailFolder::Archive, "demo-0"));
        assert_eq!(mailbox.conversation_detail("demo-0", NOW), Ok(detail));
    }

    #[test]
    fn trash_moves_conversation_without_deleting_it() {
        let mailbox = DemoMailbox::new();

        assert!(mailbox.move_to("demo-3", &sys(MailFolder::Trash)));

        assert!(!contains(&mailbox, MailFolder::Inbox, "demo-3"));
        assert!(contains(&mailbox, MailFolder::Trash, "demo-3"));
        assert!(mailbox.conversation_detail("demo-3", NOW).is_ok());
    }

    #[test]
    fn spam_moves_conversation_without_network_behavior() {
        let mailbox = DemoMailbox::new();

        assert!(mailbox.move_to("demo-4", &sys(MailFolder::Spam)));

        assert!(!contains(&mailbox, MailFolder::Inbox, "demo-4"));
        assert!(contains(&mailbox, MailFolder::Spam, "demo-4"));
    }

    #[test]
    fn derived_counts_match_every_folder_after_repeated_actions() {
        let mailbox = DemoMailbox::new();
        assert!(mailbox.set_unread("demo-0", false));
        assert!(mailbox.set_unread("demo-0", false));
        assert!(mailbox.set_starred("demo-1", true));
        assert!(mailbox.set_starred("demo-1", true));
        assert!(mailbox.move_to("demo-2", &sys(MailFolder::Archive)));
        assert!(mailbox.move_to("demo-3", &sys(MailFolder::Trash)));
        assert!(mailbox.move_to("demo-4", &sys(MailFolder::Spam)));

        let counts = mailbox.counts();
        for folder in MailFolder::ALL {
            let unread = state_page(&mailbox, folder)
                .conversations
                .iter()
                .filter(|conversation| conversation.unread)
                .count() as u32;
            assert_eq!(counts.unread(&sys(folder)), Some(unread), "{folder:?}");
        }
    }

    fn state_page(mailbox: &DemoMailbox, folder: MailFolder) -> ConversationPage {
        mailbox
            .list_conversations(&sys(folder), 0, 1_000, NOW)
            .unwrap()
    }

    fn contains(mailbox: &DemoMailbox, folder: MailFolder, id: &str) -> bool {
        state_page(mailbox, folder)
            .conversations
            .iter()
            .any(|conversation| conversation.id == id)
    }
}
