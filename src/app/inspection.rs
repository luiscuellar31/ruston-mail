use std::collections::{HashMap, HashSet, VecDeque};

use futures::future::{AbortHandle, AbortRegistration};

use super::mailbox::RequestId;
use crate::mail::INSPECTION_CONCURRENCY;

/// One Proton page of best-effort work, including running inspections. Overflow
/// keeps the server's conversation grouping instead of retaining more tasks.
const CAPACITY: usize = 50;

/// Waiting candidates are only IDs: they hold neither a task nor a client.
#[derive(Default)]
pub(super) struct Inspections {
    queued: VecDeque<String>,
    candidates: HashSet<String>,
    running: HashMap<String, (RequestId, AbortHandle)>,
}

impl Inspections {
    pub fn enqueue(&mut self, candidates: impl IntoIterator<Item = String>) {
        for id in candidates {
            if self.candidates.len() == CAPACITY {
                break;
            }
            if self.candidates.insert(id.clone()) {
                self.queued.push_back(id);
            }
        }
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
        self.candidates.remove(id);
        true
    }

    pub fn cancel(&mut self) {
        for (_, (_, handle)) in self.running.drain() {
            handle.abort();
        }
        self.queued.clear();
        self.candidates.clear();
    }
}

impl Drop for Inspections {
    fn drop(&mut self) {
        self.cancel();
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
        inspections.enqueue((0..200).flat_map(|i| [i.to_string(), i.to_string()]));
        assert_eq!(inspections.candidates.len(), CAPACITY);
        for request in 1..=4 {
            assert!(inspections.start(request).is_some());
        }
        assert!(inspections.start(5).is_none());
        inspections.enqueue(["0".into(), "extra".into()]);
        assert_eq!(inspections.candidates.len(), CAPACITY);
        assert!(!inspections.finish(99, "0"));
        assert!(inspections.finish(1, "0"));
        assert!(!inspections.finish(1, "0"));
        let (id, _) = inspections.start(5).unwrap();
        assert_eq!(id, "4");
        inspections.enqueue(["4".into(), "extra".into()]);
        assert_eq!(inspections.candidates.len(), CAPACITY);
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
