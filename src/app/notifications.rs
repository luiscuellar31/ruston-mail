//! Bounded, process-local detection of new Inbox message IDs.

use std::collections::HashSet;

use crate::mail::IncomingMail;

pub(super) const RECENT_LIMIT: u32 = 20;

#[derive(Default)]
pub(super) struct NewMail {
    initialized: bool,
    newest_time: i64,
    seen: HashSet<String>,
    pub(super) pending: Option<u64>,
}

impl NewMail {
    /// First success establishes a baseline. Unread changes on a known ID are
    /// never arrivals. Keep IDs at the watermark to handle equal timestamps.
    pub(super) fn observe<'a>(&mut self, messages: &'a [IncomingMail]) -> Option<&'a IncomingMail> {
        let arrival = self
            .initialized
            .then(|| {
                messages
                    .iter()
                    .take(RECENT_LIMIT as usize)
                    .filter(|mail| {
                        mail.unread
                            && mail.time > 0
                            && mail.time >= self.newest_time
                            && !self.seen.contains(&mail.id)
                    })
                    .max_by_key(|mail| mail.time)
            })
            .flatten();
        self.initialized = true;
        if let Some(newest) = messages
            .iter()
            .take(RECENT_LIMIT as usize)
            .map(|mail| mail.time)
            .max()
        {
            if newest > self.newest_time {
                self.newest_time = newest;
                self.seen.clear();
            }
            for mail in messages
                .iter()
                .take(RECENT_LIMIT as usize)
                .filter(|mail| mail.time == self.newest_time)
            {
                self.seen.insert(mail.id.clone());
            }
            // Bound IDs even if a server returns arbitrarily many equal timestamps.
            // Retaining the last sample is enough for the supported recent page.
            if self.seen.len() > RECENT_LIMIT as usize {
                self.seen = messages
                    .iter()
                    .take(RECENT_LIMIT as usize)
                    .filter(|mail| mail.time == self.newest_time)
                    .map(|mail| mail.id.clone())
                    .collect();
            }
        }
        arrival
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(id: &str, time: i64, unread: bool) -> IncomingMail {
        IncomingMail {
            id: id.into(),
            time,
            unread,
            sender: "Alice".into(),
            subject: "Hello".into(),
        }
    }

    #[test]
    fn baseline_and_read_changes_are_silent_but_new_ids_notify() {
        let mut state = NewMail::default();
        assert!(state.observe(&[mail("old", 100, false)]).is_none());
        assert!(state.observe(&[mail("old", 100, true)]).is_none());
        let batch = [mail("new", 101, true), mail("old", 100, false)];
        assert_eq!(state.observe(&batch).unwrap().id, "new");
        assert!(state.observe(&batch).is_none());
        assert!(state.observe(&[mail("imported", 50, true)]).is_none());
    }

    #[test]
    fn equal_timestamp_arrivals_and_replies_have_distinct_message_ids() {
        let mut state = NewMail::default();
        let baseline = [mail("first", 100, true)];
        state.observe(&baseline);
        let batch = [mail("reply", 100, true), mail("first", 100, true)];
        assert_eq!(state.observe(&batch).unwrap().id, "reply");
        assert!(state.observe(&batch).is_none());
    }

    #[test]
    fn empty_baseline_and_bursts_are_bounded_to_one_notification() {
        let mut state = NewMail::default();
        assert!(state.observe(&[]).is_none());
        let burst: Vec<_> = (0..100).map(|i| mail(&i.to_string(), 100, true)).collect();
        assert!(state.observe(&burst).is_some());
        assert!(state.seen.len() <= RECENT_LIMIT as usize);
        assert!(state.observe(&burst).is_none());
    }
}
