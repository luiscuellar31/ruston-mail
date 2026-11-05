//! The HTTP transport: requests to Proton's API, each answer judged by its
//! envelope, and a request sent again after a 401 renews the tokens, after
//! the wait Proton asks for when busy (429, or 503 to a read), or once a
//! human-verification challenge (9001) is solved.

mod headers;
mod retry_after;

use crate::error::{ApiError, Error, HvChallenge, Result};
use crate::model::enums::response_code;
use crate::session::Tokens;
use async_trait::async_trait;
use futures::future::BoxFuture;
use reqwest::Method;
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use std::fs::File;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};
use zeroize::Zeroizing;

/// Maximum response body for ordinary API requests (32 MiB).
pub const MAX_API_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
/// Total deadline for an ordinary HTTP request, including its response body.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest wait to open a connection, within any request's own deadline.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Redirects one request may follow, as many as reqwest allows by default.
const MAX_REDIRECTS: usize = 10;
/// How much of an error body `ApiError::raw_body` keeps.
const MAX_ERROR_BODY_BYTES: usize = 4 * 1024;
/// How many times one request may renew its tokens after a 401. Tokens
/// renewed meanwhile by another request or process may have expired too, so
/// reusing them leaves one more refresh; a pair Proton has just issued is the
/// last try.
const REFRESH_ATTEMPTS: u8 = 2;
/// How many times a request Proton answers busy is sent again, each after
/// the wait [`retry_after::wait`] gives: as go-proton-api, which retries 429
/// and 503 answers three times; Proton's web client tries five times in all,
/// on 429 alone. A 429 refuses a request before Proton handles it, so any
/// request is sent again after one; see [`busy_and_repeatable`] for a 503.
const BUSY_RETRIES: u8 = 3;

/// What a request sends after its headers.
#[derive(Clone)]
pub enum Body {
    /// Nothing.
    Empty,
    /// A JSON value, sent as its text.
    Json(serde_json::Value),
    /// Bytes sent as they are, under the content type [`Request::raw`] gives.
    Bytes(Vec<u8>),
}

// Bodies carry refresh tokens, password proofs and session keys, so their
// debug form names the kind of body, never its content.
impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("Empty"),
            Self::Json(_) => f.write_str("Json(..)"),
            Self::Bytes(bytes) => write!(f, "Bytes({} bytes)", bytes.len()),
        }
    }
}

/// One API call: where it goes, what it sends, and how the transport is to
/// treat it.
#[derive(Clone)]
pub struct Request {
    /// The HTTP method.
    pub method: Method,
    /// Where under the client's base URL it goes, such as `/core/v4/users`.
    pub path: String,
    /// Query parameters by name, sent in this order.
    pub query: Vec<(String, String)>,
    /// What it sends.
    pub body: Body,
    /// The `Content-Type` to declare instead of JSON's, as a raw body needs.
    pub content_type: Option<String>,
    /// A solved human-verification token and its type, sent as headers.
    pub hv: Option<(String, String)>,
    /// Whether a 401 comes back as it is instead of renewing the tokens, as
    /// sign-in needs; [`Request::no_refresh`] sets it.
    pub skip_refresh: bool,
    /// Whether `Authorization` is left out, as the token refresh needs.
    pub omit_auth: bool,
    /// Whether `x-enforce-unauthsession: true` is sent, as creating a session
    /// needs.
    pub enforce_unauth: bool,
    /// Maximum response body size, including responses from retries.
    pub max_response_bytes: usize,
    /// Total deadline for each HTTP attempt, including response-body reading.
    pub timeout: Duration,
}

