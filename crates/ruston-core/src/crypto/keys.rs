//! Key hierarchy: wire types + unlock into an in-memory `KeyStore`.
//!
//! Because `proton-crypto`'s key/provider types are unnameable (private
//! associated types), the `KeyStore` holds key material that each crypto op
//! re-imports: an address key's unlocked export, which skips the passphrase
//! derivation, with the armored key and its passphrase as the fallback.

use super::kdf;
use crate::error::{Error, Result};
use crate::model::enums::key_flag;
use proton_crypto::crypto::{
    DataEncoding, Decryptor, DecryptorSync, PGPProviderSync, VerifiedData, Verifier, VerifierSync,
};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use zeroize::Zeroizing;

/// A raw API key (user or address).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ApiKey {
    /// The key's API ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// Armored PGP private key.
    pub private_key: String,
    /// Encrypted passphrase (a PGP message to a user key) that unlocks this
    /// address key; `None` for legacy keys unlocked with the mailbox password.
    #[serde(default)]
    pub token: Option<String>,
    /// Detached signature over the token, verified against the user keys.
    #[serde(default)]
    pub signature: Option<String>,
    /// Activation token for a not-yet-activated key, if present.
    #[serde(default)]
    pub activation: Option<String>,
    /// `1` if this is the primary key in its ring.
    #[serde(default)]
    pub primary: u8,
    /// `1` if the key is active (usable); inactive (`0`) keys are skipped.
    #[serde(default = "default_active")]
    pub active: u8,
    /// Key capability flags (encrypt/sign), if present.
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

/// The account's user-level key ring.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct User {
    /// Account identity returned by the authenticated user endpoint.
    #[serde(rename = "ID")]
    pub id: String,
    /// User keys (the root of the key hierarchy).
    pub keys: Vec<ApiKey>,
}

/// An email address and its key ring.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Address {
    /// The address's API ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// The email address.
    pub email: String,
    /// 1-based sort order from the API; the lowest-order address is the
    /// account's default/primary sending address.
    #[serde(default)]
    pub order: u32,
    /// The address's keys.
    #[serde(default)]
    pub keys: Vec<ApiKey>,
}

/// A per-key salt used to derive that key's passphrase from the mailbox password.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct KeySalt {
    /// The ID of the key this salt belongs to.
    #[serde(rename = "ID")]
    pub id: String,
    /// Base64-encoded salt; `None` for keys without one.
    pub key_salt: Option<String>,
}

/// An unlocked key, stored as armored material + the passphrase to re-import it.
#[derive(Clone)]
pub struct StoredKey {
    /// Armored PGP private key.
    pub armored: String,
    /// Passphrase that unlocks the armored key on re-import.
    pub passphrase: SecretString,
    /// The same key exported without passphrase protection when it was first
    /// unlocked. Re-importing it skips the passphrase derivation, which can
    /// take tens of milliseconds per key; `None` falls back to `armored`.
    pub unlocked: Option<Zeroizing<Vec<u8>>>,
}

/// Unlocked keys for one address.
#[derive(Clone)]
pub struct AddressKeys {
    /// The address's API ID.
    pub address_id: String,
    /// The email address.
    pub email: String,
    /// The address's unlocked keys, the primary first: messages are signed
    /// and drafts encrypted with the first one.
    pub keys: Vec<StoredKey>,
}

/// The account's unlocked address keys. User keys only open address keys,
/// so none is kept once they have.
#[derive(Clone)]
pub struct KeyStore {
    /// Unlocked keys per address, ascending by API `Order` (default-first).
    pub addresses: Vec<AddressKeys>,
}

impl KeyStore {
    /// Look up an address's keys by address ID.
    pub fn address(&self, address_id: &str) -> Option<&AddressKeys> {
        self.addresses.iter().find(|a| a.address_id == address_id)
    }

    /// Look up an address's keys by email (case-insensitive).
    pub fn address_for_email(&self, email: &str) -> Option<&AddressKeys> {
        let e = email.to_ascii_lowercase();
        self.addresses
            .iter()
            .find(|a| a.email.to_ascii_lowercase() == e)
    }

