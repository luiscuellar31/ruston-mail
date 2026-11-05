//! End-to-end encryption: unlocking the account's keys, then opening,
//! sealing and signing mail with them, through Proton's `proton-crypto`.
//! Key passphrases come from `proton-srp`'s password hash.

pub(crate) mod clock;
pub mod kdf;
pub mod keys;
pub mod message;

pub use keys::{Address, AddressKeys, ApiKey, KeySalt, KeyStore, StoredKey, User};
pub use message::{
    SessionKeyMaterial, UploadedAttachment, Verdict, algo_name, decrypt_attachment, decrypt_body,
    decrypt_session_key, encrypt_attachment, encrypt_for_transport, encrypt_self_draft,
    encrypt_text_with_password, new_session_key, rewrap_attachment_session_key,
    verify_attachment_signature, wrap_session_key, wrap_session_key_to_self,
    wrap_session_key_with_password,
};

/// The OpenPGP provider that each operation here is given: proton-crypto's
/// `rustpgp` backend, written in Rust, the only one the workspace builds.
pub fn provider() -> impl proton_crypto::crypto::PGPProviderSync {
    proton_crypto::new_pgp_provider()
}
