//! The passphrase of a user key, worked out from the mailbox password and
//! the salt Proton keeps for that key.

use crate::error::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use proton_srp::mailbox_password_hash;
use secrecy::{ExposeSecret, SecretString};

/// The passphrase, or "salted key password", of the user key whose salt is
/// `salt_b64` (base64, as Proton lists it): the last 31 characters of the
/// bcrypt hash of `password` with that salt, which leave out bcrypt's
/// version, cost and salt.
pub fn derive_key_passphrase(password: &SecretString, salt_b64: &str) -> Result<SecretString> {
    let salt = match STANDARD.decode(salt_b64) {
        Ok(salt) => salt,
        Err(e) => return Err(Error::Crypto(format!("invalid key salt base64: {e}"))),
    };
    tracing::trace!(target: "ruston_core::crypto", salt_bytes = salt.len(), "deriving key passphrase (bcrypt mailbox hash, last 31 bytes)");
    let hash = mailbox_password_hash(password.expose_secret(), &salt)
        .map_err(|e| Error::Srp(format!("mailbox password hash failed: {e}")))?;
    // The hash wipes itself when dropped; the passphrase is its one copy.
    let passphrase = match std::str::from_utf8(hash.hashed_password()) {
        Ok(text) => SecretString::from(text),
        Err(e) => return Err(Error::Crypto(format!("non-utf8 key passphrase: {e}"))),
    };
    tracing::trace!(target: "ruston_core::crypto", len = passphrase.expose_secret().len(), "key passphrase derived");
    Ok(passphrase)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Passphrases worked out once and kept: a change in how they are derived
    /// would lock every account out of its keys.
    #[test]
    fn each_password_and_salt_derive_a_known_passphrase() {
        let counting: [u8; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        for (password, salt, expected) in [
            ("hunter2", [7; 16], "WmQaniQNSnDAbD2tgDBeTdc3rBCmHau"),
            ("", [7; 16], "r0gRy8e5e7IyypUiMJr6RqQUmXMePru"),
            (
                "correct horse battery staple",
                counting,
                "bRoslBHnMTNf7r5vXwx5UX9sWYt/FHi",
            ),
            ("pässwörd ✓", [0xA5; 16], "IwjBlnc3nPDf8AYueRQzdYWyjlt2/yq"),
        ] {
            let passphrase =
                derive_key_passphrase(&SecretString::from(password), &STANDARD.encode(salt))
                    .unwrap();
            let passphrase = passphrase.expose_secret();
            assert_eq!(passphrase, expected, "{password:?}");
            // The last 31 characters of the bcrypt hash, its salt left out.
            assert_eq!(passphrase.len(), 31);
        }
    }

    #[test]
    fn a_salt_that_is_not_base64_or_not_sixteen_bytes_is_refused() {
        let refusal =
            |salt_b64: &str| match derive_key_passphrase(&SecretString::from("x"), salt_b64) {
                Err(Error::Crypto(message)) => format!("crypto: {message}"),
                Err(Error::Srp(message)) => format!("srp: {message}"),
                other => panic!("{salt_b64:?} gave {other:?}"),
            };
        let unreadable = refusal("!!!not base64!!!");
        assert!(
            unreadable.starts_with("crypto: invalid key salt base64: "),
            "{unreadable}"
        );
        for short in [STANDARD.encode([1u8; 10]), String::new()] {
            let refused = refusal(&short);
            assert!(
                refused.starts_with("srp: mailbox password hash failed: "),
                "{refused}"
            );
        }
    }
}