    /// Default sending address: the first address by API `Order` (matching the
    /// web client's default). `addresses` is built in ascending-order during
    /// unlock, so this is simply the first entry. Pick a different alias by
    /// passing a `from`/`--from` address (resolved via [`Self::address_for_email`]).
    pub fn primary_address(&self) -> Option<&AddressKeys> {
        self.addresses.first()
    }
}

/// Order addresses by ascending API `Order` (the default/primary sending
/// address first). The sort is stable, so addresses with an absent or equal
/// `Order` keep their original API order.
fn addresses_by_order(addresses: &[Address]) -> Vec<&Address> {
    let mut ordered: Vec<&Address> = addresses.iter().collect();
    ordered.sort_by_key(|a| a.order);
    ordered
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

/// Unlock the key hierarchy using the mailbox password + salts (login path).
/// Returns the `KeyStore` and the key password (`skp`, the salted key
/// password) for session persistence: the passphrase of the first user key
/// that opens, which also opens address keys without a signed token.
pub fn unlock<P: PGPProviderSync>(
    provider: &P,
    user: &User,
    addresses: &[Address],
    password: &SecretString,
    salts: &[KeySalt],
) -> Result<(KeyStore, SecretString)> {
    unlock_inner(provider, user, addresses, |k| {
        // A key without an entry of its own borrows the first salt. One
        // without a salt, from before key passwords were salted, opens with
        // the password itself, as in Proton's own clients.
        let salt = match salts.iter().find(|s| s.id == k.id) {
            Some(own) => own.key_salt.as_deref(),
            None => salts.first().and_then(|s| s.key_salt.as_deref()),
        };
        match salt.filter(|salt| !salt.is_empty()) {
            Some(salt_b64) => kdf::derive_key_passphrase(password, salt_b64),
            None => Ok(password.clone()),
        }
    })
}

/// Unlock the key hierarchy using a stored `skp` directly (resume path; no
/// password / KDF). The same passphrase is used for every user key.
pub fn unlock_with_skp<P: PGPProviderSync>(
    provider: &P,
    user: &User,
    addresses: &[Address],
    skp: &SecretString,
) -> Result<KeyStore> {
    Ok(unlock_inner(provider, user, addresses, |_k| Ok(skp.clone()))?.0)
}

fn unlock_inner<P: PGPProviderSync>(
    provider: &P,
    user: &User,
    addresses: &[Address],
    passphrase_for: impl Fn(&ApiKey) -> Result<SecretString>,
) -> Result<(KeyStore, SecretString)> {
    tracing::debug!(target: "ruston_core::crypto", user_keys = user.keys.len(), addresses = addresses.len(), "unlocking key hierarchy");
    let user_keys = unlock_user_keys(provider, user, passphrase_for)?;
    // Build in ascending `Order` so the default/primary sending address is
    // first; `primary_address()` then just takes `.first()`.
    let addresses: Vec<AddressKeys> = addresses_by_order(addresses)
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
    let (Some(token), Some(signature)) = (&key.token, &key.signature) else {
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

/// Decrypt an address-key token (PGP message to the user key) and verify its
/// detached signature with the user keys. Returns the token bytes (the address
/// key passphrase). Verification failure is an error (the key is then skipped).
fn decrypt_token<P: PGPProviderSync>(
    provider: &P,
    user_privs: &[P::PrivateKey],
    user_pubs: &[P::PublicKey],
    token_armored: &str,
    sig_armored: &str,
) -> Result<SecretString> {
    let verified = provider
        .new_decryptor()
        .with_decryption_keys(user_privs.iter())
        .decrypt(token_armored.as_bytes(), DataEncoding::Armor)
        .map_err(|e| Error::Crypto(format!("token decrypt failed: {e}")))?;
    // The token is the address key's passphrase: moved out, not copied, and
    // wiped once the passphrase holds it.
    let bytes = Zeroizing::new(verified.into_vec());

    let result = provider
        .new_verifier()
        .with_verification_keys(user_pubs.iter())
        .verify_detached(&bytes, sig_armored.as_bytes(), DataEncoding::Armor);
    if result.is_err() {
        return Err(Error::Crypto("address key token signature invalid".into()));
    }

    let s =
        std::str::from_utf8(&bytes).map_err(|e| Error::Crypto(format!("non-utf8 token: {e}")))?;
    Ok(SecretString::from(s.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlocked_address_keys_work_without_their_passphrase() {
        use proton_crypto::crypto::{KeyGenerator, KeyGeneratorAlgorithm, KeyGeneratorSync};

        let provider = crate::crypto::provider();
        let api_key = |id: &str, email: &str| {
            let generated = provider
                .new_key_generator()
                .with_user_id("Test", email)
                .with_algorithm(KeyGeneratorAlgorithm::ECC)
                .generate()
                .unwrap();
            let armored = provider
                .private_key_export(&generated, "skp", DataEncoding::Armor)
                .unwrap();
            ApiKey {
                id: id.into(),
                private_key: String::from_utf8(armored.as_ref().to_vec()).unwrap(),
                token: None,
                signature: None,
                activation: None,
                primary: 1,
                active: 1,
                flags: None,
            }
        };
        let user = User {
            id: "user".into(),
            keys: vec![api_key("user-key", "user@proton.me")],
        };
        // Without a token, the address key opens with the user key's passphrase.
        let address = Address {
            id: "address".into(),
            email: "me@proton.me".into(),
            order: 1,
            keys: vec![api_key("address-key", "me@proton.me")],
        };
        let mut store =
            unlock_with_skp(&provider, &user, &[address], &SecretString::from("skp")).unwrap();

        let stored = &mut store.addresses[0].keys[0];
        assert!(
            stored.unlocked.is_some(),
            "unlocking keeps an unlocked copy"
        );
        // Every later operation uses that copy, not the passphrase.
        stored.passphrase = SecretString::from("wrong");
        stored.armored.clear();
        let address = &store.addresses[0];
        let draft = crate::crypto::encrypt_self_draft(&provider, address, "hello").unwrap();
        let (body, _) = crate::crypto::decrypt_body(&provider, address, &[], &draft).unwrap();
        assert_eq!(body, "hello");
    }

    /// A fresh key for `email` as the API lists it, locked with `passphrase`,
    /// and its armored public half.
    fn listed_key(
        id: &str,
        email: &str,
        passphrase: &str,
        primary: u8,
        flags: Option<u32>,
    ) -> (ApiKey, String) {
        use proton_crypto::crypto::{KeyGenerator, KeyGeneratorAlgorithm, KeyGeneratorSync};

        let provider = crate::crypto::provider();
        let generated = provider
            .new_key_generator()
            .with_user_id("Test", email)
            .with_algorithm(KeyGeneratorAlgorithm::ECC)
            .generate()
            .unwrap();
        let armored = provider
            .private_key_export(&generated, passphrase, DataEncoding::Armor)
            .unwrap();
        let public = provider.private_key_to_public_key(&generated).unwrap();
        let public = provider
            .public_key_export(&public, DataEncoding::Armor)
            .unwrap();
        (
            ApiKey {
                id: id.into(),
                private_key: String::from_utf8(armored.as_ref().to_vec()).unwrap(),
                token: None,
                signature: None,
                activation: None,
                primary,
                active: 1,
                flags,
            },
            String::from_utf8(public.as_ref().to_vec()).unwrap(),
        )
    }

    /// Unlocks one address holding `keys`, all opened with the user key's
    /// passphrase as legacy keys are.
    fn unlock_address(keys: Vec<ApiKey>) -> KeyStore {
        let (user_key, _) = listed_key("user-key", "user@proton.me", "skp", 1, None);
        let user = User {
            id: "user".into(),
            keys: vec![user_key],
        };
        let address = Address {
            id: "address".into(),
            email: "me@proton.me".into(),
            order: 1,
            keys,
        };
        unlock_with_skp(
            &crate::crypto::provider(),
            &user,
            &[address],
            &SecretString::from("skp"),
        )
        .unwrap()
    }

    #[test]
    fn the_primary_address_key_comes_first_and_signs() {
        let (other, _) = listed_key("other", "me@proton.me", "skp", 0, Some(3));
        let (primary, primary_public) = listed_key("primary", "me@proton.me", "skp", 1, Some(3));

        // The API happens to list the primary key second.
        let store = unlock_address(vec![other, primary.clone()]);

        let address = &store.addresses[0];
        assert_eq!(address.keys[0].armored, primary.private_key);
        let provider = crate::crypto::provider();
        let draft = crate::crypto::encrypt_self_draft(&provider, address, "hello").unwrap();
        let (_, verdict) =
            crate::crypto::decrypt_body(&provider, address, &[primary_public], &draft).unwrap();
        assert_eq!(verdict, crate::crypto::Verdict::Verified);
    }

    #[test]
    fn a_sound_key_signs_before_a_compromised_one_when_the_primary_stays_locked() {
        // Flags 2: it may still encrypt, but Proton marked it compromised.
        let (compromised, _) = listed_key("compromised", "me@proton.me", "skp", 0, Some(2));
        let (sound, _) = listed_key("sound", "me@proton.me", "skp", 0, Some(3));
        // Locked with another passphrase, so it does not open.
        let (primary, _) = listed_key("primary", "me@proton.me", "other", 1, Some(3));

        let store = unlock_address(vec![compromised.clone(), sound.clone(), primary]);

        let unlocked: Vec<&str> = store.addresses[0]
            .keys
            .iter()
            .map(|key| key.armored.as_str())
            .collect();
        assert_eq!(
            unlocked,
            [sound.private_key.as_str(), compromised.private_key.as_str()]
        );
    }

    #[test]
    fn primary_address_is_first() {
        // `addresses` is built ascending-by-`Order` during unlock, so the
        // primary/default sending address is simply the first entry.
        let ks = KeyStore {
            addresses: vec![
                AddressKeys {
                    address_id: "1".into(),
                    email: "me@protonmail.ch".into(),
                    keys: vec![],
                },
                AddressKeys {
                    address_id: "2".into(),
                    email: "me@pm.me".into(),
                    keys: vec![],
                },
            ],
        };
        assert_eq!(ks.primary_address().unwrap().address_id, "1");
        assert_eq!(ks.primary_address().unwrap().email, "me@protonmail.ch");
    }

    #[test]
    fn addresses_by_order_sorts_ascending_stably() {
        // Exercises the shipped ordering used by `unlock_inner`: an out-of-order
        // API response yields default-first addresses, and equal `Order` keeps
        // the original API sequence.
        let addrs = [
            Address {
                id: "b".into(),
                email: "second@pm.me".into(),
                order: 2,
                keys: vec![],
            },
            Address {
                id: "a".into(),
                email: "first@protonmail.ch".into(),
                order: 1,
                keys: vec![],
            },
            Address {
                id: "c".into(),
                email: "tie@proton.me".into(),
                order: 1,
                keys: vec![],
            },
        ];
        // Input order is [b(2), a(1), c(1)]; a stable ascending sort yields
        // [a(1), c(1), b(2)] — the two `order == 1` entries keep their API order.
        let ordered = addresses_by_order(&addrs);
        assert_eq!(ordered[0].id, "a");
        assert_eq!(ordered[0].email, "first@protonmail.ch");
        assert_eq!(ordered[1].id, "c");
        assert_eq!(ordered[2].id, "b");
    }

    #[test]
    fn apikey_active_defaults_to_one() {
        let k: ApiKey = serde_json::from_value(serde_json::json!({
            "ID": "k1", "PrivateKey": "ARM"
        }))
        .unwrap();
        assert_eq!(k.active, 1);
    }

    /// The mailbox password the sign-in tests unlock with.
    const MAILBOX_PASSWORD: &str = "mailbox password";

    /// A base64 key salt of sixteen `byte`s.
    fn salt(byte: u8) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode([byte; 16])
    }

    /// The passphrase that the mailbox password and `salt_b64` derive.
    fn derived(salt_b64: &str) -> String {
        kdf::derive_key_passphrase(&SecretString::from(MAILBOX_PASSWORD), salt_b64)
            .unwrap()
            .expose_secret()
            .to_owned()
    }

    fn key_salt(id: &str, key_salt: Option<String>) -> KeySalt {
        KeySalt {
            id: id.into(),
            key_salt,
        }
    }

    /// An address key's token as Proton lists it: `passphrase` encrypted to
    /// the key `to`, with a detached signature over it made by `signer`.
    /// Each key comes with the passphrase that opens it.
    fn token(passphrase: &str, to: (&ApiKey, &str), signer: (&ApiKey, &str)) -> (String, String) {
        use proton_crypto::crypto::{Encryptor, EncryptorSync, PGPMessage, Signer, SignerSync};

        let provider = crate::crypto::provider();
        let open = |(key, key_passphrase): (&ApiKey, &str)| {
            provider
                .private_key_import(
                    key.private_key.as_bytes(),
                    key_passphrase.as_bytes(),
                    DataEncoding::Armor,
                )
                .unwrap()
        };
        let recipient = provider.private_key_to_public_key(&open(to)).unwrap();
        let sealed = provider
            .new_encryptor()
            .with_encryption_key(&recipient)
            .encrypt(passphrase.as_bytes())
            .unwrap()
            .armor()
            .unwrap();
        let signature = provider
            .new_signer()
            .with_signing_key(&open(signer))
            .sign_detached(passphrase.as_bytes(), DataEncoding::Armor)
            .unwrap();
        (
            String::from_utf8(sealed).unwrap(),
            String::from_utf8(signature).unwrap(),
        )
    }

    #[test]
    fn signing_in_opens_user_keys_by_their_salt_and_address_keys_by_their_token() {
        let user_password = derived(&salt(1));
        let (user_key, _) = listed_key("user-key", "user@proton.me", &user_password, 1, None);
        // Inactive keys are never tried, though these two would open.
        let (mut inactive_user_key, _) =
            listed_key("inactive", "user@proton.me", &derived(&salt(2)), 0, None);
        inactive_user_key.active = 0;
        let (mut inactive_address_key, _) =
            listed_key("inactive-address", "me@proton.me", &user_password, 0, None);
        inactive_address_key.active = 0;
        let (stranger, _) = listed_key("stranger", "else@proton.me", "stranger", 1, None);
        let user_side = (&user_key, user_password.as_str());

        // A token the user key signed opens its key.
        let (mut signed, _) = listed_key("signed", "me@proton.me", "token passphrase", 1, None);
        let (sealed, signature) = token("token passphrase", user_side, user_side);
        (signed.token, signed.signature) = (Some(sealed), Some(signature));
        // One that another key signed leaves its key locked, though both what
        // it holds and the key password would open it.
        let (mut forged, _) = listed_key("forged", "me@proton.me", &user_password, 0, None);
        let (sealed, signature) = token(&user_password, user_side, (&stranger, "stranger"));
        (forged.token, forged.signature) = (Some(sealed), Some(signature));
        // A token without a signature is ignored: the key opens as a legacy
        // one, with the user key's passphrase.
        let (mut unsigned, _) = listed_key("unsigned", "me@proton.me", &user_password, 0, None);
        unsigned.token = Some(token("ignored", user_side, user_side).0);

        let user = User {
            id: "user".into(),
            keys: vec![inactive_user_key, user_key.clone()],
        };
        let address = Address {
            id: "address".into(),
            email: "me@proton.me".into(),
            order: 1,
            keys: vec![
                signed.clone(),
                forged,
                unsigned.clone(),
                inactive_address_key,
            ],
        };
        // Another key's salt comes first: each key is opened with its own.
        let salts = [
            key_salt("other", Some(salt(2))),
            key_salt("user-key", Some(salt(1))),
        ];
        let (store, key_password) = unlock(
            &crate::crypto::provider(),
            &user,
            &[address],
            &SecretString::from(MAILBOX_PASSWORD),
            &salts,
        )
        .unwrap();

        // The inactive user key, listed first, would have set the key password.
        assert_eq!(key_password.expose_secret(), user_password);
        let address_keys: Vec<(&str, &str)> = store.addresses[0]
            .keys
            .iter()
            .map(|key| (key.armored.as_str(), key.passphrase.expose_secret()))
            .collect();
        assert_eq!(
            address_keys,
            [
                (signed.private_key.as_str(), "token passphrase"),
                (unsigned.private_key.as_str(), user_password.as_str()),
            ]
        );
    }

    /// An account whose one user key and one address key are locked with
    /// `passphrase`, the address key as legacy keys are.
    fn account_locked_with(passphrase: &str) -> (User, [Address; 1]) {
        let (user_key, _) = listed_key("user-key", "user@proton.me", passphrase, 1, None);
        let (legacy, _) = listed_key("legacy", "me@proton.me", passphrase, 1, None);
        let user = User {
            id: "user".into(),
            keys: vec![user_key],
        };
        let addresses = [Address {
            id: "address".into(),
            email: "me@proton.me".into(),
            order: 1,
            keys: vec![legacy],
        }];
        (user, addresses)
    }

    #[test]
    fn a_user_key_without_an_entry_of_its_own_borrows_the_first_salt() {
        let user_password = derived(&salt(3));
        let (user, addresses) = account_locked_with(&user_password);
        let provider = crate::crypto::provider();
        let password = SecretString::from(MAILBOX_PASSWORD);

        let salts = [key_salt("first", Some(salt(3)))];
        let Ok((_, key_password)) = unlock(&provider, &user, &addresses, &password, &salts) else {
            panic!("the first salt opens the user key");
        };
        assert_eq!(key_password.expose_secret(), user_password);
    }

    #[test]
    fn a_user_key_without_a_salt_opens_with_the_password_itself() {
        // Keys from before key passwords were salted open with the mailbox
        // password as it is, as Proton's own clients open them.
        let (user, addresses) = account_locked_with(MAILBOX_PASSWORD);
        let provider = crate::crypto::provider();
        let password = SecretString::from(MAILBOX_PASSWORD);

        for salts in [
            vec![key_salt("first", Some(salt(3))), key_salt("user-key", None)],
            vec![key_salt("user-key", Some(String::new()))],
            Vec::new(),
        ] {
            let Ok((_, key_password)) = unlock(&provider, &user, &addresses, &password, &salts)
            else {
                panic!("the password opens a key without a salt, with salts {salts:?}");
            };
            assert_eq!(key_password.expose_secret(), MAILBOX_PASSWORD);
        }
    }

    #[test]
    fn a_salt_that_cannot_be_read_leaves_out_only_its_key() {
        let user_password = derived(&salt(4));
        let (mut user, addresses) = account_locked_with(&user_password);
        let (unreadable, _) = listed_key("unreadable", "user@proton.me", &user_password, 0, None);
        user.keys.insert(0, unreadable);
        let provider = crate::crypto::provider();
        let password = SecretString::from(MAILBOX_PASSWORD);

        let salts = [
            key_salt("unreadable", Some("not base64!".into())),
            key_salt("user-key", Some(salt(4))),
        ];
        let Ok((store, key_password)) = unlock(&provider, &user, &addresses, &password, &salts)
        else {
            panic!("the key with a readable salt still opens");
        };
        assert_eq!(key_password.expose_secret(), user_password);
        assert_eq!(store.addresses[0].keys.len(), 1);
    }

    #[test]
    fn the_key_password_is_the_first_user_keys_that_opens() {
        let second_password = derived(&salt(5));
        let third_password = derived(&salt(6));
        // The primary user key is listed first, but no longer opens.
        let (stale, _) = listed_key("stale", "user@proton.me", "changed since", 1, None);
        let (second, _) = listed_key("second", "user@proton.me", &second_password, 0, None);
        // A later key that opens too does not change it, but still opens the
        // address keys whose tokens it holds.
        let (third, _) = listed_key("third", "user@proton.me", &third_password, 0, None);
        let (legacy, _) = listed_key("legacy", "me@proton.me", &second_password, 1, None);
        let (mut by_third, _) = listed_key("by-third", "me@proton.me", "third's token", 0, None);
        let third_side = (&third, third_password.as_str());
        let (sealed, signature) = token("third's token", third_side, third_side);
        (by_third.token, by_third.signature) = (Some(sealed), Some(signature));
        let user = User {
            id: "user".into(),
            keys: vec![stale, second, third.clone()],
        };
        let address = Address {
            id: "address".into(),
            email: "me@proton.me".into(),
            order: 1,
            keys: vec![legacy.clone(), by_third.clone()],
        };
        let salts = [
            key_salt("stale", Some(salt(4))),
            key_salt("second", Some(salt(5))),
            key_salt("third", Some(salt(6))),
        ];

        let Ok((store, key_password)) = unlock(
            &crate::crypto::provider(),
            &user,
            &[address],
            &SecretString::from(MAILBOX_PASSWORD),
            &salts,
        ) else {
            panic!("the second user key opens");
        };

        assert_eq!(key_password.expose_secret(), second_password);
        let address_keys: Vec<&str> = store.addresses[0]
            .keys
            .iter()
            .map(|key| key.armored.as_str())
            .collect();
        assert_eq!(
            address_keys,
            [legacy.private_key.as_str(), by_third.private_key.as_str()]
        );
    }

    #[test]
    fn unlocking_fails_when_no_user_key_or_no_address_key_opens() {
        let (user_key, _) = listed_key("user-key", "user@proton.me", "skp", 1, None);
        let (stranger, _) = listed_key("stranger", "else@proton.me", "stranger", 1, None);
        let (mut forged, _) = listed_key("forged", "me@proton.me", "forged", 1, None);
        let (sealed, signature) = token("forged", (&user_key, "skp"), (&stranger, "stranger"));
        (forged.token, forged.signature) = (Some(sealed), Some(signature));
        let user = User {
            id: "user".into(),
            keys: vec![user_key],
        };
        let addresses = [Address {
            id: "address".into(),
            email: "me@proton.me".into(),
            order: 1,
            keys: vec![forged],
        }];
        let provider = crate::crypto::provider();

        let wrong = unlock_with_skp(&provider, &user, &addresses, &SecretString::from("wrong"));
        assert!(matches!(wrong, Err(Error::UserKeysLocked)));
        let none_opens = unlock_with_skp(&provider, &user, &addresses, &SecretString::from("skp"));
        assert!(matches!(
            none_opens,
            Err(Error::Crypto(ref message)) if message == "no address key could be unlocked"
        ));
    }

    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_key_that_does_not_unlock_is_logged_with_why() {
        use std::sync::{Arc, Mutex};

        let user_password = derived(&salt(7));
        let (stale, _) = listed_key("stale", "user@proton.me", "changed since", 1, None);
        let (user_key, _) = listed_key("user-key", "user@proton.me", &user_password, 0, None);
        let (legacy, _) = listed_key("legacy", "me@proton.me", &user_password, 1, None);
        // Its token checks out, but the key is locked with another passphrase.
        let (mut mismatched, _) = listed_key("mismatched", "me@proton.me", "another", 0, None);
        let user_side = (&user_key, user_password.as_str());
        let (sealed, signature) = token("the token's", user_side, user_side);
        (mismatched.token, mismatched.signature) = (Some(sealed), Some(signature));
        let user = User {
            id: "user".into(),
            keys: vec![stale, user_key.clone()],
        };
        let address = Address {
            id: "address".into(),
            email: "me@proton.me".into(),
            order: 1,
            keys: vec![legacy, mismatched],
        };
        let salts = [
            key_salt("stale", Some(salt(8))),
            key_salt("user-key", Some(salt(7))),
        ];
        let captured = Arc::new(Mutex::new(Vec::new()));
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(move || Capture(writer.clone()))
            .finish();
        // While only one dispatcher is registered, tracing asks the default of
        // whichever thread first reaches a callsite whether it is wanted, so
        // one another test reached first would stay unwanted here. With a
        // second one registered, it asks every registered dispatcher.
        let _second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());

        let unlocked = tracing::subscriber::with_default(subscriber, || {
            unlock(
                &crate::crypto::provider(),
                &user,
                &[address],
                &SecretString::from(MAILBOX_PASSWORD),
                &salts,
            )
        });

        let Ok((store, _)) = unlocked else {
            panic!("the other user key and address key open");
        };
        assert_eq!(store.addresses[0].keys.len(), 1);
        let log = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        let line = |key_id: &str| {
            log.lines()
                .find(|line| line.contains(&format!("key_id={key_id}")) && line.contains("error="))
                .unwrap_or_else(|| panic!("no line for {key_id} in:\n{log}"))
                .to_owned()
        };
        // Another user key opening is enough, so a user key is only noted.
        let stale = line("stale");
        assert!(
            stale.contains("DEBUG") && stale.contains("user key did not unlock"),
            "{stale}"
        );
        let mismatched = line("mismatched");
        assert!(
            mismatched.contains("WARN") && mismatched.contains("address key did not unlock"),
            "{mismatched}"
        );
    }
}
