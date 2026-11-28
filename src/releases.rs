//! Whether a newer Ruston Mail has been published, for the app to say so
//! when it is asked to check. It only asks and tells: nothing is
//! downloaded, installed or run.

use std::fmt;
use std::time::Duration;

use serde::Deserialize;

/// GitHub's latest published release of this repository, which is never a
/// draft or a prerelease.
const LATEST_RELEASE: &str =
    "https://api.github.com/repos/luiscuellar31/ruston-mail/releases/latest";

/// The page that announces the latest release, built here rather than
/// taken from GitHub's reply.
pub const RELEASE_PAGE: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases/latest");

/// How long the window waits after one check before the next, by its clock.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The most of GitHub's reply that is read. A release's JSON, with its notes
/// and a dozen downloads, is tens of kilobytes.
const MAX_REPLY: usize = 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const TIMEOUT: Duration = Duration::from_secs(30);

/// A release's number, `MAJOR.MINOR.PATCH`, compared part by part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version([u64; 3]);

impl Version {
    /// Reads `1.2.3`, or a tag such as `v1.2.3`. Anything else, such as a
    /// prerelease's `1.2.3-beta`, is no version.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
        let mut number = [0; 3];
        for part in &mut number {
            // Digits only: `parse` alone would also take a sign.
            let digits = parts.next().filter(|digits| {
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            })?;
            *part = digits.parse().ok()?;
        }
        parts.next().is_none().then_some(Self(number))
    }

    /// Whether this release is newer than the one running.
    pub fn is_newer(self) -> bool {
        Self::parse(env!("CARGO_PKG_VERSION")).is_some_and(|running| self > running)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch] = self.0;
        write!(f, "{major}.{minor}.{patch}")
    }
}

/// Asks GitHub for the number of the latest published release: none when
/// GitHub cannot be reached or its reply cannot be read.
pub async fn latest_release() -> Option<Version> {
    latest_release_at(LATEST_RELEASE).await
}

/// The same, asked of `url`. The request says nothing about the person, the
/// account or the system, and carries no cookies; a redirect is not
/// followed. The system's proxies apply, and the TLS of core's Proton client.
async fn latest_release_at(url: &str) -> Option<Version> {
    let client = reqwest::Client::builder()
        .user_agent("ruston-mail")
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;
    let mut response = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    // Counted as it arrives, decompressed, whatever length the reply states.
    let mut reply = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if reply.len() + chunk.len() > MAX_REPLY {
            return None;
        }
        reply.extend_from_slice(&chunk);
    }

    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
    }
    let release: Release = serde_json::from_slice(&reply).ok()?;
    Version::parse(&release.tag_name)
}

/// When the window asks GitHub, by its own clock: at once when checking is
/// on, and a day after each check.
#[derive(Debug, Default)]
pub struct Schedule {
    next_at: Option<f64>,
}

