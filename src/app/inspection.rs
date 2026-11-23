use std::collections::{HashMap, HashSet, VecDeque};

use futures::future::{AbortHandle, AbortRegistration, Abortable};

use super::mailbox::RequestId;
use super::{App, Effects, Message, SessionEpoch};
use crate::mail::{ConversationSummary, Folder, INSPECTION_CONCURRENCY, MailboxError};

/// One Proton page of best-effort work, including running inspections. Overflow
/// keeps the server's conversation grouping instead of retaining more tasks.
pub(super) const CAPACITY: usize = 50;

/// Waiting candidates are only IDs: they hold neither a task nor a client.
#[derive(Default)]
pub(super) struct Inspections {
    queued: VecDeque<String>,
    /// Every ID admitted, whether queued or running.
    admitted: HashSet<String>,
    running: HashMap<String, (RequestId, AbortHandle)>,
}

impl Inspections {
    pub fn enqueue(&mut self, candidates: impl IntoIterator<Item = String>) {
        for id in candidates {
            if self.admitted.len() == CAPACITY {
                break;
            }
            if self.admitted.insert(id.clone()) {
                self.queued.push_back(id);
            }
        }
    }

    /// Whether [`Self::start`] has a conversation to start now: one waits
    /// and a slot is free.
    pub fn can_start(&self) -> bool {
        self.running.len() < INSPECTION_CONCURRENCY && !self.queued.is_empty()
    }

    pub fn start(&mut self, request: RequestId) -> Option<(String, AbortRegistration)> {
        if self.running.len() == INSPECTION_CONCURRENCY {
            return None;
        }
        let id = self.queued.pop_front()?;
        let (handle, registration) = AbortHandle::new_pair();
        self.running.insert(id.clone(), (request, handle));
        Some((id, registration))
    }

    /// The request `id`'s inspection runs under, if it runs.
    #[cfg(test)]
    pub fn running_request(&self, id: &str) -> Option<RequestId> {
        self.running.get(id).map(|(request, _)| *request)
    }

    /// Only the exact running request may release a slot and apply its result.
    pub fn finish(&mut self, request: RequestId, id: &str) -> bool {
        if !self
            .running
            .get(id)
            .is_some_and(|(current, _)| *current == request)
        {
            return false;
        }
        self.running.remove(id);
        self.admitted.remove(id);
        true
    }

    pub fn cancel(&mut self) {
        for (_, (_, handle)) in self.running.drain() {
            handle.abort();
        }
        self.queued.clear();
        self.admitted.clear();
    }
}

impl Drop for Inspections {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl App {
    pub(super) fn inspect_candidates(&mut self, candidates: Vec<String>) -> Effects {
        let Some(mailbox) = self.mailbox.as_ref() else {
            return Effects::none();
        };
        if mailbox.shown_search_query().is_some() {
            return Effects::none();
        }
        // The automatic refresh lists the same rows every minute: only those
        // new or changed since they were last looked into are asked about.
        self.inspections.enqueue(
            candidates
                .into_iter()
                .filter(|id| mailbox.worth_inspecting(id)),
        );
        self.start_inspections()
    }

    pub(super) fn start_inspections(&mut self) -> Effects {
        if !self.auth_state.is_authenticated() {
            return Effects::none();
        }
        let (Some(backend), Some(mailbox)) = (self.backend.clone(), self.mailbox.as_ref()) else {
            return Effects::none();
        };
        let epoch = self.session_epoch;
        let folder = mailbox.folder().clone();
        let revision = mailbox.inspection_revision();
        let mut effects = Vec::new();
        // Each inspection takes the next request ID itself, asked for only
        // when there is one to start, so no ID goes unused.
        while self.inspections.can_start() {
            let request = self.next_request();
            let Some((id, registration)) = self.inspections.start(request) else {
                break;
            };
            // A queued row may have been moved or removed before its turn.
            if !self
                .mailbox
                .as_ref()
                .is_some_and(|mailbox| mailbox.has_inspection_target(&id))
            {
                self.inspections.finish(request, &id);
                continue;
            }
            let backend = backend.clone();
            let target_folder = folder.clone();
            let callback_folder = folder.clone();
            let conversation_id = id.clone();
            let future = Abortable::new(
                async move {
                    backend
                        .inspect_conversation(&conversation_id, &target_folder)
                        .await
                },
                registration,
            );
            effects.push(Effects::perform(future, move |result| {
                Message::ThreadInspected {
                    epoch,
                    request,
                    revision,
                    folder: callback_folder,
                    conversation_id: id,
                    result: result.unwrap_or(Ok(None)),
                }
            }));
        }
        Effects::batch(effects)
    }

