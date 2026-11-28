//! The account's keys as Proton lists them, and the opening of the address
//! keys, by way of the user keys, into a [`KeyStore`] kept in memory.
//!
//! An imported key's type belongs to the provider [`super::provider`]
//! returns, which cannot be named, so no struct can hold one. The store
//! keeps what imports a key again for each operation instead: the unlocked
//! copy exported when it first opened, which skips deriving its passphrase,
//! or else the armored key and its passphrase.

use super::kdf;
use crate::error::{Error, Result};
use crate::model::enums::key_flag;
use proton_crypto::crypto::{
    DataEncoding, Decryptor, DecryptorSync, PGPProviderSync, VerifiedData, Verifier, VerifierSync,
};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use zeroize::Zeroizing;

/// A private key of the user or of an address, as Proton lists it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ApiKey {
    /// The key's ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// The key, armored and locked with its passphrase.
    pub private_key: String,
    /// An address key's passphrase, encrypted to the user keys. A legacy
    /// key, without it or without its signature, or with either empty, opens
    /// with the key password instead.
    #[serde(default)]
    pub token: Option<String>,
    /// A user key's detached signature over the token; a token whose
    /// signature fails leaves its key locked.
    #[serde(default)]
    pub signature: Option<String>,
    /// The token of a key not yet activated. Nothing here activates keys,
    /// so it is only read.
    #[serde(default)]
    pub activation: Option<String>,
    /// `1` for the primary key of its user or address.
    #[serde(default)]
    pub primary: u8,
    /// `0` for a key no longer in use, which is never opened. Proton leaving
    /// it out reads as `1`.
    #[serde(default = "default_active")]
    pub active: u8,
    /// The key's [`key_flag`] bits, when Proton sends them.
    #[serde(default)]
    pub flags: Option<u32>,
}

fn default_active() -> u8 {
    1
}

impl ApiKey {
    /// Whether the key is usable; inactive keys are skipped.
    pub fn is_active(&self) -> bool {
        self.active != 0
    }

    /// Whether this is the primary key of its ring.
    pub fn is_primary(&self) -> bool {
        self.primary == 1
    }
}

/// The account's user, with the keys that open its addresses' keys.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct User {
    /// Account identity returned by the authenticated user endpoint.
    #[serde(rename = "ID")]
    pub id: String,
    /// The user keys.
    pub keys: Vec<ApiKey>,
}

/// One of the account's addresses, with its keys, as Proton lists it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Address {
    /// The address's ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// The email address.
    pub email: String,
    /// Where it sorts among the account's addresses, from 1. The lowest that
    /// mail can go out from is the one it goes out from unless another is
    /// chosen.
    #[serde(default)]
    pub order: u32,
    /// `1` while the address is enabled, `0` once disabled, `2` while it is
    /// being deleted. Proton leaving it out reads as `1`.
    #[serde(default = "default_active")]
    pub status: u8,
    /// `1` when Proton lets mail go out from the address. Proton leaving it
    /// out reads as `1`.
    #[serde(default = "default_active")]
    pub send: u8,
    /// The address's keys.
    #[serde(default)]
    pub keys: Vec<ApiKey>,
}

/// The salt that, with the mailbox password, gives one user key's
/// passphrase.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct KeySalt {
    /// The ID of the key it is for.
    #[serde(rename = "ID")]
    pub id: String,
    /// The salt, in base64. `None` or empty for a key without one, which
    /// opens with the primary key's salt instead, or with the mailbox
    /// password itself when that has none either.
    pub key_salt: Option<String>,
}

/// An address key that opened, kept in the forms that open it again.
#[derive(Clone)]
pub struct StoredKey {
    /// The key as Proton lists it, armored and locked.
    pub armored: String,
    /// What unlocks `armored`.
    pub passphrase: SecretString,
    /// The same key exported without passphrase protection when it was first
    /// unlocked. Re-importing it skips the passphrase derivation, which can
    /// take tens of milliseconds per key; `None` falls back to `armored`.
    pub unlocked: Option<Zeroizing<Vec<u8>>>,
}