// The human-verification token stays out of the debug form, as the body does.
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("path", &self.path)
            .field("query", &self.query)
            .field("body", &self.body)
            .field("content_type", &self.content_type)
            .field("hv_type", &self.hv.as_ref().map(|(_, kind)| kind))
            .field("skip_refresh", &self.skip_refresh)
            .field("omit_auth", &self.omit_auth)
            .field("enforce_unauth", &self.enforce_unauth)
            .field("max_response_bytes", &self.max_response_bytes)
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl Request {
    /// A request of `method` to `path` that sends nothing yet, with the
    /// ordinary response limit and deadline.
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Request {
            method,
            path: path.into(),
            query: Vec::new(),
            body: Body::Empty,
            content_type: None,
            hv: None,
            skip_refresh: false,
            omit_auth: false,
            enforce_unauth: false,
            max_response_bytes: MAX_API_RESPONSE_BYTES,
            timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
    /// A `GET` of `path`.
    pub fn get(path: impl Into<String>) -> Self {
        Self::new(Method::GET, path)
    }
    /// A `POST` to `path`.
    pub fn post(path: impl Into<String>) -> Self {
        Self::new(Method::POST, path)
    }
    /// A `PUT` to `path`.
    pub fn put(path: impl Into<String>) -> Self {
        Self::new(Method::PUT, path)
    }
    /// A `DELETE` of `path`.
    pub fn delete(path: impl Into<String>) -> Self {
        Self::new(Method::DELETE, path)
    }
    /// This request, sending `body` as JSON.
    pub fn json(self, body: serde_json::Value) -> Self {
        Self {
            body: Body::Json(body),
            ..self
        }
    }
    /// This request, sending `bytes` as they are, declared as `content_type`.
    pub fn raw(self, bytes: Vec<u8>, content_type: impl Into<String>) -> Self {
        Self {
            body: Body::Bytes(bytes),
            content_type: Some(content_type.into()),
            ..self
        }
    }
    /// This request, with the query parameter `k` set to `v` after those
    /// already set.
    pub fn query<K: Into<String>, V: Into<String>>(mut self, k: K, v: V) -> Self {
        self.query.push((k.into(), v.into()));
        self
    }
    /// This request, carrying the solved human-verification `token` of the
    /// type `ttype`.
    pub fn with_hv(self, token: impl Into<String>, ttype: impl Into<String>) -> Self {
        Self {
            hv: Some((token.into(), ttype.into())),
            ..self
        }
    }
    /// This request, returning a 401 as it is instead of renewing the tokens;
    /// see [`Request::skip_refresh`].
    pub fn no_refresh(self) -> Self {
        Self {
            skip_refresh: true,
            ..self
        }
    }
    /// This request, without the `Authorization` header.
    pub fn omit_auth(self) -> Self {
        Self {
            omit_auth: true,
            ..self
        }
    }
    /// This request, sending `x-enforce-unauthsession: true`.
    pub fn enforce_unauth(self) -> Self {
        Self {
            enforce_unauth: true,
            ..self
        }
    }
    /// This request, accepting at most `limit` bytes of response body.
    pub fn max_response_bytes(self, limit: usize) -> Self {
        Self {
            max_response_bytes: limit,
            ..self
        }
    }
    /// This request, with `timeout` as the whole deadline of each HTTP attempt.
    pub fn timeout(self, timeout: Duration) -> Self {
        Self { timeout, ..self }
    }

    /// Where this request goes on the API at `base_url`: its path, then each
    /// query parameter in order, its name and value percent-encoded.
    fn url(&self, base_url: &str) -> String {
        let mut url = format!("{base_url}{}", self.path);
        for (i, (name, value)) in self.query.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(&urlencoding::encode(name));
            url.push('=');
            url.push_str(&urlencoding::encode(value));
        }
        url
    }
}

/// What the API answered.
#[derive(Debug, Clone)]
pub struct Response {
    /// The HTTP status.
    pub status: u16,
    /// The body, as it came.
    pub body: Vec<u8>,
    /// How many seconds its `Retry-After` header asks to wait, when it has
    /// one.
    pub retry_after: Option<u64>,
}

/// The session that requests are made under. Clients made with
/// [`HttpClient::with_state`] share one, so tokens a refresh renews reach
/// every one of them.
#[derive(Debug, Clone)]
pub struct AuthState {
    /// The session's ID, sent as `x-pm-uid` once signed in.
    pub uid: Option<String>,
    /// The token each request presents once signed in.
    pub access: Option<SecretString>,
    /// The token that renews `access` when Proton refuses it.
    pub refresh: Option<SecretString>,
    /// The client name and version every request gives in `x-pm-appversion`.
    pub app_version: String,
    /// The `User-Agent` every request sends, if one is set.
    pub user_agent: Option<String>,
}

impl AuthState {
    /// The state before sign-in: no session, only the client's version.
    pub fn unauthenticated(app_version: impl Into<String>) -> Self {
        let app_version = app_version.into();
        Self {
            uid: None,
            access: None,
            refresh: None,
            app_version,
            user_agent: None,
        }
    }
}

