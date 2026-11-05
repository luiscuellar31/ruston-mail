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
    /// Where it sorts among the account's addresses, from 1. The lowest is
    /// the address mail goes out from unless another is chosen.
    #[serde(default)]
    pub order: u32,
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
    /// The address's unlocked keys, the primary first: messages are signed
    /// and drafts encrypted with the first one.
    pub keys: Vec<StoredKey>,
}

/// The account's unlocked address keys. User keys only open address keys,
/// so none is kept once they have.
#[derive(Clone)]
pub struct KeyStore {
    /// Each address whose keys opened, in Proton's `Order`: the default
    /// address first.
    pub addresses: Vec<AddressKeys>,
}

impl KeyStore {
    /// The keys of the address whose API ID is `address_id`.
    pub fn address(&self, address_id: &str) -> Option<&AddressKeys> {
        self.addresses
            .iter()
            .find(|keys| keys.address_id == address_id)
    }

    /// The keys of `email`, whatever the case of its ASCII letters.
    pub fn address_for_email(&self, email: &str) -> Option<&AddressKeys> {
        self.addresses
            .iter()
            .find(|keys| keys.email.eq_ignore_ascii_case(email))
    }

    /// The address mail is sent from unless another is chosen, as in
    /// Proton's web client: the one first in Proton's `Order`, which
    /// unlocking puts first. A `from` address picks another through
    /// [`Self::address_for_email`].
    pub fn primary_address(&self) -> Option<&AddressKeys> {
        self.addresses.first()
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
    fn the_store_finds_an_address_by_id_or_by_email_in_any_ascii_case() {
        let held = |id: &str, email: &str| AddressKeys {
            address_id: id.into(),
            email: email.into(),
            keys: Vec::new(),
        };
        let store = KeyStore {
            addresses: vec![
                held("default", "me@proton.me"),
                held("alias", "älias@pm.me"),
            ],
        };
        fn found(address: Option<&AddressKeys>) -> Option<&str> {
            address.map(|keys| keys.address_id.as_str())
        }

        // Unlocking puts the default address first.
        assert_eq!(found(store.primary_address()), Some("default"));
        assert_eq!(found(store.address("alias")), Some("alias"));
        assert_eq!(found(store.address("ALIAS")), None);
        assert_eq!(
            found(store.address_for_email("ME@Proton.ME")),
            Some("default")
        );
        assert_eq!(found(store.address_for_email("äLIAS@PM.ME")), Some("alias"));
        // Only ASCII letters match in either case.
        assert_eq!(found(store.address_for_email("ÄLIAS@pm.me")), None);
        let empty = KeyStore {
            addresses: Vec::new(),
        };
        assert!(empty.primary_address().is_none());
    }

    #[test]
    fn addresses_unlock_by_their_order_and_ties_keep_the_api_order() {
        let (user_key, _) = listed_key("user-key", "user@proton.me", "skp", 1, None);
        let user = User {
            id: "user".into(),
            keys: vec![user_key],
        };
        // Listed as Proton might: out of order, and two sharing one.
        let addresses = [("second", 2), ("first", 1), ("tied", 1)].map(|(id, order)| {
            let email = format!("{id}@proton.me");
            let (key, _) = listed_key(id, &email, "skp", 1, None);
            Address {
                id: id.into(),
                email,
                order,
                keys: vec![key],
            }
        });

        let provider = crate::crypto::provider();
        let store = unlock_with_skp(&provider, &user, &addresses, &SecretString::from("skp"));
        let unlocked: Vec<String> = store
            .unwrap()
            .addresses
            .into_iter()
            .map(|keys| keys.address_id)
            .collect();
        assert_eq!(unlocked, ["first", "tied", "second"]);
    }

    #[test]
    fn a_listed_key_is_active_and_secondary_unless_it_says_otherwise() {
        let listed = |json: serde_json::Value| serde_json::from_value::<ApiKey>(json).unwrap();

        let plain = listed(serde_json::json!({ "ID": "k1", "PrivateKey": "ARMORED" }));
        assert_eq!(
            (plain.id.as_str(), plain.private_key.as_str()),
            ("k1", "ARMORED")
        );
        assert!(plain.is_active() && !plain.is_primary());
        assert_eq!((plain.active, plain.primary), (1, 0));
        assert!(plain.token.is_none() && plain.signature.is_none());
        assert!(plain.activation.is_none() && plain.flags.is_none());

        let full = listed(serde_json::json!({
            "ID": "k2", "PrivateKey": "ARMORED", "Token": "token", "Signature": "signature",
            "Activation": "activation", "Primary": 1, "Active": 0, "Flags": 3
        }));
        assert!(!full.is_active() && full.is_primary());
        assert_eq!(full.token.as_deref(), Some("token"));
        assert_eq!(full.signature.as_deref(), Some("signature"));
        assert_eq!(full.activation.as_deref(), Some("activation"));
        assert_eq!(full.flags, Some(3));
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

    #[test]
    fn a_key_with_an_empty_token_or_signature_opens_as_a_legacy_one() {
        let (mut empty_token, _) = listed_key("empty-token", "me@proton.me", "skp", 1, None);
        (empty_token.token, empty_token.signature) = (Some(String::new()), Some("sig".into()));
        let (mut empty_signature, _) =
            listed_key("empty-signature", "me@proton.me", "skp", 0, None);
        (empty_signature.token, empty_signature.signature) =
            (Some("token".into()), Some(String::new()));

        let store = unlock_address(vec![empty_token.clone(), empty_signature.clone()]);

        // Both open with the key password, as keys without a token do.
        let opened: Vec<(&str, &str)> = store.addresses[0]
            .keys
            .iter()
            .map(|key| (key.armored.as_str(), key.passphrase.expose_secret()))
            .collect();
        assert_eq!(
            opened,
            [
                (empty_token.private_key.as_str(), "skp"),
                (empty_signature.private_key.as_str(), "skp"),
            ]
        );
    }

    #[test]
    fn a_token_opens_only_with_a_user_keys_signature_over_it() {
        let provider = crate::crypto::provider();
        let (user_key, _) = listed_key("user-key", "user@proton.me", "skp", 1, None);
        let (stranger, _) = listed_key("stranger", "else@proton.me", "stranger", 1, None);
        let private = provider
            .private_key_import(user_key.private_key.as_bytes(), b"skp", DataEncoding::Armor)
            .unwrap();
        let public = provider.private_key_to_public_key(&private).unwrap();
        let open = |sealed: &str, signature: &str| {
            let (privs, pubs) = (
                std::slice::from_ref(&private),
                std::slice::from_ref(&public),
            );
            decrypt_token(&provider, privs, pubs, sealed, signature)
        };
        let user_side = (&user_key, "skp");

        let (sealed, signature) = token("the passphrase", user_side, user_side);
        assert_eq!(
            open(&sealed, &signature).unwrap().expose_secret(),
            "the passphrase"
        );
        let (sealed, forged) = token("the passphrase", user_side, (&stranger, "stranger"));
        assert!(matches!(
            open(&sealed, &forged),
            Err(Error::Crypto(message)) if message == "address key token signature invalid"
        ));
        assert!(matches!(
            open("not a message", &signature),
            Err(Error::Crypto(message)) if message.starts_with("token decrypt failed: ")
        ));
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
    fn a_user_key_without_a_salt_of_its_own_borrows_the_primary_keys() {
        // Proton locks every user key with the key password the primary
        // key's salt derives.
        let key_password = derived(&salt(3));
        let (primary, _) = listed_key("primary", "user@proton.me", &key_password, 1, None);
        let (secondary, _) = listed_key("secondary", "user@proton.me", &key_password, 0, None);
        // Only the secondary key opens this address key's token.
        let (mut by_secondary, _) = listed_key("by-secondary", "me@proton.me", "token", 1, None);
        let secondary_side = (&secondary, key_password.as_str());
        let (sealed, signature) = token("token", secondary_side, secondary_side);
        (by_secondary.token, by_secondary.signature) = (Some(sealed), Some(signature));
        let user = User {
            id: "user".into(),
            keys: vec![primary, secondary],
        };
        let addresses = [Address {
            id: "address".into(),
            email: "me@proton.me".into(),
            order: 1,
            keys: vec![by_secondary.clone()],
        }];
        let provider = crate::crypto::provider();
        let password = SecretString::from(MAILBOX_PASSWORD);

        // Another key's salt comes first, and the secondary key has no entry,
        // or one without a salt.
        for salts in [
            vec![
                key_salt("other", Some(salt(9))),
                key_salt("primary", Some(salt(3))),
            ],
            vec![
                key_salt("other", Some(salt(9))),
                key_salt("primary", Some(salt(3))),
                key_salt("secondary", None),
            ],
        ] {
            let Ok((store, _)) = unlock(&provider, &user, &addresses, &password, &salts) else {
                panic!("the primary key's salt opens the secondary key, with salts {salts:?}");
            };
            assert_eq!(store.addresses[0].keys[0].armored, by_secondary.private_key);
        }
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
            // This key, the primary, has no entry: no other key's salt is
            // borrowed.
            vec![key_salt("other", Some(salt(3)))],
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
