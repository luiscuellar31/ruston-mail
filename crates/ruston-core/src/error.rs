//! Error types for ruston-core.

use std::fmt;

use crate::model::enums::human_verification;

/// A human-verification challenge returned by the API (code 9001).
#[derive(Debug, Clone)]
pub struct HvChallenge {
    /// Opaque verification token to echo back once the challenge is solved.
    pub token: String,
    /// Supported verification methods (e.g. `captcha`, `email`, `sms`).
    pub methods: Vec<String>,
    /// URL where the user completes the challenge in a browser.
    pub web_url: String,
}

impl HvChallenge {
    /// Whether solving a CAPTCHA answers this challenge: Proton names it among
    /// the methods, or names no method at all.
    pub fn offers_captcha(&self) -> bool {
        self.methods.is_empty()
            || self
                .methods
                .iter()
                .any(|method| method == human_verification::CAPTCHA)
    }

    /// The page where a person solves this challenge's CAPTCHA. The host the
    /// server named is kept only when it is one of Proton's own over https,
    /// since the page receives the token; anything else opens Proton's
    /// default page.
    pub fn verification_url(&self) -> String {
        let origin = self
            .web_url
            .strip_prefix("https://")
            .and_then(|rest| rest.split(['/', '?', '#']).next())
            .filter(|host| is_proton_host(host))
            .map_or_else(
                || human_verification::PAGE.to_owned(),
                |host| format!("https://{host}"),
            );
        format!(
            "{origin}/?methods={}&token={}",
            human_verification::CAPTCHA,
            urlencoding::encode(&self.token)
        )
    }
}

/// Whether `host` is a plain host name in Proton's verification domain.
fn is_proton_host(host: &str) -> bool {
    let plain = host
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    let domain = human_verification::DOMAIN;
    plain && (host == domain || host.ends_with(&format!(".{domain}")))
}

/// The process exit statuses the CLI ends with, as its `--help` explains.
pub mod exit_status {
    /// Any other error.
    pub const OTHER: i32 = 1;
    /// A session that is missing, expired or refused.
    pub const SESSION: i32 = 2;
    /// Something the request named was not found.
    pub const NOT_FOUND: i32 = 3;
    /// An ambiguous reference, or a conflict Proton reports.
    pub const CONFLICT: i32 = 4;
    /// A Proton server error.
    pub const SERVER: i32 = 5;
}

/// A structured Proton API error (non-success `Code`, or a non-2xx HTTP status).
#[derive(Debug, Clone)]
pub struct ApiError {
    /// HTTP status code of the response.
    pub http_status: u16,
    /// Proton API `Code` from the response body.
    pub code: i64,
    /// Human-readable error message from the API.
    pub message: String,
    /// The start of the raw response body, up to 4 KiB, kept for diagnostics.
    pub raw_body: String,
}

impl ApiError {
    /// Map to the CLI's process exit status, one of [`exit_status`].
    pub fn exit_code(&self) -> i32 {
        match self.http_status {
            401 | 403 => exit_status::SESSION,
            404 => exit_status::NOT_FOUND,
            409 | 422 => exit_status::CONFLICT,
            _ if self.is_server_error() => exit_status::SERVER,
            _ => exit_status::OTHER,
        }
    }

    /// Whether Proton refused the session's access token (HTTP 401).
    pub fn is_unauthorized(&self) -> bool {
        self.http_status == 401
    }

    /// Whether Proton is asking the client to slow down (HTTP 429).
    pub fn is_rate_limited(&self) -> bool {
        self.http_status == 429
    }

    /// Whether the failure was on Proton's side (HTTP 5xx).
    pub fn is_server_error(&self) -> bool {
        self.http_status >= 500
    }

    /// Whether the same request may succeed later: Proton was busy or down,
    /// rather than refusing the request or the session.
    pub fn may_pass(&self) -> bool {
        Self::status_may_pass(self.http_status)
    }