/// What is handed the new pair of tokens each time a refresh succeeds. It
/// borrows them, so saving them needs no copy in the clear — use
/// [`FallibleRefreshPersist`] when storage can fail.
pub type RefreshPersist = Arc<dyn Fn(&Tokens) + Send + Sync>;
/// A token-refresh callback whose persistence failure is returned to the request.
pub type FallibleRefreshPersist = Arc<dyn Fn(&Tokens) -> Result<()> + Send + Sync>;
type RefreshLoad = Arc<dyn Fn() -> Result<(File, Option<Tokens>)> + Send + Sync>;
/// Solves a human-verification challenge, giving the token to send the
/// request again with, and its type.
pub type HvResolver =
    Arc<dyn Fn(HvChallenge) -> BoxFuture<'static, Result<(String, String)>> + Send + Sync>;

/// What the API layer sends its requests through.
#[async_trait]
pub trait Doer: Send + Sync {
    /// Sends `req` and returns the answer as it came, once any retries are
    /// done and its envelope reads as a success.
    async fn do_raw(&self, req: Request) -> Result<Response>;
    /// Sends `req` and reads the answer's body as a `T`.
    async fn decode<T: DeserializeOwned>(&self, req: Request) -> Result<T>;
}

/// The [`Doer`] that sends requests to the API with `reqwest`.
pub struct HttpClient {
    client: reqwest::Client,
    base_url: String,
    auth: Arc<RwLock<AuthState>>,
    on_refresh: Option<FallibleRefreshPersist>,
    refresh_load: Option<RefreshLoad>,
    refresh_unsaved: Arc<AtomicBool>,
    hv: Option<HvResolver>,
}

impl HttpClient {
    /// A client for the API at `base_url`, not signed in, that names itself
    /// `app_version`.
    ///
    /// # Panics
    ///
    /// When the HTTP client cannot be built, as on a system without root
    /// certificates. [`HttpClient::try_new`] returns that as an error.
    pub fn new(base_url: impl Into<String>, app_version: impl Into<String>) -> Self {
        Self::try_new(base_url, app_version).expect("reqwest client")
    }

    /// Like [`HttpClient::new`], but a client that cannot be built, as on a
    /// system without root certificates, is an [`Error::Http`].
    pub fn try_new(base_url: impl Into<String>, app_version: impl Into<String>) -> Result<Self> {
        Self::try_with_state(
            base_url,
            Arc::new(RwLock::new(AuthState::unauthenticated(app_version))),
        )
    }

    /// A client for the API at `base_url` under the session `auth`, shared
    /// with the clients made with it, so tokens one of them renews reach all.
    ///
    /// # Panics
    ///
    /// When the HTTP client cannot be built; see [`HttpClient::try_with_state`].
    pub fn with_state(base_url: impl Into<String>, auth: Arc<RwLock<AuthState>>) -> Self {
        Self::try_with_state(base_url, auth).expect("reqwest client")
    }

    /// Like [`HttpClient::with_state`], but a client that cannot be built is
    /// an [`Error::Http`].
    pub fn try_with_state(
        base_url: impl Into<String>,
        auth: Arc<RwLock<AuthState>>,
    ) -> Result<Self> {
        let base_url = base_url.into();
        // Responses arrive gzip-compressed when Proton offers it, and the
        // system's proxy settings apply. An unreachable host fails after
        // CONNECT_TIMEOUT instead of using up the whole request deadline.
        // Requests stay with the API: a redirect elsewhere would carry their
        // body, refresh token or two-factor code along, and plain HTTP is
        // accepted only for a server on this machine.
        let client = reqwest::Client::builder()
            .cookie_store(true)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(same_origin_redirects())
            .referer(false)
            .https_only(!is_loopback(&base_url))
            .build()?;
        Ok(HttpClient {
            client,
            base_url,
            auth,
            on_refresh: None,
            refresh_load: None,
            refresh_unsaved: Arc::new(AtomicBool::new(false)),
            hv: None,
        })
    }

    /// The session this client sends requests under, shared, so the holder
    /// sees the tokens a refresh renews.
    pub fn auth_state(&self) -> Arc<RwLock<AuthState>> {
        self.auth.clone()
    }

