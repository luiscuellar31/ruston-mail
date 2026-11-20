//! Capture HTTP diagnostics in a dedicated test binary: concurrent transport
//! tests can otherwise register tracing callsites without this task's subscriber.

use ruston_core::transport::{Doer, HttpClient, Request};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(base: &str) -> HttpClient {
    HttpClient::new(base, "Other")
}

#[derive(Clone, Default)]
struct DiagnosticBuffer(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for DiagnosticBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The requests the test makes: two calls, the first of which is tried again
/// after its 429.
const ATTEMPTS: usize = 3;

/// Answers the first request for the messages with a 429 that may be tried
/// again at once, and every later one with success.
async fn mount_429_then_ok(server: &MockServer) {
    let calls = AtomicUsize::new(0);
    Mock::given(method("GET"))
        .and(path("/api/messages"))
        .respond_with(move |_: &wiremock::Request| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "0")
                    .set_body_json(serde_json::json!({"Code": 429}))
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000}))
            }
        })
        .expect(ATTEMPTS as u64)
        .mount(server)
        .await;
}

/// What the transport logs, at its most detailed, while `work` runs.
async fn logged_during(work: impl Future<Output = ()>) -> String {
    use tracing::instrument::WithSubscriber;

    let buffer = DiagnosticBuffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter("ruston_core::http=trace")
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    work.with_subscriber(subscriber).await;
    String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap()
}

fn assert_logs_nothing_private(logged: &str) {
    for secret in [
        "private",
        "Keyword",
        "Subject",
        "From=",
        "http://",
        "url=",
        "127.0.0.1",
    ] {
        assert!(
            !logged.contains(secret),
            "diagnostic leaked {secret}: {logged}"
        );
    }
}

#[tokio::test]
async fn http_diagnostics_omit_urls_and_queries_without_changing_requests_or_retries() {
    let server = MockServer::start().await;
    mount_429_then_ok(&server).await;

    let logged = logged_during(async {
        let base = server
            .uri()
            .replacen("http://", "http://private-user:private-password@", 1);
        client(&base)
            .do_raw(
                Request::get("/api/messages")
                    .query("Keyword", "private-search +/&")
                    .query("Keyword", "second-private-search")
                    .query("From", "private-sender@example.com"),
            )
            .await
            .unwrap();
        // A caller can place a query and fragment directly in Request.path.
        let path =
            "/api/messages?Subject=private%20subject&From=private%40example.com#private-fragment";
        client(&server.uri())
            .do_raw(Request::get(path))
            .await
            .unwrap();
    })
    .await;

    assert!(!logged.is_empty(), "expected captured HTTP diagnostics");
    assert_logs_nothing_private(&logged);

    // Each attempt leaves its record.
    assert_eq!(
        logged.matches("path=\"/api/messages\"").count(),
        ATTEMPTS,
        "{logged}"
    );
    assert!(logged.contains("method=GET"));
    assert!(logged.contains("body=\"empty\""));
    assert!(logged.contains("status=429"), "{logged}");
    assert!(logged.contains("status=200"), "{logged}");
    assert!(logged.contains("bytes="));
    assert!(logged.contains("ms="));

    // The requests went out as they were asked for.
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), ATTEMPTS);
    // The first call and its second try, then the second call.
    let (first_call, second_call) = requests.split_at(2);
    for request in first_call {
        let pairs: Vec<_> = request.url.query_pairs().collect();
        assert_eq!(
            pairs,
            vec![
                ("Keyword".into(), "private-search +/&".into()),
                ("Keyword".into(), "second-private-search".into()),
                ("From".into(), "private-sender@example.com".into()),
            ]
        );
    }
    assert_eq!(
        second_call[0].url.query(),
        Some("Subject=private%20subject&From=private%40example.com")
    );
}