    /// Splits the row an inspection looked into, or keeps it whole, while its
    /// answer is still wanted, and starts the inspections waiting.
    pub(super) fn finish_inspection(
        &mut self,
        epoch: SessionEpoch,
        request: RequestId,
        revision: u64,
        folder: Folder,
        conversation_id: String,
        result: Result<Option<Vec<ConversationSummary>>, MailboxError>,
    ) -> Effects {
        // Finishing first frees the answer's place even when the folder was
        // renamed meanwhile, which keeps the folder.
        if !self.is_current_session(epoch)
            || !self.inspections.finish(request, &conversation_id)
            || !self
                .mailbox
                .as_ref()
                .is_some_and(|mailbox| mailbox.shows(&folder))
        {
            return Effects::none();
        }
        match result {
            Ok(Some(split_rows)) => {
                if let Some(mailbox) = self.mailbox.as_mut()
                    && mailbox.inspection_revision() == revision
                {
                    mailbox.split_conversation(&folder, &conversation_id, split_rows);
                }
            }
            Ok(None) => {
                if let Some(mailbox) = self.mailbox.as_mut()
                    && mailbox.inspection_revision() == revision
                {
                    mailbox.keep_whole(&conversation_id);
                }
            }
            Err(error) => {
                self.end_session_if_expired(Some(error));
            }
        }
        self.start_inspections()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::Abortable;
    use futures::{FutureExt, executor::block_on};
    use std::sync::Arc;

    #[test]
    fn bounds_and_deduplicates_work_across_pages() {
        let mut inspections = Inspections::default();
        assert!(!inspections.can_start(), "nothing waits");
        inspections.enqueue((0..200).flat_map(|i| [i.to_string(), i.to_string()]));
        assert_eq!(inspections.admitted.len(), CAPACITY);
        for request in 1..=4 {
            assert!(inspections.can_start());
            assert!(inspections.start(request).is_some());
        }
        // Every slot is taken: an ID asked for now would go unused.
        assert!(!inspections.can_start());
        assert!(inspections.start(5).is_none());
        inspections.enqueue(["0".into(), "extra".into()]);
        assert_eq!(inspections.admitted.len(), CAPACITY);
        assert!(!inspections.finish(99, "0"));
        assert!(inspections.finish(1, "0"));
        assert!(!inspections.finish(1, "0"));
        let (id, _) = inspections.start(5).unwrap();
        assert_eq!(id, "4");
        inspections.enqueue(["4".into(), "extra".into()]);
        assert_eq!(inspections.admitted.len(), CAPACITY);
        assert_eq!(inspections.queued.back().unwrap(), "extra");
    }

    #[test]
    fn cancellation_drops_pending_future_and_its_client_reference() {
        let mut inspections = Inspections::default();
        inspections.enqueue(["id".into(), "queued".into()]);
        let (_, registration) = inspections.start(1).unwrap();
        let client = Arc::new(());
        let weak = Arc::downgrade(&client);
        let mut future = Box::pin(Abortable::new(
            async move {
                let _client = client;
                std::future::pending::<()>().await;
            },
            registration,
        ));
        assert!(future.as_mut().now_or_never().is_none());
        inspections.cancel();
        assert!(block_on(future).is_err());
        assert!(weak.upgrade().is_none());
        assert!(!inspections.finish(1, "id"));
        assert!(inspections.start(2).is_none());
        inspections.enqueue(["id".into()]);
        assert!(inspections.start(2).is_some());
        assert!(!inspections.finish(1, "id"));
        assert!(inspections.finish(2, "id"));
    }

    #[test]
    fn dropping_owner_cancels_even_unpolled_work() {
        let mut inspections = Inspections::default();
        inspections.enqueue(["id".into()]);
        let (_, registration) = inspections.start(1).unwrap();
        let future = Abortable::new(
            async { panic!("cancelled work must not start") },
            registration,
        );
        drop(inspections);
        assert!(block_on(future).is_err());
    }
}
