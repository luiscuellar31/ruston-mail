//! How the operations of this crate fail, and the exit status the CLI gives
//! each failure.

use crate::model::enums::human_verification;

/// What Proton asks a person to solve, with a `Code` of 9001, before it
/// answers a request.
#[derive(Debug, Clone)]
pub struct HvChallenge {
    /// The challenge's token, which the verification page is opened with.
    pub token: String,
    /// The ways Proton accepts to solve it, such as `captcha`, `email` or
    /// `sms`.
    pub methods: Vec<String>,
    /// The page Proton names for solving it. Only its host is used, and only
    /// when it is one of Proton's; see [`HvChallenge::verification_url`].
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
    /// A session that is missing, expired or refused. Its own status, apart
    /// from the 2 argument parsers such as clap give a usage error, so a
    /// script can tell that signing in again would help.
    pub const SESSION: i32 = 6;
    /// Something the request named was not found.
    pub const NOT_FOUND: i32 = 3;
    /// An ambiguous reference, or a conflict Proton reports.
    pub const CONFLICT: i32 = 4;
    /// A Proton server error.
    pub const SERVER: i32 = 5;
}

/// An answer in which Proton refused a request: an error `Code`, or a
/// failing HTTP status. It reads as the status, the code and Proton's
/// message.
#[derive(Debug, Clone, thiserror::Error)]
#[error("[HTTP {http_status}] {code}: {message}")]
pub struct ApiError {
    /// The answer's HTTP status.
    pub http_status: u16,
    /// The `Code` Proton gave, or 0 when it gave none.
    pub code: i64,
    /// What went wrong, for a person to read: Proton's `Error` text when it
    /// sent one.
    pub message: String,
    /// The start of the raw response body, up to 4 KiB, kept for diagnostics.
    pub raw_body: String,
}

impl ApiError {
    /// Map to the CLI's process exit status, one of [`exit_status`].
    pub fn exit_code(&self) -> i32 {
        match self.http_status {
            _ if self.refuses_session() => exit_status::SESSION,
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

    /// Whether Proton refused the session, or the credentials that sign in
    /// or open one: HTTP 401 or 403. Signing in and opening a session end
    /// there; in the middle of one, only a 401 does, once renewing the tokens
    /// fails ([`is_unauthorized`](Self::is_unauthorized)), since a 403 there
    /// can refuse one action alone.
    pub fn refuses_session(&self) -> bool {
        matches!(self.http_status, 401 | 403)
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

/// Every way an operation of this crate can fail.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Proton refused the request; it reads as Proton's answer.
    #[error(transparent)]
    Api(#[from] ApiError),

    /// There is no saved session, or Proton no longer accepts it and it
    /// could not be renewed: signing in again is needed.
    #[error("unauthorized (session expired or invalid)")]
    Unauthorized,

    /// Proton asks for proof that a person made the request before it
    /// answers it.
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

    /// Reading or writing JSON failed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Encrypting, decrypting, signing or unlocking a key failed; the text
    /// says at which step.
    #[error("crypto error: {0}")]
    Crypto(String),

    /// A present attachment signature could not be verified. No plaintext is
    /// released to the caller, including when sender keys are unavailable.
    #[error("attachment signature could not be verified; attachment was not released")]
    AttachmentVerificationFailed,

    /// An SRP step failed: making the password proof or a verifier, hashing
    /// a password, or checking the server's proof.
    #[error("srp error: {0}")]
    Srp(String),

    /// Saving, loading, renewing or removing the session failed, in its
    /// files, its profile's lock or the credential store.
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

    /// The local cache could not be opened, read or written, or belongs to
    /// another account.
    #[error("cache error: {0}")]
    Cache(String),

    /// The cache, or Proton's listing, changed while an operation ran, for
    /// example because another process moved the sync cursor, so nothing was
    /// applied and trying again later may succeed. The text says what changed.
    #[error("cache error: {0}; retry")]
    CacheChanged(String),

    /// Reading or writing a file, or other I/O, failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// What the request named does not exist.
    #[error("{kind} not found")]
    NotFound {
        /// What kind of thing it is, such as `message`.
        kind: String,
    },

    /// A reference fits more than one item; it holds how many.
    #[error("ambiguous reference: {0} matches")]
    Ambiguous(usize),

    /// Anything else, as its text says.
    #[error("{0}")]
    Other(String),
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.without_url())
    }
}

impl Error {
    /// Whether the same request may succeed later: the connection failed, or
    /// Proton was busy or down ([`ApiError::may_pass`]), rather than refusing
    /// the request or the session.
    pub fn may_pass(&self) -> bool {
        match self {
            Error::Http(_) => true,
            Error::Api(api) => api.may_pass(),
            _ => false,
        }
    }

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

/// What this crate's operations return: their value, or the [`Error`] that
/// kept them from it.
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
        // 2 is clap's, for a usage error.
        let statuses = [401, 403, 404, 409, 422, 500, 503, 400];
        let exit_codes = statuses.map(|status| api(status).exit_code());
        assert_eq!(exit_codes, [6, 6, 3, 4, 4, 5, 5, 1]);
    }

    #[test]
    fn an_api_error_reads_as_its_status_code_and_message_alone() {
        let refused = ApiError {
            http_status: 422,
            code: 2001,
            message: "Invalid input".into(),
            raw_body: r#"{"Code":2001,"Error":"Invalid input"}"#.into(),
        };
        assert_eq!(refused.to_string(), "[HTTP 422] 2001: Invalid input");
        assert!(std::error::Error::source(&refused).is_none());
        let wrapped = Error::from(refused);
        assert_eq!(wrapped.to_string(), "[HTTP 422] 2001: Invalid input");
        assert!(std::error::Error::source(&wrapped).is_none());
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
        assert!(Error::Api(api(503)).may_pass() && !Error::Api(api(401)).may_pass());
        assert!(!Error::Other("refused".into()).may_pass());
        assert!(api(401).refuses_session() && api(403).refuses_session());
        for status in [400, 404, 429, 500] {
            assert!(!api(status).refuses_session(), "{status}");
        }
    }

    #[test]
    fn error_exit_code_delegates() {
        let missing = Error::NotFound {
            kind: "message".into(),
        };
        let errors = [
            Error::Api(api(404)),
            Error::Unauthorized,
            missing,
            Error::Ambiguous(3),
            Error::Other("x".into()),
        ];
        assert_eq!(errors.map(|error| error.exit_code()), [3, 6, 3, 4, 1]);
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