/// The keys of one of the account's addresses that opened.
#[derive(Clone)]
pub struct AddressKeys {
    /// The address's ID.
    pub address_id: String,
    /// The email address.
    pub email: String,
    /// Whether mail can go out from the address: Proton has it enabled and
    /// lets it send. The keys of one that cannot still open the mail that
    /// reached it.
    pub can_send: bool,
    /// The address's unlocked keys, the primary first: messages are signed
    /// and drafts encrypted with the first one.
    pub keys: Vec<StoredKey>,
}

/// The account's unlocked address keys. User keys only open address keys,
/// so none is kept once they have.
#[derive(Clone)]
pub struct KeyStore {
    /// Each address whose keys opened, in Proton's `Order`, those mail can
    /// no longer go out from included: their keys still open the mail that
    /// reached them.
    pub addresses: Vec<AddressKeys>,
}

impl KeyStore {
    /// The keys of the address whose API ID is `address_id`.
    pub fn address(&self, address_id: &str) -> Option<&AddressKeys> {
        self.addresses
            .iter()
            .find(|keys| keys.address_id == address_id)
    }

    /// The keys of `email`, whatever the case of its ASCII letters, when mail
    /// can go out from it.
    pub fn address_for_email(&self, email: &str) -> Option<&AddressKeys> {
        self.addresses
            .iter()
            .find(|keys| keys.can_send && keys.email.eq_ignore_ascii_case(email))
    }

    /// The address mail is sent from unless another is chosen, as in
    /// Proton's web client: the first in Proton's `Order`, which unlocking
    /// keeps, that mail can go out from. A `from` address picks another
    /// through [`Self::address_for_email`].
    pub fn primary_address(&self) -> Option<&AddressKeys> {
        self.addresses.iter().find(|keys| keys.can_send)
    }
}

/// Order an address's keys so the one to sign and encrypt with comes first:
/// the primary key, then keys Proton has not marked compromised or obsolete.
/// Keys without flags count as sound, and the sort is stable, so keys that
/// rank the same keep the API's order.
fn keys_by_preference(keys: &[ApiKey]) -> Vec<&ApiKey> {
    let mut ordered: Vec<&ApiKey> = keys.iter().collect();
    ordered.sort_by_key(|key| {
        let sound = key
            .flags
            .is_none_or(|flags| key_flag::can_verify(flags) && key_flag::can_encrypt(flags));
        (!key.is_primary(), !sound)
    });
    ordered
}

/// Opens the account's keys when signing in, each user key with the
/// passphrase the mailbox password and its salt derive. Returns the
/// `KeyStore` and the key password (`skp`, the salted key password) for
/// session persistence: the passphrase of the first user key that opens,
/// which also opens address keys without a signed token.
pub fn unlock<P: PGPProviderSync>(
    provider: &P,
    user: &User,
    addresses: &[Address],
    password: &SecretString,
    salts: &[KeySalt],
) -> Result<(KeyStore, SecretString)> {
    let salt_of = |key_id: &str| {
        salts
            .iter()
            .find(|salt| salt.id == key_id)
            .and_then(|salt| salt.key_salt.as_deref())
            .filter(|salt| !salt.is_empty())
    };
    // Proton's web client derives the key password from the primary key's
    // salt, so a key without a salt of its own borrows that one. When the
    // primary has none either, as before key passwords were salted, the
    // password itself opens the key, as it does there.
    let primary_salt = user
        .keys
        .iter()
        .find(|key| key.is_primary())
        .and_then(|primary| salt_of(&primary.id));
    unlock_with(provider, user, addresses, |key| {
        match salt_of(&key.id).or(primary_salt) {
            Some(salt_b64) => kdf::derive_key_passphrase(password, salt_b64),
            None => Ok(password.clone()),
        }
    })
}

/// Opens the account's keys when resuming a session, with the key password
/// `skp` it saved: no password is asked for and no passphrase derived.
pub fn unlock_with_skp<P: PGPProviderSync>(
    provider: &P,
    user: &User,
    addresses: &[Address],
    skp: &SecretString,
) -> Result<KeyStore> {
    // Every user key is tried with the one key password the session saved.
    let (store, _) = unlock_with(provider, user, addresses, |_| Ok(skp.clone()))?;
    Ok(store)
}