    /// Waits until no token refresh is under way. A refresh holds the
    /// tokens until the new pair is stored, on a task of its own that a
    /// program ending right away would cut short, losing the session. That
    /// task then saves the pair without yielding, which a program returning
    /// from `main` lets finish; one ending at once, as on Ctrl+C, waits with
    /// [`crate::Client::settle_session`] instead.
    pub async fn settled(&self) {
        drop(self.auth.read().await);
    }
    /// A client for requests made after the saved session is gone. It shares
    /// connections and cookies and starts from a copy of the current tokens;
    /// a 401 refreshes them in memory only, never loading or saving the
    /// profile's session.
    pub(crate) async fn detached(&self) -> HttpClient {
        let auth = self.auth.read().await.clone();
        HttpClient {
            client: self.client.clone(),
            base_url: self.base_url.clone(),
            auth: Arc::new(RwLock::new(auth)),
            on_refresh: None,
            refresh_load: None,
            refresh_unsaved: Arc::new(AtomicBool::new(false)),
            hv: None,
        }
    }
    /// Has each refresh hand its new pair of tokens to `cb`, to be saved.
    pub fn set_refresh_persist(&mut self, cb: RefreshPersist) {
        self.on_refresh = Some(Arc::new(move |tokens| {
            cb(tokens);
            Ok(())
        }));
    }
    /// Register a callback whose persistence error aborts the triggering request.
    /// Rotated tokens remain available in memory when that callback fails.
    pub fn set_refresh_persist_fallible(&mut self, cb: FallibleRefreshPersist) {
        self.on_refresh = Some(cb);
    }
    /// Coordinate a refresh with another process using the same saved session.
    pub(crate) fn set_refresh_load(&mut self, cb: RefreshLoad) {
        self.refresh_load = Some(cb);
    }
    /// Sets what solves a human-verification challenge (code 9001), so the
    /// request is sent again with the answer. Without one, the challenge
    /// is the request's error.
    pub fn set_hv_resolver(&mut self, cb: HvResolver) {
        self.hv = Some(cb);
    }

    /// Has the requests that follow use the session `uid` and its pair of
    /// tokens.
    pub async fn set_tokens(&self, uid: String, access: SecretString, refresh: SecretString) {
        let mut auth = self.auth.write().await;
        auth.uid = Some(uid);
        auth.access = Some(access);
        auth.refresh = Some(refresh);
    }

    /// Sends `ua` as the `User-Agent` of every request.
    pub async fn set_user_agent(&self, ua: String) {
        self.auth.write().await.user_agent = Some(ua);
    }

    /// Move byte buffers into reqwest once. Its buffered requests share their
    /// body allocation when cloned for authentication, rate-limit or HV retries.
    /// A JSON body leaves `req` as it is written out, so the value, which can
    /// hold session keys, is not kept through those retries.
    fn prepare_request(&self, req: &mut Request) -> Result<reqwest::Request> {
        let builder = self
            .client
            .request(req.method.clone(), req.url(&self.base_url))
            .timeout(req.timeout);
        let builder = match &mut req.body {
            Body::Empty => builder,
            Body::Json(v) => builder.body(serde_json::to_vec(&std::mem::take(v))?),
            Body::Bytes(bytes) => builder.body(std::mem::take(bytes)),
        };
        Ok(builder.build()?)
    }

    async fn execute_once(
        client: &reqwest::Client,
        req: &Request,
        prepared: &reqwest::Request,
        auth: &AuthState,
    ) -> Result<Response> {
        let hdrs = headers::build_headers(auth, req);
        let body_kind = match &req.body {
            Body::Empty => "empty",
            Body::Json(_) => "json",
            Body::Bytes(_) => "bytes",
        };
        // Borrow the parsed path: never log queries, fragments or URL credentials,
        // including ones supplied directly in Request.path or the base URL.
        tracing::debug!(target: "ruston_core::http", method = %req.method, path = prepared.url().path(), body = body_kind, hv = req.hv.is_some(), "→ request");
        tracing::trace!(target: "ruston_core::http", headers = ?hdrs.keys().map(|k| k.as_str()).collect::<Vec<_>>(), "request headers");

        let mut attempt = prepared
            .try_clone()
            .ok_or_else(|| Error::Other("request body cannot be replayed".into()))?;
        *attempt.headers_mut() = hdrs;
        let started = std::time::Instant::now();
        let sent = std::time::SystemTime::now();
        let resp = client.execute(attempt).await?;
        // Every response comes from the API, whose clock OpenPGP keeps to.
        if let Some(date) = header_text(&resp, reqwest::header::DATE) {
            crate::crypto::clock::note_proton_date(date, sent, std::time::SystemTime::now());
        }
        let status = resp.status().as_u16();
        let retry_after =
            header_text(&resp, reqwest::header::RETRY_AFTER).and_then(retry_after::parse);
        let body = read_bounded_body(resp, req.max_response_bytes).await?;
        tracing::debug!(target: "ruston_core::http", status, bytes = body.len(), ms = started.elapsed().as_millis() as u64, "← response");
        Ok(Response {
            status,
            body,
            retry_after,
        })
    }

