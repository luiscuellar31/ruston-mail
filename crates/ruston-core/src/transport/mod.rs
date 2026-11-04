//! The HTTP transport: requests to Proton's API, each answer judged by its
//! envelope, and a request sent again after a 401 renews the tokens, after a
//! 429's wait, or once a human-verification challenge (9001) is solved.

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
    /// one, cut to the longest wait the transport honors.
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
        // REFRESH_ATTEMPTS times; after one wait on a 429; and after one
        // solved human verification.
        let prepared = self.prepare_request(&mut request)?;
        let mut refreshes_left = REFRESH_ATTEMPTS;
        let mut rate_limited = false;
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
                Detected::Api(e) if e.is_rate_limited() && !rate_limited => {
                    let wait = resp.retry_after.unwrap_or(0);
                    tracing::debug!(target: "ruston_core::http", wait_s = wait, "429 rate-limited — backing off then retrying");
                    if wait > 0 {
                        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    }
                    rate_limited = true;
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
mod tests {
    use super::*;
    use crate::session::{MemoryStore, Paths, SecretStore, Session};
    use serde::Deserialize;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(base: &str) -> HttpClient {
        HttpClient::new(base, "Other")
    }

    #[tokio::test]
    async fn settled_outlasts_a_request_cut_short_during_its_refresh() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(serde_json::json!({"Code":401,"Error":"Invalid access token"})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!(
                        {"Code":1000,"AccessToken":"new-acc","RefreshToken":"new-ref"}
                    ))
                    .set_delay(Duration::from_millis(300)),
            )
            .mount(&server)
            .await;
        let c = Arc::new(client(&server.uri()));
        c.set_tokens(
            "uid".into(),
            SecretString::from("old-acc"),
            SecretString::from("old-ref"),
        )
        .await;
        let request = tokio::spawn({
            let c = c.clone();
            async move { c.decode::<ValueResp>(Request::get("/protected")).await }
        });
        // Once the refresh is on its way, the request is cut short, as
        // quitting a program would.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let received = server.received_requests().await.unwrap_or_default();
                if received.iter().any(|r| r.url.path() == "/auth/v4/refresh") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the refresh was sent");
        request.abort();
        tokio::time::timeout(Duration::from_secs(5), c.settled())
            .await
            .expect("settled once the refresh ended");
        let auth = c.auth.read().await;
        assert_eq!(
            auth.refresh.as_ref().map(|t| t.expose_secret().to_owned()),
            Some("new-ref".to_owned())
        );
    }

    #[tokio::test]
    async fn settled_waits_for_a_token_refresh_under_way() {
        let c = client("http://127.0.0.1:9");
        // As a refresh holds the tokens until it has saved the new pair.
        let refreshing = c.auth.clone().write_owned().await;
        let settled = c.settled();
        tokio::pin!(settled);
        let waited = tokio::time::timeout(Duration::from_millis(50), &mut settled).await;
        assert!(waited.is_err(), "settled before the refresh ended");
        drop(refreshing);
        tokio::time::timeout(Duration::from_secs(5), settled)
            .await
            .expect("settled once the refresh ended");
    }

    #[test]
    fn a_request_debug_form_shows_neither_its_body_nor_its_verification_token() {
        let request = Request::post("/auth/v4/refresh")
            .json(serde_json::json!({"RefreshToken": "refresh-secret"}))
            .with_hv("hv-secret", "captcha");
        let shown = format!("{request:?}");
        assert!(shown.contains("/auth/v4/refresh"), "{shown}");
        assert!(shown.contains("Json(..)"), "{shown}");
        assert!(shown.contains("captcha"), "{shown}");
        assert!(!shown.contains("secret"), "{shown}");
        let raw = format!("{:?}", Body::Bytes(b"session-key-secret".to_vec()));
        assert_eq!(raw, "Bytes(18 bytes)");
    }

    async fn read_get_request(socket: &mut tokio::net::TcpStream) {
        // Closing with unread headers can reset TCP and discard the response,
        // making a body-limit test fail with an unrelated connection error.
        let mut reader = tokio::io::BufReader::new(socket);
        let mut headers = String::new();
        while !headers.ends_with("\r\n\r\n") {
            assert_ne!(
                reader.read_line(&mut headers).await.unwrap(),
                0,
                "client closed before sending complete headers"
            );
        }
        assert!(headers.starts_with("GET /x HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn a_redirect_away_from_the_api_is_refused() {
        let api = MockServer::start().await;
        // Another port is another origin, as another host would be.
        let elsewhere = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/core/v4/users"))
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("Location", format!("{}/collect", elsewhere.uri())),
            )
            .mount(&api)
            .await;

        let error = client(&api.uri())
            .do_raw(Request::get("/core/v4/users"))
            .await
            .unwrap_err();

        assert!(
            matches!(&error, Error::Http(http) if http.is_redirect()),
            "{error}"
        );
        assert!(elsewhere.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_redirect_within_the_api_is_followed_without_a_referer() {
        let api = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/old"))
            .respond_with(ResponseTemplate::new(307).insert_header("Location", "/new"))
            .mount(&api)
            .await;
        Mock::given(method("GET"))
            .and(path("/new"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .mount(&api)
            .await;

        client(&api.uri())
            .do_raw(Request::get("/old").query("Keyword", "private-search"))
            .await
            .unwrap();

        let requests = api.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].headers.get("referer").is_none());
    }

    #[tokio::test]
    async fn plain_http_is_refused_unless_the_api_is_on_this_machine() {
        // Refused before any connection or name lookup is attempted.
        let error = client("http://api.example.test")
            .do_raw(Request::get("/core/v4/users"))
            .await
            .unwrap_err();
        assert!(
            matches!(&error, Error::Http(http) if http.is_builder()),
            "{error}"
        );

        for local in [
            "http://127.0.0.1:8080",
            "http://[::1]:80",
            "http://localhost",
        ] {
            assert!(is_loopback(local), "{local}");
        }
        for remote in [
            "http://api.example.test",
            "https://mail.proton.me/api",
            "nonsense",
        ] {
            assert!(!is_loopback(remote), "{remote}");
        }
    }

    #[test]
    fn tls_offers_the_post_quantum_hybrid_key_exchange_first() {
        // reqwest builds its TLS configuration from this provider.
        let provider = rustls::crypto::aws_lc_rs::default_provider();
        assert_eq!(
            provider.kx_groups[0].name(),
            rustls::NamedGroup::X25519MLKEM768
        );
    }

    #[tokio::test]
    async fn http_timeouts_omit_request_url_in_error_and_source() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/messages"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(1)))
            .mount(&server)
            .await;
        let error = client(&server.uri())
            .do_raw(
                Request::get("/messages")
                    .query("Keyword", "private-search")
                    .timeout(Duration::from_millis(20)),
            )
            .await
            .unwrap_err();
        let Error::Http(http) = &error else {
            panic!("expected HTTP timeout")
        };
        assert!(http.is_timeout());
        assert!(http.url().is_none());
        for rendered in [
            error.to_string(),
            format!("{error:?}"),
            std::error::Error::source(&error).unwrap().to_string(),
        ] {
            assert!(!rendered.contains("private-search"));
            assert!(!rendered.contains("Keyword"));
            assert!(!rendered.contains(&server.uri()));
        }
    }

    #[derive(Deserialize, Debug)]
    struct ValueResp {
        #[serde(rename = "Value")]
        value: i64,
    }

    #[test]
    fn buffered_requests_move_and_share_raw_bytes_for_retries() {
        let bytes = vec![0x7f; 64 * 1024];
        let pointer = bytes.as_ptr();
        let mut request = Request::post("/upload").raw(bytes, "application/octet-stream");
        let prepared = client("http://127.0.0.1")
            .prepare_request(&mut request)
            .unwrap();
        assert!(matches!(request.body, Body::Bytes(ref bytes) if bytes.is_empty()));
        assert_eq!(
            prepared.body().unwrap().as_bytes().unwrap().as_ptr(),
            pointer
        );
        for _ in 0..3 {
            let retry = prepared.try_clone().unwrap();
            let data = retry.body().unwrap().as_bytes().unwrap();
            assert_eq!(data.len(), 64 * 1024);
            assert_eq!(data.as_ptr(), pointer);
        }
    }

    #[test]
    fn a_json_body_is_kept_only_as_the_bytes_retries_share() {
        let mut request =
            Request::post("/mail/v4/messages/send").json(serde_json::json!({"Key": "session"}));
        let prepared = client("http://127.0.0.1")
            .prepare_request(&mut request)
            .unwrap();
        // The value is gone; what goes out stays, for every retry.
        assert!(matches!(request.body, Body::Json(serde_json::Value::Null)));
        let sent = prepared.body().unwrap().as_bytes().unwrap();
        assert_eq!(sent, br#"{"Key":"session"}"#);
        let retry = prepared.try_clone().unwrap();
        assert_eq!(
            retry.body().unwrap().as_bytes().unwrap().as_ptr(),
            sent.as_ptr()
        );
    }

    #[tokio::test]
    async fn responses_are_requested_and_read_compressed() {
        // `{"Code":1000,"Note":"zzz…"}` with 200 `z`s: 223 bytes, gzip-compressed to 45.
        const GZIPPED: [u8; 45] = [
            31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 171, 86, 114, 206, 79, 73, 85, 178, 50, 52, 48, 48,
            208, 81, 242, 203, 47, 1, 178, 149, 170, 134, 9, 80, 170, 5, 0, 159, 209, 189, 8, 223,
            0, 0, 0,
        ];
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/compressed"))
            .and(header("accept-encoding", "gzip"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-encoding", "gzip")
                    .set_body_raw(GZIPPED.to_vec(), "application/json"),
            )
            .mount(&server)
            .await;

        #[derive(Deserialize)]
        struct Note {
            #[serde(rename = "Note")]
            note: String,
        }
        let http = HttpClient::new(server.uri(), "Other");
        let body: Note = http.decode(Request::get("/compressed")).await.unwrap();
        assert_eq!(body.note, "z".repeat(200));

        // Limits count what the body expands to, not what crossed the wire.
        assert!(matches!(
            http.do_raw(Request::get("/compressed").max_response_bytes(100))
                .await,
            Err(Error::ResponseTooLarge { limit: 100 })
        ));
    }

    #[tokio::test]
    async fn raw_body_and_content_type_survive_rate_limit_retry() {
        use std::sync::atomic::AtomicUsize;
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let bytes = vec![0x7f; 64 * 1024];
        let expected = bytes.clone();
        Mock::given(method("POST"))
            .and(path("/upload"))
            .and(header("content-type", "application/octet-stream"))
            .respond_with(move |request: &wiremock::Request| {
                assert_eq!(request.body, expected);
                if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                    ResponseTemplate::new(429).set_body_json(serde_json::json!({"Code": 429}))
                } else {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000}))
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        client(&server.uri())
            .do_raw(Request::post("/upload").raw(bytes, "application/octet-stream"))
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_response_is_judged_by_its_envelope_alone() {
        for success in [
            &b"not json"[..],
            br#"[1, 2]"#,
            br#"{"Code": 1000, "Message": {"Body": "-----BEGIN PGP MESSAGE-----"}}"#,
            br#"{"Code": 1001, "Responses": [{"Code": 1000}]}"#,
            br#"{"Code": 0}"#,
        ] {
            assert!(matches!(detect(200, success), Detected::Ok));
        }
        let Detected::Api(error) = detect(
            422,
            br#"{"Code": 2001, "Error": "Invalid input", "Details": {"Field": "Email"}}"#,
        ) else {
            panic!("expected an API error");
        };
        assert_eq!(
            (error.code, error.message.as_str()),
            (2001, "Invalid input")
        );

        // An error page keeps only its start for diagnostics.
        let page = vec![b'x'; 1024 * 1024];
        let Detected::Api(error) = detect(502, &page) else {
            panic!("expected an API error");
        };
        assert_eq!(error.raw_body.len(), MAX_ERROR_BODY_BYTES);
        assert_eq!(error.message, "request failed");
    }

    #[test]
    fn only_a_2xx_succeeds_and_only_a_detailed_challenge_asks_for_verification() {
        // A success code does not make an error status a success.
        for code in [1000, 1001, 0] {
            let body = format!(r#"{{"Code": {code}}}"#);
            let Detected::Api(error) = detect(500, body.as_bytes()) else {
                panic!("a 500 with Code {code} succeeded");
            };
            assert_eq!((error.http_status, error.code), (500, code));
        }
        // A code that is not a number counts as no code.
        assert!(matches!(detect(200, br#"{"Code": "1000"}"#), Detected::Ok));
        // A challenge without its details is an ordinary error.
        let Detected::Api(error) = detect(
            422,
            br#"{"Code": 9001, "Error": "human verification required"}"#,
        ) else {
            panic!("a challenge without details asked for verification");
        };
        assert_eq!(error.code, 9001);
        // Details it lacks read as empty.
        let Detected::Hv(challenge) = detect(422, br#"{"Code": 9001, "Details": {}}"#) else {
            panic!("expected a challenge");
        };
        assert_eq!(
            (challenge.token.as_str(), challenge.web_url.as_str()),
            ("", "")
        );
        assert!(challenge.methods.is_empty());
    }

    #[tokio::test]
    async fn a_client_built_without_panicking_works_like_any_other() {
        // A system without root certificates fails to build one; that is an
        // error for sign-in and resume to report, never a panic.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code": 1000, "Value": 7})),
            )
            .mount(&server)
            .await;
        let client = HttpClient::try_new(server.uri(), "Other").unwrap();
        let value: ValueResp = client.decode(Request::get("/x")).await.unwrap();
        assert_eq!(value.value, 7);
    }

    /// Proton's success envelope around `{"Value": value}`.
    fn success(value: i64) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000, "Value": value}))
    }

    /// A server giving every `GET /x` the answer `respond` makes.
    async fn answering(respond: impl wiremock::Respond + 'static) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(respond)
            .mount(&server)
            .await;
        server
    }

    /// Gives each request the next of `answers`, and those after the last
    /// the last again.
    fn in_turn<const N: usize>(answers: [ResponseTemplate; N]) -> impl wiremock::Respond {
        let served = std::sync::atomic::AtomicUsize::new(0);
        move |_: &wiremock::Request| {
            let turn = served.fetch_add(1, Ordering::SeqCst);
            answers[turn.min(N - 1)].clone()
        }
    }

    #[tokio::test]
    async fn a_success_envelope_is_decoded_with_the_rest_of_the_body() {
        let server = answering(success(42)).await;
        let decoded: ValueResp = client(&server.uri())
            .decode(Request::get("/x"))
            .await
            .unwrap();
        assert_eq!(decoded.value, 42);
    }

    #[test]
    fn query_parameters_are_percent_encoded_in_the_order_they_were_set() {
        let search = Request::get("/mail/v4/messages")
            .query("Keyword", "a b&c=d")
            .query("LabelID", "x/y+z")
            .query("Page", "0");
        assert_eq!(
            search.url("https://mail.proton.me/api"),
            "https://mail.proton.me/api/mail/v4/messages?Keyword=a%20b%26c%3Dd&LabelID=x%2Fy%2Bz&Page=0"
        );
        assert_eq!(
            Request::get("/core/v4/users").url("http://127.0.0.1"),
            "http://127.0.0.1/core/v4/users"
        );
    }

    #[tokio::test]
    async fn response_limit_accepts_boundary_and_rejects_larger_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"abcd".to_vec()))
            .mount(&server)
            .await;
        let c = client(&server.uri());
        assert_eq!(
            c.do_raw(Request::get("/x").max_response_bytes(4))
                .await
                .unwrap()
                .body,
            b"abcd"
        );
        assert!(matches!(
            c.do_raw(Request::get("/x").max_response_bytes(3)).await,
            Err(Error::ResponseTooLarge { limit: 3 })
        ));
    }

    #[tokio::test]
    async fn a_stated_body_length_is_reserved_before_reading() {
        let server = MockServer::start().await;
        // Not a power of two, so a buffer grown by doubling cannot end at it.
        let bytes = vec![b'x'; 100_003];
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
            .mount(&server)
            .await;
        let body = client(&server.uri())
            .do_raw(Request::get("/x"))
            .await
            .unwrap()
            .body;
        assert_eq!(body, bytes);
        assert_eq!(body.capacity(), body.len());
    }

    #[tokio::test]
    async fn response_limit_rejects_chunked_body_without_content_length() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (release, held) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_get_request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n")
                .await
                .unwrap();
            // Body limits must be enforced while the connection is still open.
            let _ = held.await;
        });

        let c = client(&format!("http://{addr}"));
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            c.do_raw(Request::get("/x").max_response_bytes(5)),
        )
        .await;
        let _ = release.send(());
        server.await.unwrap();
        let result = result.expect("body limit should be enforced before the connection closes");
        assert!(
            matches!(result, Err(Error::ResponseTooLarge { limit: 5 })),
            "expected body limit error, got {result:?}"
        );
    }

    #[tokio::test]
    async fn request_deadline_covers_stalled_response_body() {
        assert_eq!(Request::get("/x").timeout, DEFAULT_REQUEST_TIMEOUT);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (release, held) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_get_request(&mut socket).await;
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\na")
                .await
                .unwrap();
            let _ = held.await;
        });

        let result = tokio::time::timeout(
            Duration::from_secs(2),
            client(&format!("http://{addr}"))
                .do_raw(Request::get("/x").timeout(Duration::from_millis(50))),
        )
        .await
        .expect("transport should enforce its own request deadline");
        let _ = release.send(());
        server.await.unwrap();
        assert!(matches!(result, Err(Error::Http(error)) if error.is_timeout()));
    }

    #[tokio::test]
    async fn an_error_code_is_returned_with_proton_s_message() {
        let refusal = ResponseTemplate::new(200)
            .set_body_json(serde_json::json!({"Code": 2501, "Error": "nope"}));
        let server = answering(refusal).await;
        let result = client(&server.uri())
            .decode::<ValueResp>(Request::get("/x"))
            .await;
        let Err(Error::Api(error)) = &result else {
            panic!("expected an API error, got {result:?}");
        };
        assert_eq!(
            (error.http_status, error.code, error.message.as_str()),
            (200, 2501, "nope")
        );
    }

    #[tokio::test]
    async fn sends_app_version_and_user_agent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .and(wiremock::matchers::header("x-pm-appversion", "Other"))
            .and(wiremock::matchers::header(
                "user-agent",
                "ruston-cli/9.9.9 (testos)",
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code":1000,"Value":1})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let c = client(&server.uri());
        c.set_user_agent("ruston-cli/9.9.9 (testos)".into()).await;
        let v: ValueResp = c.decode(Request::get("/x")).await.unwrap();
        assert_eq!(v.value, 1);
    }

    #[tokio::test]
    async fn a_rate_limited_request_is_sent_once_more() {
        let limited = ResponseTemplate::new(429).insert_header("Retry-After", "0");
        let server = answering(in_turn([limited, success(7)])).await;
        let decoded: ValueResp = client(&server.uri())
            .decode(Request::get("/x"))
            .await
            .unwrap();
        assert_eq!(decoded.value, 7);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_expired_access_token_is_renewed_and_the_request_sent_again() {
        use wiremock::matchers::body_partial_json;
        let server = MockServer::start().await;
        refuse(&server, "old-acc", 1, Duration::ZERO).await;
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .and(body_partial_json(
                serde_json::json!({"UID": "uid", "RefreshToken": "old-ref"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000,
                "AccessToken": "new-acc",
                "RefreshToken": "new-ref",
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer new-acc"))
            .respond_with(success(99))
            .expect(1)
            .mount(&server)
            .await;
        let http = client(&server.uri());
        http.set_tokens(
            "uid".into(),
            SecretString::from("old-acc"),
            SecretString::from("old-ref"),
        )
        .await;

        let decoded: ValueResp = http.decode(Request::get("/protected")).await.unwrap();

        assert_eq!(decoded.value, 99);
        server.verify().await;
        let auth = http.auth.read().await;
        let held = |token: &Option<SecretString>| {
            token.as_ref().map(|token| token.expose_secret().to_owned())
        };
        assert_eq!(
            (held(&auth.access), held(&auth.refresh)),
            (Some("new-acc".to_owned()), Some("new-ref".to_owned()))
        );
    }

    #[tokio::test]
    async fn refresh_persistence_failure_reaches_the_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({"Code":401})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"Code":1000,"AccessToken":"new-acc","RefreshToken":"new-ref"}),
            ))
            .expect(1)
            .mount(&server)
            .await;

        let mut c = client(&server.uri());
        c.set_tokens(
            "uid".into(),
            SecretString::from("old-acc"),
            SecretString::from("old-ref"),
        )
        .await;
        c.set_refresh_persist_fallible(Arc::new(|_| {
            Err(Error::Session("keyring unavailable".into()))
        }));
        let err = c
            .decode::<ValueResp>(Request::get("/protected"))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Session(message) if message == "keyring unavailable"));
        let state = c.auth.read().await;
        assert_eq!(state.access.as_ref().unwrap().expose_secret(), "new-acc");
        assert_eq!(state.refresh.as_ref().unwrap().expose_secret(), "new-ref");
    }

    #[tokio::test]
    async fn failed_persistence_keeps_newer_in_memory_tokens_for_next_refresh() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer old-acc"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({"Code":401})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .and(body_json(serde_json::json!({
                "UID":"uid", "RefreshToken":"old-ref", "ResponseType":"token",
                "GrantType":"refresh_token", "RedirectURI":"https://protonmail.ch", "State":"0"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"Code":1000,"AccessToken":"first-acc","RefreshToken":"first-ref"}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer first-acc"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({"Code":401})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .and(body_json(serde_json::json!({
                "UID":"uid", "RefreshToken":"first-ref", "ResponseType":"token",
                "GrantType":"refresh_token", "RedirectURI":"https://protonmail.ch", "State":"0"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"Code":1000,"AccessToken":"second-acc","RefreshToken":"second-ref"}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer second-acc"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code":1000,"Value":99})),
            )
            .expect(1)
            .mount(&server)
            .await;

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "ruston-unsaved-refresh-{}-{unique}",
            std::process::id()
        ));
        let paths = Paths::with_base(&base);
        let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
        Session {
            uid: "uid".into(),
            app_version: "Other".into(),
            base_url: server.uri(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        }
        .save(
            &paths,
            "ruston",
            store.as_ref(),
            &Tokens {
                uid: "uid".into(),
                access: SecretString::from("old-acc"),
                refresh: SecretString::from("old-ref"),
            },
            &SecretString::from("skp"),
        )
        .unwrap();

        let mut client = client(&server.uri());
        client
            .set_tokens(
                "uid".into(),
                SecretString::from("old-acc"),
                SecretString::from("old-ref"),
            )
            .await;
        crate::mail::Client::wire_refresh(&mut client, store.clone(), paths.clone(), "ruston");
        // Only saving is made to fail; loading stays the client's own.
        client.set_refresh_persist_fallible(Arc::new(|_| {
            Err(Error::Session("keyring unavailable".into()))
        }));
        assert!(matches!(
            client.decode::<ValueResp>(Request::get("/protected")).await,
            Err(Error::Session(_))
        ));
        crate::mail::Client::wire_refresh(&mut client, store.clone(), paths.clone(), "ruston");

        let result: ValueResp = client.decode(Request::get("/protected")).await.unwrap();
        assert_eq!(result.value, 99);
        server.verify().await;
        assert_eq!(
            Session::load(&paths, "ruston", store.as_ref())
                .unwrap()
                .unwrap()
                .tokens
                .refresh
                .expose_secret(),
            "second-ref"
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn a_request_cancelled_mid_refresh_keeps_the_rotated_tokens() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer old-acc"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(serde_json::json!({"Code": 401})),
            )
            .mount(&server)
            .await;
        let requested = Arc::new(tokio::sync::Notify::new());
        let received = requested.clone();
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .respond_with(move |_: &wiremock::Request| {
                received.notify_one();
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({
                        "Code": 1000,
                        "AccessToken": "new-acc",
                        "RefreshToken": "new-ref",
                    }))
                    .set_delay(Duration::from_millis(200))
            })
            .expect(1)
            .mount(&server)
            .await;
        let mut http = HttpClient::new(server.uri(), "Other");
        let saved = Arc::new(std::sync::Mutex::new(None));
        let saves = saved.clone();
        http.set_refresh_persist_fallible(Arc::new(move |tokens: &Tokens| {
            *saves.lock().unwrap() = Some((
                tokens.access.expose_secret().to_owned(),
                tokens.refresh.expose_secret().to_owned(),
            ));
            Ok(())
        }));
        http.set_tokens(
            "uid".into(),
            SecretString::from("old-acc"),
            SecretString::from("old-ref"),
        )
        .await;

        {
            let request = http.do_raw(Request::get("/protected"));
            tokio::pin!(request);
            tokio::select! {
                result = &mut request => panic!("the refresh should be in flight: {result:?}"),
                _ = requested.notified() => {}
            }
            // The caller gives up after Proton has spent the old refresh token.
        }

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while saved.lock().unwrap().is_none() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            saved.lock().unwrap().clone(),
            Some(("new-acc".to_owned(), "new-ref".to_owned()))
        );
        let state = http.auth_state();
        let state = state.read().await;
        assert_eq!(state.access.as_ref().unwrap().expose_secret(), "new-acc");
        assert_eq!(state.refresh.as_ref().unwrap().expose_secret(), "new-ref");
        server.verify().await;
    }

    // Two workers: the slow save blocks one while the test waits on the other.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn waiting_for_the_profile_outlasts_a_refresh_until_its_tokens_are_saved() {
        /// A store whose writes take a while once `slow` is set, as a
        /// keychain's can.
        #[derive(Default)]
        struct SlowStore {
            inner: MemoryStore,
            slow: std::sync::atomic::AtomicBool,
        }
        impl SecretStore for SlowStore {
            fn set(&self, key: &str, value: &str) -> Result<()> {
                if self.slow.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(300));
                }
                self.inner.set(key, value)
            }
            fn get(&self, key: &str) -> Result<Option<String>> {
                self.inner.get(key)
            }
            fn delete(&self, key: &str) -> Result<()> {
                self.inner.delete(key)
            }
        }

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer old-acc"))
            .respond_with(
                ResponseTemplate::new(401).set_body_json(serde_json::json!({"Code": 401})),
            )
            .mount(&server)
            .await;
        let requested = Arc::new(tokio::sync::Notify::new());
        let received = requested.clone();
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .respond_with(move |_: &wiremock::Request| {
                received.notify_one();
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "Code": 1000,
                    "AccessToken": "new-acc",
                    "RefreshToken": "new-ref",
                }))
            })
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer new-acc"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code":1000,"Value":99})),
            )
            .mount(&server)
            .await;

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "ruston-settle-session-{}-{unique}",
            std::process::id()
        ));
        let paths = Paths::with_base(&base);
        let slow_store = Arc::new(SlowStore::default());
        let store: Arc<dyn SecretStore> = slow_store.clone();
        Session {
            uid: "uid".into(),
            app_version: "Other".into(),
            base_url: server.uri(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        }
        .save(
            &paths,
            "ruston",
            store.as_ref(),
            &Tokens {
                uid: "uid".into(),
                access: SecretString::from("old-acc"),
                refresh: SecretString::from("old-ref"),
            },
            &SecretString::from("skp"),
        )
        .unwrap();
        slow_store
            .slow
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let mut client = client(&server.uri());
        client
            .set_tokens(
                "uid".into(),
                SecretString::from("old-acc"),
                SecretString::from("old-ref"),
            )
            .await;
        crate::mail::Client::wire_refresh(&mut client, store.clone(), paths.clone(), "ruston");

        let request =
            tokio::spawn(
                async move { client.decode::<ValueResp>(Request::get("/protected")).await },
            );
        requested.notified().await;
        // As Ctrl+C would, once Proton has spent the old refresh token.
        let waiting = paths.clone();
        tokio::task::spawn_blocking(move || Session::wait_unlocked(&waiting, "ruston"))
            .await
            .unwrap()
            .unwrap();
        let saved = Session::load(&paths, "ruston", store.as_ref())
            .unwrap()
            .unwrap();
        assert_eq!(saved.tokens.refresh.expose_secret(), "new-ref");
        assert_eq!(request.await.unwrap().unwrap().value, 99);
        server.verify().await;
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn concurrent_401s_refresh_shared_session_once() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer old-acc"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(serde_json::json!({"Code":401,"Error":"Invalid access token"}))
                    .set_delay(std::time::Duration::from_millis(200)),
            )
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .and(body_json(serde_json::json!({
                "UID": "uid",
                "RefreshToken": "old-ref",
                "ResponseType": "token",
                "GrantType": "refresh_token",
                "RedirectURI": "https://protonmail.ch",
                "State": "0"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"Code":1000,"AccessToken":"new-acc","RefreshToken":"new-ref"}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer new-acc"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code":1000,"Value":99})),
            )
            .expect(2)
            .mount(&server)
            .await;

        let auth = Arc::new(RwLock::new(AuthState::unauthenticated("Other")));
        let first = HttpClient::with_state(server.uri(), auth.clone());
        let second = HttpClient::with_state(server.uri(), auth.clone());
        first
            .set_tokens(
                "uid".into(),
                SecretString::from("old-acc"),
                SecretString::from("old-ref"),
            )
            .await;

        let (a, b) = tokio::join!(
            first.decode::<ValueResp>(Request::get("/protected")),
            second.decode::<ValueResp>(Request::get("/protected")),
        );
        assert_eq!(a.unwrap().value, 99);
        assert_eq!(b.unwrap().value, 99);
        let state = auth.read().await;
        assert_eq!(state.access.as_ref().unwrap().expose_secret(), "new-acc");
        assert_eq!(state.refresh.as_ref().unwrap().expose_secret(), "new-ref");
    }

    #[tokio::test]
    async fn separate_clients_reuse_rotated_tokens_from_shared_session() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer old-acc"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(serde_json::json!({"Code":401}))
                    .set_delay(std::time::Duration::from_millis(100)),
            )
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"Code":1000,"AccessToken":"new-acc","RefreshToken":"new-ref"}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer new-acc"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code":1000,"Value":99})),
            )
            .expect(2)
            .mount(&server)
            .await;

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "ruston-shared-refresh-{}-{unique}",
            std::process::id()
        ));
        let paths = Paths::with_base(&base);
        let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
        Session {
            uid: "uid".into(),
            app_version: "Other".into(),
            base_url: server.uri(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        }
        .save(
            &paths,
            "ruston",
            store.as_ref(),
            &Tokens {
                uid: "uid".into(),
                access: SecretString::from("old-acc"),
                refresh: SecretString::from("old-ref"),
            },
            &SecretString::from("skp"),
        )
        .unwrap();

        let mut clients = Vec::new();
        for _ in 0..2 {
            let mut client = client(&server.uri());
            client
                .set_tokens(
                    "uid".into(),
                    SecretString::from("old-acc"),
                    SecretString::from("old-ref"),
                )
                .await;
            crate::mail::Client::wire_refresh(&mut client, store.clone(), paths.clone(), "ruston");
            clients.push(client);
        }

        let (first, second) = tokio::join!(
            clients[0].decode::<ValueResp>(Request::get("/protected")),
            clients[1].decode::<ValueResp>(Request::get("/protected")),
        );
        assert_eq!(first.unwrap().value, 99);
        assert_eq!(second.unwrap().value, 99);
        server.verify().await;
        assert_eq!(
            Session::load(&paths, "ruston", store.as_ref())
                .unwrap()
                .unwrap()
                .tokens
                .refresh
                .expose_secret(),
            "new-ref"
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    /// A client still holding `old-acc` and `old-ref` while its saved session
    /// already holds the pair another process rotated to, `cli-acc` and
    /// `cli-ref`, as after the machine slept. Returns the folder to remove.
    async fn client_behind_its_saved_session(
        server: &MockServer,
    ) -> (HttpClient, Paths, Arc<dyn SecretStore>, std::path::PathBuf) {
        // Tests run in parallel and the clock can read the same for two of
        // them, so a counter keeps each directory to itself.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "ruston-adopted-refresh-{}-{unique}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let paths = Paths::with_base(&base);
        let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
        Session {
            uid: "uid".into(),
            app_version: "Other".into(),
            base_url: server.uri(),
            password_mode: 1,
            user_agent: None,
            user_id: None,
        }
        .save(
            &paths,
            "ruston",
            store.as_ref(),
            &Tokens {
                uid: "uid".into(),
                access: SecretString::from("cli-acc"),
                refresh: SecretString::from("cli-ref"),
            },
            &SecretString::from("skp"),
        )
        .unwrap();
        let mut client = client(&server.uri());
        client
            .set_tokens(
                "uid".into(),
                SecretString::from("old-acc"),
                SecretString::from("old-ref"),
            )
            .await;
        crate::mail::Client::wire_refresh(&mut client, store.clone(), paths.clone(), "ruston");
        (client, paths, store, base)
    }

    /// Answers `token` with 401 `times` times, each after `delay`.
    async fn refuse(server: &MockServer, token: &str, times: u64, delay: std::time::Duration) {
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", format!("Bearer {token}").as_str()))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(serde_json::json!({"Code":401}))
                    .set_delay(delay),
            )
            .expect(times)
            .mount(server)
            .await;
    }

    /// Accepts `cli-ref` once, rotating it to `new-acc` and `new-ref`.
    async fn rotate_saved_refresh_token(server: &MockServer) {
        use wiremock::matchers::body_partial_json;
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .and(body_partial_json(
                serde_json::json!({"RefreshToken": "cli-ref"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"Code":1000,"AccessToken":"new-acc","RefreshToken":"new-ref"}),
            ))
            .expect(1)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn saved_tokens_that_have_also_expired_are_refreshed() {
        let server = MockServer::start().await;
        refuse(&server, "old-acc", 1, std::time::Duration::ZERO).await;
        refuse(&server, "cli-acc", 1, std::time::Duration::ZERO).await;
        rotate_saved_refresh_token(&server).await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer new-acc"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code":1000,"Value":99})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let (client, paths, store, base) = client_behind_its_saved_session(&server).await;

        let result = client.decode::<ValueResp>(Request::get("/protected")).await;

        let saved = Session::load(&paths, "ruston", store.as_ref())
            .unwrap()
            .unwrap()
            .tokens
            .refresh
            .expose_secret()
            .to_owned();
        std::fs::remove_dir_all(base).unwrap();
        assert_eq!(result.unwrap().value, 99);
        server.verify().await;
        assert_eq!(saved, "new-ref");
    }

    #[tokio::test]
    async fn a_request_asks_proton_for_new_tokens_at_most_once() {
        // Even the pair Proton has just issued is refused.
        let server = MockServer::start().await;
        refuse(&server, "old-acc", 1, std::time::Duration::ZERO).await;
        refuse(&server, "cli-acc", 1, std::time::Duration::ZERO).await;
        rotate_saved_refresh_token(&server).await;
        refuse(&server, "new-acc", 1, std::time::Duration::ZERO).await;
        let (client, _, _, base) = client_behind_its_saved_session(&server).await;

        let result = client.decode::<ValueResp>(Request::get("/protected")).await;

        std::fs::remove_dir_all(base).unwrap();
        assert!(matches!(result, Err(Error::Unauthorized)));
        server.verify().await;
        assert_eq!(server.received_requests().await.unwrap().len(), 4);

        // A refused refresh token ends the session at once.
        let server = MockServer::start().await;
        refuse(&server, "old-acc", 1, std::time::Duration::ZERO).await;
        refuse(&server, "cli-acc", 1, std::time::Duration::ZERO).await;
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .respond_with(
                ResponseTemplate::new(422).set_body_json(serde_json::json!({"Code":10013})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let (client, _, _, base) = client_behind_its_saved_session(&server).await;

        let result = client.decode::<ValueResp>(Request::get("/protected")).await;

        std::fs::remove_dir_all(base).unwrap();
        assert!(matches!(result, Err(Error::Unauthorized)));
        server.verify().await;
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn concurrent_requests_share_one_refresh_of_expired_saved_tokens() {
        let server = MockServer::start().await;
        // The delays keep both requests in flight before either refreshes.
        let delay = std::time::Duration::from_millis(200);
        refuse(&server, "old-acc", 2, delay).await;
        refuse(&server, "cli-acc", 2, delay).await;
        rotate_saved_refresh_token(&server).await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer new-acc"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code":1000,"Value":99})),
            )
            .expect(2)
            .mount(&server)
            .await;
        let (client, _, _, base) = client_behind_its_saved_session(&server).await;

        let (first, second) = tokio::join!(
            client.decode::<ValueResp>(Request::get("/protected")),
            client.decode::<ValueResp>(Request::get("/protected")),
        );

        std::fs::remove_dir_all(base).unwrap();
        assert_eq!(first.unwrap().value, 99);
        assert_eq!(second.unwrap().value, 99);
        server.verify().await;
    }

    #[tokio::test]
    async fn a_refused_refresh_token_ends_the_session() {
        let server = MockServer::start().await;
        refuse(&server, "a", 1, Duration::ZERO).await;
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .respond_with(
                ResponseTemplate::new(422).set_body_json(serde_json::json!({"Code": 10013})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let http = client(&server.uri());
        http.set_tokens(
            "uid".into(),
            SecretString::from("a"),
            SecretString::from("r"),
        )
        .await;

        let result = http.decode::<ValueResp>(Request::get("/protected")).await;

        assert!(matches!(result, Err(Error::Unauthorized)), "{result:?}");
        server.verify().await;
    }

    #[tokio::test]
    async fn a_refresh_that_fails_for_a_passing_reason_keeps_the_session() {
        let challenge = serde_json::json!({
            "Code": 9001,
            "Details": {
                "HumanVerificationToken": "t",
                "HumanVerificationMethods": ["captcha"],
                "WebUrl": "https://verify.proton.me"
            }
        });
        for (status, body) in [
            (
                503,
                serde_json::json!({"Code": 503, "Error": "maintenance"}),
            ),
            (502, serde_json::json!({})),
            (429, serde_json::json!({"Code": 2028})),
            (422, challenge),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/protected"))
                .respond_with(
                    ResponseTemplate::new(401).set_body_json(serde_json::json!({"Code":401})),
                )
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/auth/v4/refresh"))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .mount(&server)
                .await;
            let c = client(&server.uri());
            c.set_tokens(
                "uid".into(),
                SecretString::from("a"),
                SecretString::from("r"),
            )
            .await;
            let err = c
                .decode::<ValueResp>(Request::get("/protected"))
                .await
                .unwrap_err();
            match err {
                Error::Api(api) => assert_eq!(api.http_status, status),
                Error::HumanVerification(_) => assert_eq!(status, 422),
                other => panic!("HTTP {status} on refresh ended the session: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_solved_challenge_goes_with_the_request_sent_again() {
        let server = MockServer::start().await;
        // Only the request that carries the answer gets through.
        Mock::given(method("GET"))
            .and(path("/x"))
            .and(header("x-pm-human-verification-token", "hv-1:resp"))
            .and(header("x-pm-human-verification-token-type", "captcha"))
            .respond_with(success(77))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(human_verification_required())
            .mount(&server)
            .await;
        let challenged = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = challenged.clone();
        let mut http = client(&server.uri());
        http.set_hv_resolver(Arc::new(move |challenge: HvChallenge| {
            seen.lock().unwrap().push(challenge.token.clone());
            // A captcha is answered with its token and the solver's response.
            let answer = format!("{}:resp", challenge.token);
            Box::pin(async move { Ok((answer, "captcha".to_owned())) })
        }));

        let decoded: ValueResp = http.decode(Request::get("/x")).await.unwrap();

        assert_eq!(decoded.value, 77);
        // The resolver was given the token the challenge's details name.
        assert_eq!(*challenged.lock().unwrap(), ["hv-1"]);
    }

    /// Proton's 9001 answer, asking to solve the challenge `hv-1`.
    fn human_verification_required() -> ResponseTemplate {
        ResponseTemplate::new(422).set_body_json(serde_json::json!({
            "Code": 9001,
            "Error": "human verification required",
            "Details": {
                "HumanVerificationToken": "hv-1",
                "HumanVerificationMethods": ["captcha"],
                "WebUrl": "https://verify.proton.me/?methods=captcha&token=hv-1"
            }
        }))
    }

    #[tokio::test]
    async fn human_verification_with_no_resolver_is_left_to_the_caller() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(human_verification_required())
            .mount(&server)
            .await;

        let result = client(&server.uri()).do_raw(Request::get("/x")).await;
        assert!(matches!(result, Err(Error::HumanVerification(ref chal)) if chal.token == "hv-1"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_second_human_verification_is_left_to_the_caller() {
        let server = MockServer::start().await;
        // Even the request carrying the solved challenge is challenged again.
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(human_verification_required())
            .mount(&server)
            .await;
        let mut c = client(&server.uri());
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = asked.clone();
        c.set_hv_resolver(Arc::new(move |chal: HvChallenge| {
            let first = counter.fetch_add(1, Ordering::SeqCst) == 0;
            Box::pin(async move {
                if first {
                    Ok((format!("{}:resp", chal.token), "captcha".to_string()))
                } else {
                    Err(Error::Other("asked to verify twice".into()))
                }
            })
        }));

        let result = c.do_raw(Request::get("/x")).await;
        assert!(matches!(result, Err(Error::HumanVerification(_))));
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
    }
}
