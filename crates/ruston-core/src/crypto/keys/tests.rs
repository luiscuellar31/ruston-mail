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
        status: 1,
        send: 1,
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
        status: 1,
        send: 1,
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
        can_send: true,
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
            status: 1,
            send: 1,
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
fn mail_goes_out_only_from_addresses_proton_has_enabled_and_lets_send() {
    let (user_key, _) = listed_key("user-key", "user@proton.me", "skp", 1, None);
    let user = User {
        id: "user".into(),
        keys: vec![user_key],
    };
    // Proton's `Status` and `Send` for each, in its order; one says neither.
    let listed = [
        ("disabled", serde_json::json!({ "Status": 0, "Send": 1 })),
        ("deleting", serde_json::json!({ "Status": 2, "Send": 1 })),
        ("receives", serde_json::json!({ "Status": 1, "Send": 0 })),
        ("sends", serde_json::json!({ "Status": 1, "Send": 1 })),
        ("unsaid", serde_json::json!({})),
    ];
    let addresses: Vec<Address> = listed
        .into_iter()
        .zip(1..)
        .map(|((id, mut fields), order)| {
            let email = format!("{id}@proton.me");
            fields["ID"] = id.into();
            fields["Email"] = email.clone().into();
            fields["Order"] = order.into();
            let mut address: Address = serde_json::from_value(fields).unwrap();
            address.keys = vec![listed_key(id, &email, "skp", 1, None).0];
            address
        })
        .collect();

    let provider = crate::crypto::provider();
    let store = unlock_with_skp(&provider, &user, &addresses, &SecretString::from("skp")).unwrap();

    let sending: Vec<&str> = store
        .addresses
        .iter()
        .filter(|keys| keys.can_send)
        .map(|keys| keys.address_id.as_str())
        .collect();
    assert_eq!(sending, ["sends", "unsaid"]);
    // New mail goes from the first that can send, and no other can be named.
    let found = |keys: Option<&AddressKeys>| keys.map(|keys| keys.address_id.clone());
    assert_eq!(found(store.primary_address()).as_deref(), Some("sends"));
    assert_eq!(found(store.address_for_email("disabled@proton.me")), None);
    assert_eq!(found(store.address_for_email("receives@proton.me")), None);
    // Every address's keys stay, to open the mail that reached it.
    assert_eq!(
        found(store.address("disabled")).as_deref(),
        Some("disabled")
    );
    assert_eq!(store.addresses.len(), 5);
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
        status: 1,
        send: 1,
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
    let (mut empty_signature, _) = listed_key("empty-signature", "me@proton.me", "skp", 0, None);
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
        status: 1,
        send: 1,
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
        status: 1,
        send: 1,
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
        let Ok((_, key_password)) = unlock(&provider, &user, &addresses, &password, &salts) else {
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
    let Ok((store, key_password)) = unlock(&provider, &user, &addresses, &password, &salts) else {
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
        status: 1,
        send: 1,
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
        status: 1,
        send: 1,
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
        status: 1,
        send: 1,
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