    /// Renews the session's tokens with `POST /auth/v4/refresh`, unless
    /// another request or process already replaced the ones `sent_auth` holds.
    async fn refresh_tokens(&self, sent_auth: &AuthState) -> Result<Refresh> {
        // Keep the check, refresh request, and token replacement under the
        // shared state lock so concurrent 401s reuse a successful rotation.
        let mut a = self.auth.clone().write_owned().await;
        if a.uid != sent_auth.uid
            || a.access.as_ref().map(|t| t.expose_secret())
                != sent_auth.access.as_ref().map(|t| t.expose_secret())
            || a.refresh.as_ref().map(|t| t.expose_secret())
                != sent_auth.refresh.as_ref().map(|t| t.expose_secret())
        {
            return Ok(Refresh::Reused);
        }
        // Hold the cross-process lock until the rotated pair is persisted.
        // Another process may have refreshed while this client was idle.
        let session_lock = if let Some(load) = &self.refresh_load {
            let load = load.clone();
            let (lock, saved) = tokio::task::spawn_blocking(move || load())
                .await
                .map_err(|e| Error::Session(format!("refresh lock task failed: {e}")))??;
            let saved = saved.ok_or(Error::Unauthorized)?;
            if a.uid.as_deref() != Some(saved.uid.as_str()) {
                return Err(Error::Unauthorized);
            }
            if !self.refresh_unsaved.load(Ordering::Acquire)
                && (a.access.as_ref().map(|t| t.expose_secret())
                    != Some(saved.access.expose_secret())
                    || a.refresh.as_ref().map(|t| t.expose_secret())
                        != Some(saved.refresh.expose_secret()))
            {
                a.access = Some(saved.access);
                a.refresh = Some(saved.refresh);
                return Ok(Refresh::Reused);
            }
            Some(lock)
        } else {
            None
        };
        let (Some(uid), Some(refresh)) = (a.uid.clone(), a.refresh.as_ref()) else {
            return Err(Error::Unauthorized);
        };
        let body = serde_json::json!({
            "UID": uid,
            "RefreshToken": refresh.expose_secret(),
            "ResponseType": "token",
            "GrantType": "refresh_token",
            "RedirectURI": "https://protonmail.ch",
            "State": "0",
        });
        let mut req = Request::post("/auth/v4/refresh")
            .json(body)
            .no_refresh()
            .omit_auth();
        let prepared = self.prepare_request(&mut req)?;
        // Once sent, the old refresh token is spent. The rotation finishes on
        // a task of its own, holding both locks, so cancelling the request
        // that needed it cannot lose the new pair before it is stored and saved.
        let rotation = Rotation {
            client: self.client.clone(),
            on_refresh: self.on_refresh.clone(),
            refresh_unsaved: self.refresh_unsaved.clone(),
        };
        tokio::spawn(rotation.run(a, session_lock, req, prepared, uid))
            .await
            .map_err(|e| Error::Session(format!("token refresh task failed: {e}")))??;
        Ok(Refresh::Rotated)
    }
}

/// How [`HttpClient::refresh_tokens`] replaced the tokens a refused request
/// sent.
enum Refresh {
    /// Proton issued a new pair.
    Rotated,
    /// Another request or process had already replaced them, and that pair
    /// may have expired since.
    Reused,
}

/// The part of a token refresh that must not be cancelled: sending it,
/// storing the rotated pair and saving it.
struct Rotation {
    client: reqwest::Client,
    on_refresh: Option<FallibleRefreshPersist>,
    refresh_unsaved: Arc<AtomicBool>,
}