    /// [`may_pass`](Self::may_pass) for an answer known only by its HTTP
    /// status, as a frontend keeps it to show.
    pub fn status_may_pass(http_status: u16) -> bool {
        http_status == 429 || http_status >= 500
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[HTTP {}] {}: {}",
            self.http_status, self.code, self.message
        )
    }
}

impl std::error::Error for ApiError {}

/// The crate-wide error type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A structured Proton API error (non-success code or non-2xx status).
    #[error(transparent)]
    Api(#[from] ApiError),

    /// The session is expired or invalid.
    #[error("unauthorized (session expired or invalid)")]
    Unauthorized,

    /// The API demands human verification before proceeding.
    #[error("human verification required")]
    HumanVerification(HvChallenge),

    /// A transport-level HTTP failure. Conversion removes its attached URL so
    /// displaying the error cannot expose request queries or URL credentials.
    #[error("http error: {0}")]
    Http(#[source] reqwest::Error),

    /// The response body exceeded the request's size limit.
    #[error("response exceeds the {limit} byte limit")]
    ResponseTooLarge {
        /// Maximum accepted response body size.
        limit: usize,
    },

    /// An attachment collection exceeded its combined plaintext size limit.
    #[error("attachments exceed the {limit} byte total limit")]
    AttachmentBatchTooLarge {
        /// Maximum accepted total plaintext size.
        limit: usize,
    },

    /// An attachment exceeded a plaintext size limit: the local one for an
    /// outgoing file, or what decryption accepts for an incoming one.
    #[error("attachment exceeds the {limit} byte limit")]
    AttachmentTooLarge {
        /// Maximum accepted plaintext size of one file.
        limit: usize,
    },

    /// An outgoing attachment is unavailable or is not a regular file.
    #[error("cannot read attachment {path}: {source}")]
    AttachmentUnavailable {
        /// The path selected by the sender.
        path: std::path::PathBuf,
        /// The filesystem error or invalid file type.
        #[source]
        source: std::io::Error,
    },

    /// The final send request may have succeeded, but its result was not confirmed.
    #[error(
        "could not confirm whether message {message_id} was sent; check Sent before retrying: {source}"
    )]
    SendUnconfirmed {
        /// ID of the draft submitted to Proton.
        message_id: String,
        /// Failure that prevented confirmation.
        #[source]
        source: Box<Error>,
    },

    /// JSON (de)serialization failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// An OpenPGP encryption, decryption, or key operation failed.
    #[error("crypto error: {0}")]
    Crypto(String),

    /// A present attachment signature could not be verified. No plaintext is
    /// released to the caller, including when sender keys are unavailable.
    #[error("attachment signature could not be verified; attachment was not released")]
    AttachmentVerificationFailed,

    /// An SRP authentication step failed.
    #[error("srp error: {0}")]
    Srp(String),

    /// A session load, save, or token-refresh operation failed.
    #[error("session error: {0}")]
    Session(String),

    /// The saved session names an API address other than the one it was
    /// created for, so its tokens are not sent there.
    #[error(
        "session error: saved session does not match the API address it was created for; sign in again"
    )]
    SessionAddressMismatch,

    /// The account has a separate mailbox password, and the login was given
    /// none and could not ask for one.
    #[error(
        "this account uses a separate mailbox password (PasswordMode 2); supply --mailbox-password"
    )]
    MailboxPasswordRequired,

    /// The account's second factor is a security key (FIDO2/WebAuthn), which
    /// is not supported.
    #[error(
        "this account requires a security key (FIDO2/WebAuthn) for 2FA, which is not yet supported — enable a TOTP authenticator app, or use an app/bridge password"
    )]
    SecurityKeyRequired,

    /// None of the account's user keys opened: a wrong mailbox password, or a
    /// saved session from before the password changed.
    #[error("crypto error: no user key could be unlocked (wrong password?)")]
    UserKeysLocked,

    /// A local cache read or write failed.
    #[error("cache error: {0}")]
    Cache(String),

    /// The cache, or Proton's listing, changed while an operation ran, for
    /// example because another process moved the sync cursor, so nothing was
    /// applied and trying again later may succeed. The text says what changed.
    #[error("cache error: {0}; retry")]
    CacheChanged(String),

    /// A filesystem or other I/O operation failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A referenced entity of the given kind was not found.
    #[error("{kind} not found")]
    NotFound {
        /// The kind of entity that was not found (e.g. `message`).
        kind: String,
    },

    /// A reference matched more than one entity (count of matches).
    #[error("ambiguous reference: {0} matches")]
    Ambiguous(usize),

    /// Any other error, carrying a message.
    #[error("{0}")]
    Other(String),
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.without_url())
    }
}