impl Schedule {
    /// Whether to check at `now`, with checking `on`. Turned off, it checks
    /// at once the next time it is on.
    pub fn due(&mut self, now: f64, on: bool) -> bool {
        if !on {
            self.next_at = None;
            return false;
        }
        if self.next_at.is_some_and(|at| now < at) {
            return false;
        }
        self.next_at = Some(now + CHECK_INTERVAL.as_secs_f64());
        true
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const DAY: f64 = 24.0 * 60.0 * 60.0;

    #[test]
    fn a_release_number_is_three_numbers_with_or_without_a_v() {
        assert_eq!(Version::parse("v0.10.1"), Some(Version([0, 10, 1])));
        assert_eq!(Version::parse("1.2.3"), Some(Version([1, 2, 3])));
        for unreadable in [
            "",
            "v",
            "1.2",
            "1.2.3.4",
            "1.2.3-beta",
            "v1.2.3+build",
            "V1.2.3",
            "1..3",
            "+1.2.3",
            "1.-2.3",
            " 1.2.3",
            "1.2.3 ",
            "99999999999999999999.0.0",
        ] {
            assert_eq!(Version::parse(unreadable), None, "{unreadable:?}");
        }
        assert_eq!(Version([0, 11, 0]).to_string(), "0.11.0");
    }

    #[test]
    fn releases_compare_by_number_not_by_text() {
        let parse = |text| Version::parse(text).unwrap();
        assert!(parse("0.10.0") > parse("0.9.9"));
        assert!(parse("1.0.0") > parse("0.99.99"));
        assert!(parse("0.10.1") > parse("0.10.0"));
        assert_eq!(parse("v0.10.1"), parse("0.10.1"));
    }

    #[test]
    fn only_a_later_release_than_the_running_one_is_newer() {
        let running = Version::parse(env!("CARGO_PKG_VERSION")).expect("the package version reads");
        let [major, minor, patch] = running.0;
        assert!(Version([major, minor, patch + 1]).is_newer());
        assert!(Version([major + 1, 0, 0]).is_newer());
        assert!(!running.is_newer());
        assert!(!Version([0, 0, 0]).is_newer());
    }

    #[test]
    fn github_is_asked_about_the_repository_whose_page_opens() {
        let repository = env!("CARGO_PKG_REPOSITORY");
        assert_eq!(
            LATEST_RELEASE,
            format!(
                "{}/releases/latest",
                repository.replacen("https://github.com/", "https://api.github.com/repos/", 1)
            )
        );
        assert_eq!(RELEASE_PAGE, format!("{repository}/releases/latest"));
    }

    #[test]
    fn the_window_checks_at_once_and_then_a_day_after_each_check() {
        let mut schedule = Schedule::default();
        assert!(!schedule.due(0.0, false), "off, nothing is asked");
        assert!(schedule.due(5.0, true), "on, at once");
        assert!(!schedule.due(6.0, true));
        assert!(!schedule.due(5.0 + DAY - 1.0, true));
        assert!(schedule.due(5.0 + DAY, true), "a day later");
        assert!(!schedule.due(5.0 + DAY + 1.0, true));

        // Off and on again, it asks at once rather than waiting out the day.
        assert!(!schedule.due(10.0 + DAY, false));
        assert!(schedule.due(11.0 + DAY, true));
    }

    /// A server that answers the latest release's path as GitHub would.
    async fn github(response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/releases/latest"))
            .respond_with(response)
            .mount(&server)
            .await;
        server
    }

    fn latest_at(server: &MockServer) -> String {
        format!("{}/repos/o/r/releases/latest", server.uri())
    }

    #[tokio::test]
    async fn the_latest_release_number_is_all_that_is_read() {
        let server = github(ResponseTemplate::new(200).set_body_raw(
            r#"{"tag_name":"v0.11.0","name":"<b>The Next Update</b>","html_url":"https://example.com/elsewhere","body":"Notes","assets":[]}"#,
            "application/json",
        ))
        .await;

        assert_eq!(
            latest_release_at(&latest_at(&server)).await,
            Some(Version([0, 11, 0]))
        );

        let requests = server.received_requests().await.unwrap();
        let [request] = requests.as_slice() else {
            panic!("one request, got {}", requests.len());
        };
        let mut sent: Vec<&str> = request.headers.keys().map(|name| name.as_str()).collect();
        sent.sort_unstable();
        // Only what GitHub's API asks for: nothing about the person, the
        // account or the system, and no cookies.
        assert_eq!(
            sent,
            [
                "accept",
                "accept-encoding",
                "host",
                "user-agent",
                "x-github-api-version"
            ]
        );
        assert_eq!(request.headers["user-agent"], "ruston-mail");
        assert_eq!(request.headers["accept"], "application/vnd.github+json");
        assert!(request.body.is_empty());
    }

    #[tokio::test]
    async fn a_refusal_a_redirect_or_an_unreadable_reply_finds_nothing() {
        let refused = github(ResponseTemplate::new(403).set_body_raw(
            r#"{"tag_name":"v9.9.9","message":"API rate limit exceeded"}"#,
            "application/json",
        ))
        .await;
        assert_eq!(latest_release_at(&latest_at(&refused)).await, None);

        let elsewhere = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"tag_name":"v9.9.9"}"#, "application/json"),
            )
            .mount(&elsewhere)
            .await;
        let moved = github(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/x", elsewhere.uri())),
        )
        .await;
        assert_eq!(latest_release_at(&latest_at(&moved)).await, None);
        assert!(elsewhere.received_requests().await.unwrap().is_empty());

        for unreadable in [
            r#"{"tag_name":"v1.2.3-beta"}"#,
            r#"{"tag_name":7}"#,
            r#"{"name":"v1.2.3"}"#,
            "<html>v1.2.3</html>",
        ] {
            let server =
                github(ResponseTemplate::new(200).set_body_raw(unreadable, "application/json"))
                    .await;
            assert_eq!(
                latest_release_at(&latest_at(&server)).await,
                None,
                "{unreadable}"
            );
        }
    }

    #[tokio::test]
    async fn a_reply_longer_than_any_release_is_not_read_through() {
        let notes = "z".repeat(MAX_REPLY);
        let server = github(ResponseTemplate::new(200).set_body_raw(
            format!(r#"{{"tag_name":"v9.9.9","body":"{notes}"}}"#),
            "application/json",
        ))
        .await;

        assert_eq!(latest_release_at(&latest_at(&server)).await, None);
    }
}