impl Rotation {
    async fn run(
        self,
        mut a: OwnedRwLockWriteGuard<AuthState>,
        _session_lock: Option<File>,
        req: Request,
        prepared: reqwest::Request,
        uid: String,
    ) -> Result<()> {
        let resp = HttpClient::execute_once(&self.client, &req, &prepared, &a).await?;
        match detect(resp.status, &resp.body) {
            Detected::Ok => {}
            // An outage, a rate limit or a challenge leaves the saved session
            // valid; only Proton refusing the refresh token ends it.
            Detected::Api(error) if error.may_pass() => {
                return Err(Error::Api(error));
            }
            Detected::Hv(challenge) => return Err(Error::HumanVerification(challenge)),
            Detected::Api(_) => return Err(Error::Unauthorized),
        }
        #[derive(serde::Deserialize)]
        struct RefreshResp {
            #[serde(rename = "AccessToken")]
            access_token: String,
            #[serde(rename = "RefreshToken")]
            refresh_token: String,
        }
        // The body holds the pair in the clear: it is wiped once read, and
        // the tokens move into secrets rather than being copied.
        let body = Zeroizing::new(resp.body);
        let parsed: RefreshResp = serde_json::from_slice(&body)?;
        let tokens = Tokens {
            uid,
            access: SecretString::from(parsed.access_token),
            refresh: SecretString::from(parsed.refresh_token),
        };
        a.access = Some(tokens.access.clone());
        a.refresh = Some(tokens.refresh.clone());
        if self.on_refresh.is_some() {
            self.refresh_unsaved.store(true, Ordering::Release);
        }
        drop(a);
        if let Some(cb) = &self.on_refresh {
            cb(&tokens)?;
            self.refresh_unsaved.store(false, Ordering::Release);
        }
        Ok(())
    }
}

/// The value of `response`'s header `name`, when it has one in visible ASCII.
fn header_text(response: &reqwest::Response, name: reqwest::header::HeaderName) -> Option<&str> {
    response.headers().get(name)?.to_str().ok()
}

