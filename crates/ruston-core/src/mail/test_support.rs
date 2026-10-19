//! Helpers the mail tests share.

use super::{Client, SenderKeyCache};
use crate::cache::CacheIdentity;
use crate::crypto::{AddressKeys, KeyStore};
use crate::session::{MemoryStore, Paths};
use crate::transport::HttpClient;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wiremock::MockServer;

/// A client of `server` that holds `addresses`' keys and saves nothing.
pub(crate) fn client(server: &MockServer, addresses: Vec<AddressKeys>) -> Client {
    Client {
        http: HttpClient::new(server.uri(), "Other"),
        keys: KeyStore {
            user_keys: Vec::new(),
            addresses,
        },
        paths: Paths::with_base(std::env::temp_dir()),
        profile: "test".into(),
        session_uid: "test".into(),
        cache_identity: CacheIdentity::new(&server.uri(), "test").unwrap(),
        store: Arc::new(MemoryStore::default()),
        sender_cache: tokio::sync::Mutex::new(SenderKeyCache::default()),
    }
}

/// Waits until `counter` reaches `count`; the test fails if `work` takes
/// longer than two seconds to get there.
pub(crate) async fn wait_for(counter: &AtomicUsize, count: usize, work: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while counter.load(Ordering::SeqCst) < count {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{work} did not make progress"));
}