/// Opens the user keys, each with the passphrase `passphrase_for` gives it,
/// then the address keys they open. Returns those with the key password.
fn unlock_with<P: PGPProviderSync>(
    provider: &P,
    user: &User,
    addresses: &[Address],
    passphrase_for: impl Fn(&ApiKey) -> Result<SecretString>,
) -> Result<(KeyStore, SecretString)> {
    tracing::debug!(target: "ruston_core::crypto", user_keys = user.keys.len(), addresses = addresses.len(), "unlocking key hierarchy");
    let user_keys = unlock_user_keys(provider, user, passphrase_for)?;
    // In Proton's `Order`, the default address first. Addresses with the
    // same `Order`, or none, keep the order they were listed in: the sort is
    // stable.
    let mut by_order: Vec<&Address> = addresses.iter().collect();
    by_order.sort_by_key(|address| address.order);
    let addresses: Vec<AddressKeys> = by_order
        .into_iter()
        .filter_map(|address| unlock_address_keys(provider, &user_keys, address))
        .collect();
    if addresses.is_empty() {
        return Err(Error::Crypto("no address key could be unlocked".into()));
    }
    tracing::info!(target: "ruston_core::crypto", addresses = addresses.len(), "key hierarchy unlocked");

    Ok((KeyStore { addresses }, user_keys.key_password))
}

/// The account's unlocked user keys, imported to open address key tokens,
/// their public halves to check the tokens' signatures, and the key password
/// that opens legacy address keys. They go once address keys are open.
struct UserKeys<P: PGPProviderSync> {
    privs: Vec<P::PrivateKey>,
    pubs: Vec<P::PublicKey>,
    /// The passphrase of the first user key that opened.
    key_password: SecretString,
}

/// Opens each active user key with the passphrase `passphrase_for` gives it.
/// A key that does not open, or whose passphrase cannot be worked out, is
/// left out; none opening is an error.
fn unlock_user_keys<P: PGPProviderSync>(
    provider: &P,
    user: &User,
    passphrase_for: impl Fn(&ApiKey) -> Result<SecretString>,
) -> Result<UserKeys<P>> {
    let mut key_password = None;
    let mut privs = Vec::new();
    for key in user.keys.iter().filter(|key| key.is_active()) {
        let passphrase = match passphrase_for(key) {
            Ok(passphrase) => passphrase,
            Err(error) => {
                tracing::warn!(target: "ruston_core::crypto", key_id = %key.id, error = %error, "user key passphrase unavailable — key left out");
                continue;
            }
        };
        if let Ok(imported) = provider
            .private_key_import(
                key.private_key.as_bytes(),
                passphrase.expose_secret().as_bytes(),
                DataEncoding::Armor,
            )
            .inspect_err(|error| {
                // Another user key opening is enough, so this alone is no fault.
                tracing::debug!(target: "ruston_core::crypto", key_id = %key.id, error = %error, "user key did not unlock — key left out");
            })
        {
            tracing::trace!(target: "ruston_core::crypto", key_id = %key.id, "unlocked user key");
            privs.push(imported);
            key_password.get_or_insert(passphrase);
        }
    }
    let Some(key_password) = key_password else {
        return Err(Error::UserKeysLocked);
    };
    tracing::debug!(target: "ruston_core::crypto", count = privs.len(), "user keys unlocked");
    let pubs = privs
        .iter()
        .filter_map(|private| provider.private_key_to_public_key(private).ok())
        .collect();
    Ok(UserKeys {
        privs,
        pubs,
        key_password,
    })
}

