//! Auth wire types.

use crate::model::enums::password_mode;
use serde::Deserialize;

/// Response from creating an unauthenticated session (UID + initial tokens).
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionResp {
    /// Session UID identifying this (initially unauthenticated) session.
    #[serde(rename = "UID")]
    pub uid: String,
    /// Bearer access token for the session.
    pub access_token: String,
    /// Refresh token used to mint new access tokens.
    pub refresh_token: String,
}

// The tokens stay out of the debug form.
impl std::fmt::Debug for SessionResp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionResp")
            .field("uid", &self.uid)
            .finish_non_exhaustive()
    }
}

/// SRP challenge parameters returned by the auth-info endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthInfo {
    /// Signed SRP group modulus (its signature is verified during login).
    pub modulus: String,
    /// Server's SRP ephemeral value (B).
    pub server_ephemeral: String,
    /// SRP hash version to use.
    pub version: u8,
    /// Per-user SRP salt.
    pub salt: String,
    /// Opaque SRP session identifier echoed back when submitting the proof.
    #[serde(rename = "SRPSession")]
    pub srp_session: String,
}

/// Two-factor status block embedded in the auth response.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TwoFa {
    /// Bitmask of enabled 2FA methods, of
    /// [`two_factor`](crate::model::enums::two_factor) bits.
    #[serde(default)]
    pub enabled: u32,
}

/// Response from completing SRP authentication (tokens, server proof, 2FA/scope).
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthResponse {
    /// Session UID for the now-authenticated session.
    #[serde(rename = "UID")]
    pub uid: String,
    /// Bearer access token for the session.
    pub access_token: String,
    /// Refresh token used to mint new access tokens.
    pub refresh_token: String,
    /// Server's SRP proof, verified client-side as a MITM guard.
    #[serde(default)]
    pub server_proof: String,
    /// Two-factor status (which methods are required).
    #[serde(rename = "2FA", default)]
    pub two_fa: TwoFa,
    /// Password mode, one of [`password_mode`]; one password when absent.
    #[serde(default = "one_password")]
    pub password_mode: u8,
    /// Granted scope string, if returned.
    #[serde(default)]
    pub scope: Option<String>,
}

// The tokens stay out of the debug form.
impl std::fmt::Debug for AuthResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthResponse")
            .field("uid", &self.uid)
            .field("two_fa", &self.two_fa)
            .field("password_mode", &self.password_mode)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

fn one_password() -> u8 {
    password_mode::ONE_PASSWORD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_auth_response() {
        let json = serde_json::json!({
            "Code": 1000, "UID": "x", "AccessToken": "a", "RefreshToken": "r",
            "ServerProof": "sp", "2FA": {"Enabled": 1}, "PasswordMode": 2
        });
        let r: AuthResponse = serde_json::from_value(json).unwrap();
        assert_eq!(r.uid, "x");
        assert_eq!(r.two_fa.enabled, 1);
        assert_eq!(r.password_mode, password_mode::TWO_PASSWORDS);
    }

    #[test]
    fn an_auth_response_without_a_password_mode_has_one_password() {
        let json = serde_json::json!({
            "Code": 1000, "UID": "x", "AccessToken": "a", "RefreshToken": "r"
        });
        let r: AuthResponse = serde_json::from_value(json).unwrap();
        assert_eq!(r.password_mode, 1);
    }

    #[test]
    fn tokens_stay_out_of_the_debug_form() {
        let tokens = serde_json::json!({
            "UID": "uid-1", "AccessToken": "access-secret", "RefreshToken": "refresh-secret"
        });
        let session: SessionResp = serde_json::from_value(tokens.clone()).unwrap();
        let auth: AuthResponse = serde_json::from_value(tokens).unwrap();
        for shown in [format!("{session:?}"), format!("{auth:?}")] {
            assert!(shown.contains("uid-1"), "{shown}");
            assert!(!shown.contains("secret"), "{shown}");
        }
    }

    #[test]
    fn auth_info_srp_session_rename() {
        let json = serde_json::json!({
            "Modulus": "m", "ServerEphemeral": "se", "Version": 4, "Salt": "s", "SRPSession": "sess"
        });
        let i: AuthInfo = serde_json::from_value(json).unwrap();
        assert_eq!(i.srp_session, "sess");
        assert_eq!(i.version, 4);
    }
}
