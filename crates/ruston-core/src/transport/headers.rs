//! The headers each request carries: the client's, the session's, and those
//! its own options ask for.

use super::{AuthState, Prepared};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use secrecy::ExposeSecret;

/// The headers `prepared` is sent with under the session in `auth`, in the
/// order they go out. A header whose value is absent or cannot be a header
/// value is left out rather than failing the request, so Proton answers as if
/// it were missing: a token that is not a valid header reads as an expired
/// session.
pub fn build_headers(auth: &AuthState, prepared: &Prepared) -> HeaderMap {
    let bearer = auth
        .tokens
        .as_ref()
        .filter(|_| !prepared.omit_auth)
        .map(|tokens| format!("Bearer {}", tokens.access.expose_secret()));
    // Proton reads a verification token only with its type: both go, or neither.
    let (hv_token, hv_type) = prepared
        .hv
        .as_ref()
        .filter(|(token, kind)| {
            HeaderValue::from_str(token).is_ok() && HeaderValue::from_str(kind).is_ok()
        })
        .map(|(token, kind)| (token.as_str(), kind.as_str()))
        .unzip();
    let wanted = [
        (
            "content-type",
            Some(
                prepared
                    .content_type
                    .as_deref()
                    .unwrap_or("application/json"),
            ),
        ),
        ("x-pm-appversion", Some(auth.app_version.as_str())),
        ("user-agent", auth.user_agent.as_deref()),
        (
            "x-pm-uid",
            auth.tokens.as_ref().map(|tokens| tokens.uid.as_str()),
        ),
        ("authorization", bearer.as_deref()),
        ("x-pm-human-verification-token", hv_token),
        ("x-pm-human-verification-token-type", hv_type),
        (
            "x-enforce-unauthsession",
            prepared.enforce_unauth.then_some("true"),
        ),
    ];
    wanted
        .into_iter()
        .filter_map(|(name, value)| {
            let value = HeaderValue::from_str(value?).ok()?;
            Some((HeaderName::from_static(name), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Tokens;
    use crate::transport::{HttpClient, Request};
    use secrecy::SecretString;

    /// A signed-in session whose client presents a browser's user agent.
    fn signed_in() -> AuthState {
        AuthState {
            tokens: Some(Tokens {
                uid: "UID123".into(),
                access: SecretString::from("ACCESS"),
                refresh: SecretString::from("REFRESH"),
            }),
            user_agent: Some("Mozilla/5.0 test-ua".into()),
            ..AuthState::unauthenticated("Other")
        }
    }

    /// The headers `request` gets under `auth`, as `name: value` lines in
    /// the order they are sent.
    fn sent(auth: &AuthState, request: Request) -> Vec<String> {
        let prepared = HttpClient::new("http://127.0.0.1", "Other")
            .prepare(request)
            .unwrap();
        build_headers(auth, &prepared)
            .iter()
            .map(|(name, value)| format!("{name}: {}", value.to_str().unwrap()))
            .collect()
    }

    #[test]
    fn a_signed_in_request_names_the_client_and_the_session() {
        assert_eq!(
            sent(&signed_in(), Request::get("/x")),
            [
                "content-type: application/json",
                "x-pm-appversion: Other",
                "user-agent: Mozilla/5.0 test-ua",
                "x-pm-uid: UID123",
                "authorization: Bearer ACCESS",
            ]
        );
    }

    #[test]
    fn before_sign_in_only_the_client_is_named() {
        assert_eq!(
            sent(&AuthState::unauthenticated("Other"), Request::get("/x")),
            ["content-type: application/json", "x-pm-appversion: Other"]
        );
    }

    #[test]
    fn a_verification_token_goes_with_its_type_or_not_at_all() {
        let solved = sent(&signed_in(), Request::post("/x").with_hv("tok", "captcha"));
        assert_eq!(
            solved[solved.len() - 2..],
            [
                "x-pm-human-verification-token: tok",
                "x-pm-human-verification-token-type: captcha",
            ]
        );
        let broken = sent(&signed_in(), Request::post("/x").with_hv("tok", "a\nb"));
        assert!(
            broken.iter().all(|line| !line.starts_with("x-pm-human")),
            "{broken:?}"
        );
    }

    #[test]
    fn a_raw_body_declares_its_own_content_type() {
        let upload = Request::post("/x").raw(vec![1, 2, 3], "multipart/form-data; boundary=z");
        assert_eq!(
            sent(&signed_in(), upload)[0],
            "content-type: multipart/form-data; boundary=z"
        );
    }

    #[test]
    fn the_token_refresh_names_its_session_without_an_access_token() {
        let refresh = sent(&signed_in(), Request::post("/x").omit_auth());
        assert!(refresh.contains(&"x-pm-uid: UID123".to_owned()));
        assert!(
            refresh
                .iter()
                .all(|line| !line.starts_with("authorization")),
            "{refresh:?}"
        );
    }

    #[test]
    fn session_creation_asks_for_a_session_of_its_own() {
        let creation = sent(
            &AuthState::unauthenticated("Other"),
            Request::post("/x").enforce_unauth(),
        );
        assert_eq!(creation.last().unwrap(), "x-enforce-unauthsession: true");
    }

    #[test]
    fn a_value_that_cannot_be_a_header_is_left_out() {
        let mut auth = signed_in();
        auth.tokens.as_mut().unwrap().access = SecretString::from("expired\r\ntoken");
        let headers = sent(&auth, Request::get("/x"));
        assert!(
            headers
                .iter()
                .all(|line| !line.starts_with("authorization")),
            "{headers:?}"
        );
        assert!(headers.contains(&"x-pm-uid: UID123".to_owned()));
    }
}
