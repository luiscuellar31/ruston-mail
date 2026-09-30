//! Capture HTTP diagnostics in a dedicated test binary: concurrent transport
//! tests can otherwise register tracing callsites without this task's subscriber.

use ruston_core::transport::{Doer, HttpClient, Request};
use std::sync::{Arc, atomic::Ordering};
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

#[tokio::test]
async fn http_diagnostics_omit_urls_and_queries_without_changing_requests_or_retries() {
    use std::sync::atomic::AtomicUsize;
    use tracing::instrument::WithSubscriber;

    let server = MockServer::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    Mock::given(method("GET"))
        .and(path("/api/messages"))
        .respond_with(move |_: &wiremock::Request| {
            if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(429).set_body_json(serde_json::json!({"Code": 429}))
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000}))
            }
        })
        .expect(3)
        .mount(&server)
        .await;
    let buffer = DiagnosticBuffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter("ruston_core::http=trace")
        .with_ansi(false)
        .without_time()
        .with_writer(move || writer.clone())
        .finish();
    async {
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
    }
    .with_subscriber(subscriber)
    .await;
    let logged = String::from_utf8(buffer.0.lock().unwrap().clone()).unwrap();
    assert!(!logged.is_empty(), "expected captured HTTP diagnostics");
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
    assert_eq!(
        logged.matches("path=\"/api/messages\"").count(),
        3,
        "{logged}"
    );
    assert!(logged.contains("method=GET"));
    assert!(logged.contains("body=\"empty\""));
    assert!(logged.contains("status=429"), "{logged}");
    assert!(logged.contains("status=200"), "{logged}");
    assert!(logged.contains("bytes="));
    assert!(logged.contains("ms="));
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    for request in &requests[..2] {
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
        requests[2].url.query(),
        Some("Subject=private%20subject&From=private%40example.com")
    );
}
