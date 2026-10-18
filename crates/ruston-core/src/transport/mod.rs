//! HTTP transport: typed request/response, Proton envelope handling, and
//! automatic 401-refresh / 429-rate-limit / 9001-human-verification retries.

mod headers;
mod retry;

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

/// Request body.
#[derive(Debug, Clone)]
pub enum Body {
    /// No request body.
    Empty,
    /// JSON request body.
    Json(serde_json::Value),
    /// Raw bytes with an explicit content-type (set via `Request::raw`).
    Bytes(Vec<u8>),
}

/// A transport-level request.
#[derive(Debug, Clone)]
pub struct Request {
    /// HTTP method.
    pub method: Method,
    /// Request path appended to the client's base URL.
    pub path: String,
    /// Query-string parameters.
    pub query: Vec<(String, String)>,
    /// Request body.
    pub body: Body,
    /// Explicit `Content-Type`, if set (e.g. for a raw body).
    pub content_type: Option<String>,
    /// Human-verification token and its type, if attached.
    pub hv: Option<(String, String)>,
    /// Skip the 401→refresh retry (auth-flow endpoints).
    pub skip_refresh: bool,
    /// Do not attach `Authorization` (refresh endpoint).
    pub omit_auth: bool,
    /// Send `x-enforce-unauthsession: true` (session creation).
    pub enforce_unauth: bool,
    /// Maximum response body size, including responses from retries.
    pub max_response_bytes: usize,
    /// Total deadline for each HTTP attempt, including response-body reading.
    pub timeout: Duration,
}

impl Request {
    /// Build an empty request with the given method and path.
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
    /// A `GET` request to `path`.
    pub fn get(path: impl Into<String>) -> Self {
        Self::new(Method::GET, path)
    }
    /// A `POST` request to `path`.
    pub fn post(path: impl Into<String>) -> Self {
        Self::new(Method::POST, path)
    }
    /// A `PUT` request to `path`.
    pub fn put(path: impl Into<String>) -> Self {
        Self::new(Method::PUT, path)
    }
    /// A `DELETE` request to `path`.
    pub fn delete(path: impl Into<String>) -> Self {
        Self::new(Method::DELETE, path)
    }
    /// Set a JSON request body.
    pub fn json(mut self, v: serde_json::Value) -> Self {
        self.body = Body::Json(v);
        self
    }
    /// Set a raw byte body with an explicit content-type.
    pub fn raw(mut self, bytes: Vec<u8>, content_type: impl Into<String>) -> Self {
        self.body = Body::Bytes(bytes);
        self.content_type = Some(content_type.into());
        self
    }
    /// Append a query-string parameter.
    pub fn query<K: Into<String>, V: Into<String>>(mut self, k: K, v: V) -> Self {
        self.query.push((k.into(), v.into()));
        self
    }
    /// Attach a human-verification token and its type.
    pub fn with_hv(mut self, token: impl Into<String>, ttype: impl Into<String>) -> Self {
        self.hv = Some((token.into(), ttype.into()));
        self
    }
    /// Skip the automatic 401→refresh retry (for auth-flow endpoints).
    pub fn no_refresh(mut self) -> Self {
        self.skip_refresh = true;
        self
    }
    /// Do not send the `Authorization` header (for the refresh endpoint).
    pub fn omit_auth(mut self) -> Self {
        self.omit_auth = true;
        self
    }
    /// Send the `x-enforce-unauthsession` header (for session creation).
    pub fn enforce_unauth(mut self) -> Self {
        self.enforce_unauth = true;
        self
    }
    /// Set the maximum number of response body bytes accepted for this request.
    pub fn max_response_bytes(mut self, limit: usize) -> Self {
        self.max_response_bytes = limit;
        self
    }
    /// Override the total deadline for this request's HTTP attempt.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// A transport-level response.
#[derive(Debug, Clone)]
pub struct Response {
    /// HTTP status code.
    pub status: u16,
    /// Raw response body bytes.
    pub body: Vec<u8>,
    /// `Retry-After` delay in seconds, if the server sent one.
    pub retry_after: Option<u64>,
}

/// Mutable auth state shared with the client (so refresh can update it).
#[derive(Debug, Clone)]
pub struct AuthState {
    /// Session UID, once authenticated.
    pub uid: Option<String>,
    /// Current access token, once authenticated.
    pub access: Option<SecretString>,
    /// Current refresh token, once authenticated.
    pub refresh: Option<SecretString>,
    /// App-version string sent on every request.
    pub app_version: String,
    /// Optional `User-Agent` (to present as a real client).
    pub user_agent: Option<String>,
}

impl AuthState {
    /// An empty (logged-out) auth state for the given app version.
    pub fn unauthenticated(app_version: impl Into<String>) -> Self {
        AuthState {
            uid: None,
            access: None,
            refresh: None,
            app_version: app_version.into(),
            user_agent: None,
        }
    }
}

/// Callback invoked after a successful token refresh (uid, access, refresh) —
/// use [`FallibleRefreshPersist`] when storage can fail.
pub type RefreshPersist = Arc<dyn Fn(&str, &str, &str) + Send + Sync>;
/// A token-refresh callback whose persistence failure is returned to the request.
pub type FallibleRefreshPersist = Arc<dyn Fn(&str, &str, &str) -> Result<()> + Send + Sync>;
type RefreshLoad = Arc<dyn Fn() -> Result<(File, Option<Tokens>)> + Send + Sync>;
/// Resolver for human-verification challenges (returns token, token-type).
pub type HvResolver =
    Arc<dyn Fn(HvChallenge) -> BoxFuture<'static, Result<(String, String)>> + Send + Sync>;

/// The async transport interface consumed by the API layer.
#[async_trait]
pub trait Doer: Send + Sync {
    /// Execute a request and return the raw response (after envelope/retry handling).
    async fn do_raw(&self, req: Request) -> Result<Response>;
    /// Execute a request and deserialize the response body into `T`.
    async fn decode<T: DeserializeOwned>(&self, req: Request) -> Result<T>;
}

/// Default HTTP client over `reqwest`.
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
    /// Create a client with a fresh unauthenticated state for `base_url`.
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