/// Opens the active keys of `address`, leaving out those that do not open.
/// `None` when none does.
fn unlock_address_keys<P: PGPProviderSync>(
    provider: &P,
    user_keys: &UserKeys<P>,
    address: &Address,
) -> Option<AddressKeys> {
    let mut keys = Vec::new();
    // The first key that unlocks signs and encrypts drafts, so the primary
    // is tried first, whatever order the API lists keys in.
    for key in keys_by_preference(&address.keys)
        .into_iter()
        .filter(|key| key.is_active())
    {
        let Some(passphrase) = address_key_passphrase(provider, user_keys, address, key) else {
            continue;
        };
        if let Ok(imported) = provider
            .private_key_import(
                key.private_key.as_bytes(),
                passphrase.expose_secret().as_bytes(),
                DataEncoding::Armor,
            )
            .inspect_err(|error| {
                tracing::warn!(target: "ruston_core::crypto", address = %address.email, key_id = %key.id, error = %error, "address key did not unlock — skipping key");
            })
        {
            keys.push(StoredKey {
                armored: key.private_key.clone(),
                passphrase,
                unlocked: provider
                    .private_key_export_unlocked(&imported, DataEncoding::Bytes)
                    .ok()
                    .map(|bytes| Zeroizing::new(bytes.as_ref().to_vec())),
            });
        }
    }
    if keys.is_empty() {
        return None;
    }
    tracing::debug!(target: "ruston_core::crypto", address = %address.email, keys = keys.len(), "address keys unlocked");
    Some(AddressKeys {
        address_id: address.id.clone(),
        email: address.email.clone(),
        can_send: address.status == 1 && address.send == 1,
        keys,
    })
}

/// The passphrase that opens address key `key`: its token, once the user
/// keys decrypt it and vouch for its signature, or the key password for a
/// legacy key without a signed token. `None` when the token does not check
/// out, which leaves the key locked.
fn address_key_passphrase<P: PGPProviderSync>(
    provider: &P,
    user_keys: &UserKeys<P>,
    address: &Address,
    key: &ApiKey,
) -> Option<SecretString> {
    // An empty field counts as none, as go-proton-api reads it.
    let token = key.token.as_deref().filter(|token| !token.is_empty());
    let signature = key
        .signature
        .as_deref()
        .filter(|signature| !signature.is_empty());
    let (Some(token), Some(signature)) = (token, signature) else {
        tracing::trace!(target: "ruston_core::crypto", address = %address.email, key_id = %key.id, "address key: legacy skp passphrase");
        return Some(user_keys.key_password.clone());
    };
    tracing::trace!(target: "ruston_core::crypto", address = %address.email, key_id = %key.id, "address key: decrypting token + verifying signature against user keys");
    decrypt_token(
        provider,
        &user_keys.privs,
        &user_keys.pubs,
        token,
        signature,
    )
    .inspect_err(|error| {
        tracing::warn!(target: "ruston_core::crypto", address = %address.email, key_id = %key.id, error = %error, "address key token invalid — skipping key");
    })
    .ok()
}

/// The passphrase of an address key that its armored token holds, once a
/// user key opens the token and `sig_armored`, a detached signature over
/// it, proves one of them made it. A token that fails either check leaves
/// its key locked.
fn decrypt_token<P: PGPProviderSync>(
    provider: &P,
    user_privs: &[P::PrivateKey],
    user_pubs: &[P::PublicKey],
    token_armored: &str,
    sig_armored: &str,
) -> Result<SecretString> {
    let opened = provider
        .new_decryptor()
        .with_decryption_keys(user_privs)
        .decrypt(token_armored, DataEncoding::Armor)
        .map_err(|e| Error::Crypto(format!("token decrypt failed: {e}")))?;
    // The token is the address key's passphrase: moved out, not copied, and
    // wiped once the passphrase holds it.
    let token = Zeroizing::new(opened.into_vec());
    let signed_by_user = provider
        .new_verifier()
        .with_verification_keys(user_pubs)
        .verify_detached(token.as_slice(), sig_armored, DataEncoding::Armor)
        .is_ok();
    if !signed_by_user {
        return Err(Error::Crypto("address key token signature invalid".into()));
    }
    std::str::from_utf8(&token)
        .map(SecretString::from)
        .map_err(|e| Error::Crypto(format!("non-utf8 token: {e}")))
}

#[cfg(test)]
mod tests;