async fn read_bounded_body(mut resp: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let stated = resp.content_length();
    if stated.is_some_and(|len| len > limit as u64) {
        return Err(Error::ResponseTooLarge { limit });
    }
    // A stated length is within the limit here, so it bounds the reservation.
    // A compressed body states none and grows as it arrives.
    let capacity = stated.and_then(|len| usize::try_from(len).ok());
    let mut body = Vec::with_capacity(capacity.unwrap_or(0));
    while let Some(chunk) = resp.chunk().await? {
        if chunk.len() > limit.saturating_sub(body.len()) {
            return Err(Error::ResponseTooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// What a response's envelope says about it.
enum Detected {
    Ok,
    Api(ApiError),
    Hv(HvChallenge),
}

/// The envelope fields of a Proton response. The rest of the body is skipped
/// without being built, so a large response is not copied into a tree only
/// to read its code; the caller decodes what it needs once.
#[derive(serde::Deserialize)]
struct Envelope {
    /// Any JSON value, so a `Code` of another type reads as no code instead
    /// of failing the whole envelope, and `Error` and `Details` with it.
    #[serde(rename = "Code")]
    code: Option<serde_json::Value>,
    #[serde(rename = "Error")]
    error: Option<serde_json::Value>,
    #[serde(rename = "Details")]
    details: Option<serde_json::Value>,
}

fn detect(status: u16, body: &[u8]) -> Detected {
    let envelope: Option<Envelope> = serde_json::from_slice(body).ok();
    let envelope = envelope.as_ref();
    let code = envelope
        .and_then(|envelope| envelope.code.as_ref())
        .and_then(serde_json::Value::as_i64);

    // A challenge comes with a 422, or with a 200 while signing in.
    if code == Some(response_code::HUMAN_VERIFICATION)
        && let Some(details) = envelope.and_then(|envelope| envelope.details.as_ref())
    {
        return Detected::Hv(hv_challenge(details));
    }
    if is_success(status, code) {
        return Detected::Ok;
    }

    let message = envelope
        .and_then(|envelope| envelope.error.as_ref())
        .and_then(serde_json::Value::as_str)
        .unwrap_or("request failed")
        .to_string();
    Detected::Api(ApiError {
        http_status: status,
        code: code.unwrap_or(response_code::NONE),
        message,
        raw_body: String::from_utf8_lossy(&body[..body.len().min(MAX_ERROR_BODY_BYTES)])
            .into_owned(),
    })
}

/// Whether a response succeeded: a 2xx whose `Code`, if it has one, is a
/// success or none.
fn is_success(status: u16, code: Option<i64>) -> bool {
    (200..300).contains(&status)
        && matches!(
            code,
            None | Some(
                response_code::NONE | response_code::SUCCESS | response_code::MULTI_SUCCESS
            )
        )
}

/// Whether Proton's `error` says it was busy with `request`, which may then
/// be sent again: a 429, or a 503 to a read. A gateway may answer 503 once
/// Proton has acted on a request, so anything that is not a read, a send
/// above all, is never sent twice after one.
fn busy_and_repeatable(request: &Request, error: &ApiError) -> bool {
    error.is_rate_limited() || (error.http_status == 503 && request.method.is_safe())
}

/// The challenge a 9001 response's `Details` describe. A field they lack
/// reads as empty.
fn hv_challenge(details: &serde_json::Value) -> HvChallenge {
    let text = |key: &str| {
        details
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    HvChallenge {
        token: text("HumanVerificationToken"),
        methods: details
            .get("HumanVerificationMethods")
            .and_then(serde_json::Value::as_array)
            .map(|methods| {
                methods
                    .iter()
                    .filter_map(|method| method.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        web_url: text("WebUrl"),
    }
}

#[async_trait]
impl Doer for HttpClient {
    async fn do_raw(&self, mut request: Request) -> Result<Response> {
        // The request is sent again after renewing the tokens on a 401, up to
        // REFRESH_ATTEMPTS times; after a busy answer, up to BUSY_RETRIES
        // times; and after one solved human verification.
        let prepared = self.prepare_request(&mut request)?;
        let mut refreshes_left = REFRESH_ATTEMPTS;
        let mut busy_retries_left = BUSY_RETRIES;
        let mut hv_tried = false;

        loop {
            let sent_auth = self.auth.read().await.clone();
            let resp = Self::execute_once(&self.client, &request, &prepared, &sent_auth).await?;
            match detect(resp.status, &resp.body) {
                Detected::Ok => return Ok(resp),
                Detected::Api(e)
                    if e.is_unauthorized() && !request.skip_refresh && refreshes_left > 0 =>
                {
                    tracing::debug!(target: "ruston_core::http", "401 — refreshing access token and retrying");
                    refreshes_left = match self.refresh_tokens(&sent_auth).await? {
                        Refresh::Rotated => 0,
                        Refresh::Reused => refreshes_left - 1,
                    };
                }
                Detected::Api(e) if e.is_unauthorized() => return Err(Error::Unauthorized),
                Detected::Api(e) if busy_retries_left > 0 && busy_and_repeatable(&request, &e) => {
                    let Some(wait) = retry_after::wait(resp.retry_after) else {
                        return Err(Error::Api(e));
                    };
                    tracing::debug!(target: "ruston_core::http", status = e.http_status, wait_s = wait.as_secs(), "busy — backing off then retrying");
                    if !wait.is_zero() {
                        tokio::time::sleep(wait).await;
                    }
                    busy_retries_left -= 1;
                }
                Detected::Hv(chal) => {
                    let Some(resolver) = self.hv.as_ref().filter(|_| !hv_tried) else {
                        return Err(Error::HumanVerification(chal));
                    };
                    tracing::info!(target: "ruston_core::auth", methods = ?chal.methods, "9001 human verification required — invoking resolver");
                    let (tok, ttype) = resolver(chal).await?;
                    tracing::info!(target: "ruston_core::auth", token_type = %ttype, "human verification token obtained — retrying");
                    request.hv = Some((tok, ttype));
                    hv_tried = true;
                }
                Detected::Api(e) => return Err(Error::Api(e)),
            }
        }
    }

    async fn decode<T: DeserializeOwned>(&self, req: Request) -> Result<T> {
        let resp = self.do_raw(req).await?;
        Ok(serde_json::from_slice(&resp.body)?)
    }
}

/// Follows a redirect only within the origin its request started at.
fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let start = attempt.previous().first();
        if start.is_some_and(|start| start.origin() != attempt.url().origin()) {
            attempt.error("redirect away from the API")
        } else if attempt.previous().len() > MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    })
}

/// Whether `base_url` names this machine, where plain HTTP crosses no network.
fn is_loopback(base_url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base_url) else {
        return false;
    };
    url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    })
}

#[cfg(test)]
mod tests;