    /// Create a client sharing the given auth state (so refresh updates propagate).
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

    /// The shared auth-state handle (clone to observe token rotation).
    pub fn auth_state(&self) -> Arc<RwLock<AuthState>> {
        self.auth.clone()
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
    /// Register a callback invoked with rotated tokens after each refresh.
    pub fn set_refresh_persist(&mut self, cb: RefreshPersist) {
        self.on_refresh = Some(Arc::new(move |uid, access, refresh| {
            cb(uid, access, refresh);
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
    /// Register a resolver to satisfy human-verification (9001) challenges.
    pub fn set_hv_resolver(&mut self, cb: HvResolver) {
        self.hv = Some(cb);
    }

    /// Set the active session's UID and access/refresh tokens.
    pub async fn set_tokens(&self, uid: String, access: SecretString, refresh: SecretString) {
        let mut a = self.auth.write().await;
        a.uid = Some(uid);
        a.access = Some(access);
        a.refresh = Some(refresh);
    }

    /// Present a custom `User-Agent` on every request.
    pub async fn set_user_agent(&self, ua: String) {
        self.auth.write().await.user_agent = Some(ua);
    }

    fn url(&self, req: &Request) -> String {
        let mut u = format!("{}{}", self.base_url, req.path);
        if !req.query.is_empty() {
            let qs: Vec<String> = req
                .query
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect();
            u.push('?');
            u.push_str(&qs.join("&"));
        }
        u
    }

    /// Move byte buffers into reqwest once. Its buffered requests share their
    /// body allocation when cloned for authentication, rate-limit or HV retries.
    fn prepare_request(&self, req: &mut Request) -> Result<reqwest::Request> {
        let builder = self
            .client
            .request(req.method.clone(), self.url(req))
            .timeout(req.timeout);
        let builder = match &mut req.body {
            Body::Empty => builder,
            Body::Json(v) => builder.body(serde_json::to_vec(v)?),
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
        let mut hdrs = headers::build_headers(auth, req);
        if req.omit_auth {
            hdrs.remove(reqwest::header::AUTHORIZATION);
        }
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
        let resp = client.execute(attempt).await?;
        let status = resp.status().as_u16();
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| retry::parse_retry_after(Some(s)));
        let body = read_bounded_body(resp, req.max_response_bytes).await?;
        tracing::debug!(target: "ruston_core::http", status, bytes = body.len(), ms = started.elapsed().as_millis() as u64, "← response");
        Ok(Response {
            status,
            body,
            retry_after,
        })
    }

    /// Refresh the access/refresh tokens via `POST /auth/v4/refresh`, unless
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
        let (uid, refresh) = {
            match (&a.uid, &a.refresh) {
                (Some(u), Some(r)) => (u.clone(), r.expose_secret().to_string()),
                _ => return Err(Error::Unauthorized),
            }
        };
        let body = serde_json::json!({
            "UID": uid,
            "RefreshToken": refresh,
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
        let parsed: RefreshResp = serde_json::from_slice(&resp.body)?;
        a.access = Some(SecretString::from(parsed.access_token.clone()));
        a.refresh = Some(SecretString::from(parsed.refresh_token.clone()));
        if self.on_refresh.is_some() {
            self.refresh_unsaved.store(true, Ordering::Release);
        }
        drop(a);
        if let Some(cb) = &self.on_refresh {
            cb(&uid, &parsed.access_token, &parsed.refresh_token)?;
            self.refresh_unsaved.store(false, Ordering::Release);
        }
        Ok(())
    }
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

/// Outcome of inspecting a response for the Proton error envelope.
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
    #[serde(rename = "Code")]
    code: Option<serde_json::Value>,
    #[serde(rename = "Error")]
    error: Option<serde_json::Value>,
    #[serde(rename = "Details")]
    details: Option<serde_json::Value>,
}

fn detect(status: u16, body: &[u8]) -> Detected {
    let envelope: Option<Envelope> = serde_json::from_slice(body).ok();
    let code = envelope
        .as_ref()
        .and_then(|envelope| envelope.code.as_ref())
        .and_then(serde_json::Value::as_i64);

    // Human verification (may arrive on HTTP 200 in the auth flow, or 422).
    if code == Some(response_code::HUMAN_VERIFICATION)
        && let Some(details) = envelope
            .as_ref()
            .and_then(|envelope| envelope.details.as_ref())
    {
        let methods = details
            .get("HumanVerificationMethods")
            .and_then(|m| m.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        return Detected::Hv(HvChallenge {
            token: details
                .get("HumanVerificationToken")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            methods,
            web_url: details
                .get("WebUrl")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }

    let success = matches!(
        code,
        Some(response_code::SUCCESS | response_code::MULTI_SUCCESS)
    );
    let http_ok = (200..300).contains(&status);

    if http_ok && (success || code.is_none()) {
        return Detected::Ok;
    }
    if http_ok && code == Some(response_code::NONE) {
        return Detected::Ok;
    }

    let message = envelope
        .as_ref()
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

#[async_trait]
impl Doer for HttpClient {
    async fn do_raw(&self, req: Request) -> Result<Response> {
        let mut work = req;
        let prepared = self.prepare_request(&mut work)?;
        // Tokens renewed meanwhile by another request or process may have
        // expired too, so reusing them leaves one more refresh; a pair Proton
        // has just issued is the last try.
        let mut refreshes_left = 2u8;
        let mut rate_limited = false;
        let mut hv_tried = false;

        loop {
            let sent_auth = self.auth.read().await.clone();
            let resp = Self::execute_once(&self.client, &work, &prepared, &sent_auth).await?;
            match detect(resp.status, &resp.body) {
                Detected::Ok => return Ok(resp),
                Detected::Api(e)
                    if e.is_unauthorized() && !work.skip_refresh && refreshes_left > 0 =>
                {
                    tracing::debug!(target: "ruston_core::http", "401 — refreshing access token and retrying");
                    refreshes_left = match self.refresh_tokens(&sent_auth).await? {
                        Refresh::Rotated => 0,
                        Refresh::Reused => refreshes_left - 1,
                    };
                    continue;
                }
                Detected::Api(e) if e.is_unauthorized() => return Err(Error::Unauthorized),
                Detected::Api(e) if e.is_rate_limited() && !rate_limited => {
                    let wait = resp.retry_after.unwrap_or(0);
                    tracing::debug!(target: "ruston_core::http", wait_s = wait, "429 rate-limited — backing off then retrying");
                    if wait > 0 {
                        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    }
                    rate_limited = true;
                    continue;
                }
                Detected::Hv(chal) if self.hv.is_some() && !hv_tried => {
                    tracing::info!(target: "ruston_core::auth", methods = ?chal.methods, "9001 human verification required — invoking resolver");
                    let resolver = self.hv.as_ref().unwrap().clone();
                    let (tok, ttype) = resolver(chal).await?;
                    tracing::info!(target: "ruston_core::auth", token_type = %ttype, "human verification token obtained — retrying");
                    work.hv = Some((tok, ttype));
                    hv_tried = true;
                    continue;
                }
                Detected::Hv(chal) => return Err(Error::HumanVerification(chal)),
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

    #[tokio::test]
    async fn decodes_success_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code":1000,"Value":42})),
            )
            .mount(&server)
            .await;
        let c = client(&server.uri());
        let v: ValueResp = c.decode(Request::get("/x")).await.unwrap();
        assert_eq!(v.value, 42);
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
    async fn maps_error_code() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code":2501,"Error":"nope"})),
            )
            .mount(&server)
            .await;
        let c = client(&server.uri());
        let err = c.decode::<ValueResp>(Request::get("/x")).await.unwrap_err();
        match err {
            Error::Api(e) => {
                assert_eq!(e.code, 2501);
                assert_eq!(e.message, "nope");
            }
            other => panic!("expected Api, got {other:?}"),
        }
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
    async fn retries_once_on_429() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code":1000,"Value":7})),
            )
            .mount(&server)
            .await;
        let c = client(&server.uri());
        let v: ValueResp = c.decode(Request::get("/x")).await.unwrap();
        assert_eq!(v.value, 7);
    }

    #[tokio::test]
    async fn refreshes_then_retries_on_401() {
        let server = MockServer::start().await;
        // First protected call → 401.
        Mock::given(method("GET"))
            .and(path("/protected"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(serde_json::json!({"Code":401,"Error":"expired"})),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Refresh endpoint → new tokens.
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"Code":1000,"AccessToken":"new-acc","RefreshToken":"new-ref"}),
            ))
            .mount(&server)
            .await;
        // Retried protected call → success.
        Mock::given(method("GET"))
            .and(path("/protected"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code":1000,"Value":99})),
            )
            .mount(&server)
            .await;

        let c = client(&server.uri());
        c.set_tokens(
            "uid".into(),
            SecretString::from("old-acc"),
            SecretString::from("old-ref"),
        )
        .await;
        let v: ValueResp = c.decode(Request::get("/protected")).await.unwrap();
        assert_eq!(v.value, 99);
        // Tokens were rotated.
        let a = c.auth.read().await;
        assert_eq!(a.access.as_ref().unwrap().expose_secret(), "new-acc");
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
        c.set_refresh_persist_fallible(Arc::new(|_, _, _| {
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
        client.set_refresh_persist_fallible(Arc::new(|_, _, _| {
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
        http.set_refresh_persist_fallible(Arc::new(move |_, access, refresh| {
            *saves.lock().unwrap() = Some((access.to_owned(), refresh.to_owned()));
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

    #[tokio::test]
    async fn concurrent_401s_refresh_shared_session_once() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .and(header("authorization", "Bearer old-acc"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_json(serde_json::json!({"Code":401,"Error":"expired"}))
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
    async fn unauthorized_when_refresh_fails() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/protected"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({"Code":401})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/auth/v4/refresh"))
            .respond_with(
                ResponseTemplate::new(422).set_body_json(serde_json::json!({"Code":10013})),
            )
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
        assert!(matches!(err, Error::Unauthorized));
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
    async fn resolves_human_verification_then_retries_with_headers() {
        let server = MockServer::start().await;
        // Retry that carries the HV headers → success (higher priority).
        Mock::given(method("GET"))
            .and(path("/x"))
            .and(wiremock::matchers::header(
                "x-pm-human-verification-token",
                "chal:resp",
            ))
            .and(wiremock::matchers::header(
                "x-pm-human-verification-token-type",
                "captcha",
            ))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Code":1000,"Value":77})),
            )
            .with_priority(1)
            .mount(&server)
            .await;
        // First (no HV header) → 9001 challenge.
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "Code": 9001,
                "Error": "human verification required",
                "Details": {
                    "HumanVerificationToken": "chal",
                    "HumanVerificationMethods": ["captcha"],
                    "WebUrl": "https://verify.proton.me/?methods=captcha&token=chal"
                }
            })))
            .mount(&server)
            .await;

        let mut c = client(&server.uri());
        let seen: Arc<RwLock<Vec<String>>> = Arc::new(RwLock::new(Vec::new()));
        let seen2 = seen.clone();
        c.set_hv_resolver(Arc::new(move |chal: HvChallenge| {
            let seen2 = seen2.clone();
            Box::pin(async move {
                seen2.write().await.push(chal.token.clone());
                // value format = "<challenge>:<captcha-response>"
                Ok((format!("{}:resp", chal.token), "captcha".to_string()))
            })
        }));

        let v: ValueResp = c.decode(Request::get("/x")).await.unwrap();
        assert_eq!(v.value, 77);
        // resolver was invoked with the challenge token from Details.
        assert_eq!(seen.read().await.as_slice(), &["chal".to_string()]);
    }
}
