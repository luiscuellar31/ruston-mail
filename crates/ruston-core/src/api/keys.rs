//! Keys and the account they belong to: the user, its addresses and their
//! key salts, and the public keys of the people mail is sent to.

use crate::crypto::keys::{Address, KeySalt, User};
use crate::error::Result;
use crate::transport::{Doer, Request};
use serde::Deserialize;
use serde::de::IgnoredAny;

#[derive(Deserialize)]
struct SaltsResp {
    #[serde(rename = "KeySalts", default)]
    key_salts: Vec<KeySalt>,
}

#[derive(Deserialize)]
struct UserResp {
    #[serde(rename = "User")]
    user: User,
}

#[derive(Deserialize)]
struct AddressesResp {
    #[serde(rename = "Addresses", default)]
    addresses: Vec<Address>,
}

/// One of a recipient's public keys, as `/core/v4/keys/all` lists them.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ApiPublicKey {
    /// The key, armored.
    pub public_key: String,
    /// What the key may be used for, as
    /// [`key_flag`](crate::model::enums::key_flag) bits.
    #[serde(default)]
    pub flags: u32,
    /// `1` when it is the address's primary key.
    #[serde(default)]
    pub primary: u8,
    /// Where Proton found the key, such as among its own or by WKD.
    #[serde(default)]
    pub source: u8,
}

impl ApiPublicKey {
    /// Whether the address uses this key before its others.
    pub fn is_primary(&self) -> bool {
        self.primary == 1
    }
}

#[derive(Deserialize)]
struct AddrKeys {
    #[serde(rename = "Keys", default)]
    keys: Vec<ApiPublicKey>,
}

#[derive(Deserialize)]
struct AllKeysResp {
    #[serde(rename = "Address", default)]
    address: Option<AddrKeys>,
    #[serde(rename = "Unverified", default)]
    unverified: Option<AddrKeys>,
    #[serde(rename = "RecipientType", default)]
    recipient_type: u8,
}

/// A recipient's public keys: those of an address at Proton, which Proton
/// vouches for, and those found elsewhere, such as by WKD, for PGP mail to
/// someone outside Proton.
#[derive(Debug, Clone)]
pub struct RecipientKeys {
    /// Recipient type, one of [`recipient_type`](crate::model::enums::recipient_type).
    pub recipient_type: u8,
    /// The keys Proton vouches for.
    pub keys: Vec<ApiPublicKey>,
    /// The keys found elsewhere, which Proton does not vouch for.
    pub unverified: Vec<ApiPublicKey>,
}

impl RecipientKeys {
    /// Whether the recipient's address is at Proton, as keys Proton vouches
    /// for show.
    pub fn is_internal(&self) -> bool {
        !self.keys.is_empty()
    }
}

/// The salts each user key's passphrase is derived with.
pub async fn get_key_salts<D: Doer>(d: &D) -> Result<Vec<KeySalt>> {
    let SaltsResp { key_salts } = d.decode(Request::get("/core/v4/keys/salts")).await?;
    Ok(key_salts)
}

/// The signed-in user, with its user keys still locked.
pub async fn get_user<D: Doer>(d: &D) -> Result<User> {
    let UserResp { user } = d.decode(Request::get("/core/v4/users")).await?;
    Ok(user)
}

/// The account's addresses, with their address keys still locked.
pub async fn get_addresses<D: Doer>(d: &D) -> Result<Vec<Address>> {
    let AddressesResp { addresses } = d.decode(Request::get("/core/v4/addresses")).await?;
    Ok(addresses)
}

#[derive(Deserialize)]
struct ModulusResp {
    #[serde(rename = "Modulus")]
    modulus: String,
    #[serde(rename = "ModulusID")]
    modulus_id: String,
}

/// A new SRP modulus and its ID, from which mail sent with a password to
/// someone outside Proton gets its verifier.
pub async fn get_modulus<D: Doer>(d: &D) -> Result<(String, String)> {
    d.decode(Request::get("/core/v4/auth/modulus"))
        .await
        .map(|found: ModulusResp| (found.modulus, found.modulus_id))
}

/// Changes the display name, the signature or both of the address `id`;
/// what is `None` stays as it is.
pub async fn update_address<D: Doer>(
    d: &D,
    id: &str,
    display_name: Option<&str>,
    signature: Option<&str>,
) -> Result<()> {
    let changes: serde_json::Map<String, serde_json::Value> =
        [("DisplayName", display_name), ("Signature", signature)]
            .into_iter()
            .filter_map(|(field, value)| Some((field.to_owned(), value?.into())))
            .collect();
    let request = Request::put(format!("/core/v4/addresses/{id}")).json(changes.into());
    d.decode::<IgnoredAny>(request).await?;
    Ok(())
}