impl Error {
    /// Map this error to the CLI's process exit status: an API error by its
    /// HTTP status, and any other [`exit_status::OTHER`] unless it names a
    /// session, something not found, or a conflict.
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Api(e) => e.exit_code(),
            Error::Unauthorized => exit_status::SESSION,
            Error::NotFound { .. } => exit_status::NOT_FOUND,
            Error::Ambiguous(_) => exit_status::CONFLICT,
            _ => exit_status::OTHER,
        }
    }

    /// A cache error for an operation that found the cache, or Proton's
    /// listing, changed under it; `reason` says what changed.
    pub(crate) fn cache_changed(reason: &str) -> Self {
        Error::CacheChanged(reason.to_owned())
    }

    /// Whether this is a cache error that may pass if the operation is tried
    /// again later, because the cache changed while it ran.
    pub fn is_retryable_cache_error(&self) -> bool {
        matches!(self, Error::CacheChanged(_))
    }
}

/// Crate-wide `Result` alias using [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    fn challenge(web_url: &str, token: &str) -> HvChallenge {
        HvChallenge {
            token: token.into(),
            methods: vec!["captcha".into(), "email".into()],
            web_url: web_url.into(),
        }
    }

    #[test]
    fn the_verification_page_opens_only_on_protons_own_hosts() {
        assert_eq!(
            challenge("https://verify-api.proton.me/x?y=1", "t").verification_url(),
            "https://verify-api.proton.me/?methods=captcha&token=t"
        );
        assert_eq!(
            challenge("https://proton.me/", "t").verification_url(),
            "https://proton.me/?methods=captcha&token=t"
        );
        // The page receives the token, so any other host gets the default one.
        for untrusted in [
            "",
            "https://",
            "http://verify.proton.me/",
            "https://example.com/",
            "https://proton.me.example.com/",
            "https://evilproton.me/",
            "https://evil@verify.proton.me/",
            "https://x&calc&/",
            "https://evil host/",
            "https://a\"b/",
        ] {
            assert_eq!(
                challenge(untrusted, "t").verification_url(),
                "https://verify.proton.me/?methods=captcha&token=t",
                "{untrusted}"
            );
        }
    }

    #[test]
    fn the_verification_page_asks_for_the_captcha_with_the_token_encoded() {
        let page = challenge(
            "https://verify.proton.me/?methods=captcha,email&token=other",
            "chal/1 x",
        );
        assert_eq!(
            page.verification_url(),
            "https://verify.proton.me/?methods=captcha&token=chal%2F1%20x"
        );
    }

    #[test]
    fn a_captcha_answers_a_challenge_that_names_it_or_no_method() {
        for (methods, offered) in [
            (vec![], true),
            (vec!["captcha"], true),
            (vec!["email", "captcha"], true),
            (vec!["email", "sms"], false),
        ] {
            let named = HvChallenge {
                methods: methods.iter().map(|method| (*method).to_owned()).collect(),
                ..challenge("", "token")
            };
            assert_eq!(named.offers_captcha(), offered, "{methods:?}");
        }
    }

    #[test]
    fn http_conversion_removes_url_without_losing_error_kind_or_cause() {
        let url = reqwest::Url::parse(
            "https://user:private-password@example.com/messages?Keyword=private-search#private-fragment",
        )
        .unwrap();
        let original = reqwest::Client::new()
            .get("://invalid")
            .build()
            .unwrap_err()
            .with_url(url);
        assert!(original.is_builder());
        assert!(original.url().is_some());
        let cause = std::error::Error::source(&original).unwrap().to_string();
        let error = Error::from(original);
        let Error::Http(http) = &error else {
            panic!("expected HTTP error")
        };
        assert!(
            http.url().is_none(),
            "HTTP errors must not retain request URLs"
        );
        assert!(http.is_builder());
        assert_eq!(std::error::Error::source(http).unwrap().to_string(), cause);
        assert!(std::error::Error::source(&error).is_some());
        for rendered in [error.to_string(), format!("{error:?}")] {
            for secret in [
                "private-search",
                "private-password",
                "private-fragment",
                "example.com",
                "Keyword",
            ] {
                assert!(!rendered.contains(secret), "leaked URL data: {rendered}");
            }
        }
    }

    fn api(status: u16) -> ApiError {
        ApiError {
            http_status: status,
            code: 0,
            message: "x".into(),
            raw_body: String::new(),
        }
    }

    #[test]
    fn exit_codes_map_status() {
        assert_eq!(api(401).exit_code(), 2);
        assert_eq!(api(403).exit_code(), 2);
        assert_eq!(api(404).exit_code(), 3);
        assert_eq!(api(409).exit_code(), 4);
        assert_eq!(api(422).exit_code(), 4);
        assert_eq!(api(500).exit_code(), 5);
        assert_eq!(api(503).exit_code(), 5);
        assert_eq!(api(400).exit_code(), 1);
    }

    #[test]
    fn only_a_busy_or_failing_proton_may_pass() {
        for status in [429, 500, 503] {
            assert!(api(status).may_pass(), "{status}");
        }
        for status in [400, 401, 403, 404, 409, 422] {
            assert!(!api(status).may_pass(), "{status}");
        }
        assert!(api(401).is_unauthorized());
        assert!(!api(403).is_unauthorized());
    }

    #[test]
    fn error_exit_code_delegates() {
        assert_eq!(Error::Api(api(404)).exit_code(), 3);
        assert_eq!(Error::Unauthorized.exit_code(), 2);
        assert_eq!(
            Error::NotFound {
                kind: "message".into()
            }
            .exit_code(),
            3
        );
        assert_eq!(Error::Ambiguous(3).exit_code(), 4);
        assert_eq!(Error::Other("x".into()).exit_code(), 1);
    }

    #[test]
    fn only_a_cache_that_changed_under_an_operation_asks_to_be_tried_again() {
        let changed = Error::cache_changed("mailbox changed during resync");
        assert_eq!(
            changed.to_string(),
            "cache error: mailbox changed during resync; retry"
        );
        assert!(changed.is_retryable_cache_error());
        assert!(
            !Error::Cache("database disk image is malformed".into()).is_retryable_cache_error()
        );
        // The kind of error says so, whatever its text says.
        assert!(!Error::Cache("cursor moved; retry".into()).is_retryable_cache_error());
        assert!(!Error::Session("sync cursor changed; retry".into()).is_retryable_cache_error());
    }

    #[test]
    fn sign_in_failures_have_kinds_of_their_own_and_read_as_before() {
        for (error, text) in [
            (
                Error::MailboxPasswordRequired,
                "this account uses a separate mailbox password (PasswordMode 2); supply --mailbox-password",
            ),
            (
                Error::SecurityKeyRequired,
                "this account requires a security key (FIDO2/WebAuthn) for 2FA, which is not yet supported — enable a TOTP authenticator app, or use an app/bridge password",
            ),
            (
                Error::UserKeysLocked,
                "crypto error: no user key could be unlocked (wrong password?)",
            ),
            (
                Error::SessionAddressMismatch,
                "session error: saved session does not match the API address it was created for; sign in again",
            ),
        ] {
            assert_eq!(error.to_string(), text);
            assert_eq!(error.exit_code(), exit_status::OTHER);
        }
    }
}
