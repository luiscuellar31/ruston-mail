//! What Proton answers while an account signs in.

use crate::model::enums::password_mode;
use serde::Deserialize;

/// The session Proton opens for a sign-in before it knows whose it is.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionResp {
    /// The session's ID.
    #[serde(rename = "UID")]
    pub uid: String,
    /// The token the sign-in requests carry.
    pub access_token: String,
    /// The token that renews the access token.
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

/// The SRP challenge Proton sets an account, from `/core/v4/auth/info`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthInfo {
    /// The SRP modulus, signed with Proton's key, which is checked before it
    /// is used.
    pub modulus: String,
    /// The server's one-time SRP value, B.
    pub server_ephemeral: String,
    /// Which version of Proton's SRP hashing the proof is made with.
    pub version: u8,
    /// The salt the password is hashed with.
    pub salt: String,
    /// Names this challenge in the request that answers it.
    #[serde(rename = "SRPSession")]
    pub srp_session: String,
}

/// The second factors an account has, as the answer to its password proof
/// lists them.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TwoFa {
    /// Its [`two_factor`](crate::model::enums::two_factor) bits, one for
    /// each factor set up.
    #[serde(default)]
    pub enabled: u32,
}

/// Proton's answer to the password proof: the signed-in session and the
/// server's own proof.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthResponse {
    /// The ID of the signed-in session.
    #[serde(rename = "UID")]
    pub uid: String,
    /// The token each request of the session carries.
    pub access_token: String,
    /// The token that renews the access token.
    pub refresh_token: String,
    /// The server's SRP proof, which only a server holding the password's
    /// verifier can make. The sign-in stops when it does not check out.
    #[serde(default)]
    pub server_proof: String,
    /// The second factors the account has.
    #[serde(rename = "2FA", default)]
    pub two_fa: TwoFa,
    /// Password mode, one of [`password_mode`]; one password when absent.
    #[serde(default = "one_password")]
    pub password_mode: u8,
    /// The scope Proton grants the session, when it names one.
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
    fn an_auth_response_reads_its_wire_names_and_defaults() {
        let full: AuthResponse = serde_json::from_value(serde_json::json!({
            "Code": 1000, "UID": "uid-1", "AccessToken": "access", "RefreshToken": "refresh",
            "ServerProof": "proof", "2FA": { "Enabled": 3 }, "PasswordMode": 2, "Scope": "full"
        }))
        .unwrap();
        let tokens = [&full.uid, &full.access_token, &full.refresh_token];
        assert_eq!(tokens, ["uid-1", "access", "refresh"]);
        assert_eq!(full.server_proof, "proof");
        assert_eq!(full.two_fa.enabled, 3);
        assert_eq!(full.password_mode, password_mode::TWO_PASSWORDS);
        assert_eq!(full.scope.as_deref(), Some("full"));

        // Without them: no proof, no second factor, no scope.
        let bare: AuthResponse = serde_json::from_value(serde_json::json!({
            "UID": "uid-1", "AccessToken": "access", "RefreshToken": "refresh"
        }))
        .unwrap();
        assert_eq!((bare.server_proof.as_str(), bare.two_fa.enabled), ("", 0));
        assert_eq!(bare.scope, None);
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
    fn auth_info_reads_its_wire_names() {
        let info: AuthInfo = serde_json::from_value(serde_json::json!({
            "Code": 1000, "Version": 4, "Modulus": "signed modulus",
            "ServerEphemeral": "ephemeral", "Salt": "salt", "SRPSession": "srp session"
        }))
        .unwrap();
        assert_eq!(info.version, 4);
        let challenge = [
            info.modulus,
            info.server_ephemeral,
            info.salt,
            info.srp_session,
        ];
        assert_eq!(
            challenge,
            ["signed modulus", "ephemeral", "salt", "srp session"]
        );
    }
}