/// The public keys Proton has for `email`, those it vouches for and those
/// found elsewhere.
pub async fn get_all_public_keys<D: Doer>(d: &D, email: &str) -> Result<RecipientKeys> {
    let lookup = Request::get("/core/v4/keys/all").query("Email", email);
    let found: AllKeysResp = d.decode(lookup).await?;
    // A group Proton has no keys in can be left out of its answer.
    let keys_of = |group: Option<AddrKeys>| group.map(|group| group.keys).unwrap_or_default();
    Ok(RecipientKeys {
        recipient_type: found.recipient_type,
        keys: keys_of(found.address),
        unverified: keys_of(found.unverified),
    })
}

/// Pick the primary key mail may be encrypted to, else the first such key:
/// one that is not obsolete, on an address whose owner has not turned email
/// encryption off.
pub fn pick_encryption_key(keys: &[ApiPublicKey]) -> Option<&ApiPublicKey> {
    use crate::model::enums::key_flag;
    keys.iter()
        .find(|k| k.is_primary() && key_flag::can_encrypt_mail(k.flags))
        .or_else(|| keys.iter().find(|k| key_flag::can_encrypt_mail(k.flags)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::enums::key_flag::{
        EMAIL_NO_ENCRYPT, EMAIL_NO_SIGN, NOT_COMPROMISED, NOT_OBSOLETE,
    };
    use crate::model::enums::recipient_type;

    /// The flags of a key mail may be encrypted to.
    const USABLE: u32 = NOT_COMPROMISED | NOT_OBSOLETE;

    /// A key that is not its address's primary one, armored as `name` so a
    /// test can tell which key was picked.
    fn secondary(name: &str, flags: u32) -> ApiPublicKey {
        ApiPublicKey {
            public_key: name.into(),
            flags,
            primary: 0,
            source: 0,
        }
    }

    /// Like [`secondary`], but its address's primary key.
    fn primary(name: &str, flags: u32) -> ApiPublicKey {
        ApiPublicKey {
            primary: 1,
            ..secondary(name, flags)
        }
    }

    /// The name of the key mail would be encrypted to, among `keys`.
    fn picked(keys: &[ApiPublicKey]) -> Option<&str> {
        pick_encryption_key(keys).map(|key| key.public_key.as_str())
    }

    #[test]
    fn authenticated_user_requires_the_account_id() {
        let response: UserResp = serde_json::from_value(serde_json::json!({
            "User": {"ID": "account-id", "Keys": []}
        }))
        .unwrap();
        assert_eq!(response.user.id, "account-id");
        assert!(
            serde_json::from_value::<UserResp>(serde_json::json!({
                "User": {"Keys": []}
            }))
            .is_err()
        );
    }

    #[test]
    fn the_primary_key_mail_may_be_encrypted_to_is_picked_first() {
        // Not compromised but obsolete: it still verifies, but takes no mail.
        let obsolete = NOT_COMPROMISED;
        let every = [
            primary("obsolete", obsolete),
            secondary("secondary", USABLE),
            primary("primary", USABLE),
        ];
        assert_eq!(picked(&every), Some("primary"));
        assert_eq!(picked(&every[..2]), Some("secondary"));
        assert_eq!(picked(&every[..1]), None);
    }

    #[test]
    fn keys_whose_address_turned_email_encryption_off_are_skipped() {
        let keys = [
            primary("off", USABLE | EMAIL_NO_ENCRYPT),
            secondary("on", USABLE),
        ];
        assert_eq!(picked(&keys), Some("on"));
        for flags in [
            USABLE | EMAIL_NO_ENCRYPT,
            USABLE | EMAIL_NO_ENCRYPT | EMAIL_NO_SIGN,
        ] {
            assert_eq!(picked(&[primary("off", flags)]), None);
        }
        // Only signing is off: mail is still encrypted to it.
        assert_eq!(
            picked(&[primary("unsigned", USABLE | EMAIL_NO_SIGN)]),
            Some("unsigned")
        );
    }

    #[test]
    fn a_recipient_is_internal_when_proton_vouches_for_one_of_its_keys() {
        let internal = RecipientKeys {
            recipient_type: recipient_type::INTERNAL,
            keys: vec![primary("vouched", USABLE)],
            unverified: Vec::new(),
        };
        assert!(internal.is_internal());
        let external = RecipientKeys {
            recipient_type: recipient_type::EXTERNAL,
            keys: Vec::new(),
            unverified: vec![primary("found", USABLE)],
        };
        assert!(!external.is_internal());
    }
}
