//! OpenPGP for mail: opening and sealing message bodies and attachments,
//! and the session keys that let each recipient open them.

use super::keys::{AddressKeys, StoredKey};
use crate::error::{Error, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use proton_crypto::crypto::{
    DataEncoding, Decryptor, DecryptorSync, Encryptor, EncryptorSync, PGPMessage, PGPProviderSync,
    SessionKeyAlgorithm, Signer, SignerSync, VerificationError, VerifiedData, Verifier,
    VerifierSync,
};
use secrecy::ExposeSecret;
use zeroize::Zeroizing;

/// What checking the signature of a decrypted body found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// The signature checks out against one of the sender's keys.
    Verified,
    /// The body has no signature.
    Unsigned,
    /// No usable key of the sender's was given, so any signature went
    /// unchecked.
    Unverified,
    /// The body is signed, but the signature does not check out: the body
    /// was altered, or a key that is not the sender's made it.
    Invalid,
}

/// A session key taken out of the OpenPGP library with its algorithm, so
/// that what it encrypted can be opened later by each recipient in turn.
#[derive(Clone)]
pub struct SessionKeyMaterial {
    /// The key's bytes, wiped when dropped.
    pub key: Zeroizing<Vec<u8>>,
    /// The cipher the key is for.
    pub algo: SessionKeyAlgorithm,
}

/// An attachment encrypted for upload, with what opens it and vouches for
/// it.
pub struct UploadedAttachment {
    /// The session key, sealed to the sender's own address key.
    pub key_packet: Vec<u8>,
    /// The file, encrypted under the session key.
    pub data_packet: Vec<u8>,
    /// The sender's binary signature over the file as it was given.
    pub signature: Vec<u8>,
    /// The session key itself, to seal for each recipient when sending.
    pub material: SessionKeyMaterial,
}

/// The keys of an address that open, in the order the address keeps them:
/// the first is its primary key whenever that one opens (see
/// [`AddressKeys::keys`]). Never empty.
struct OpenedKeys<P: PGPProviderSync>(Vec<P::PrivateKey>);

impl<P: PGPProviderSync> OpenedKeys<P> {
    /// Opens what it can of `address`'s keys, passing over those that do not
    /// open. With none open, nothing can be signed, sealed to the address or
    /// opened with it.
    fn of(provider: &P, address: &AddressKeys) -> Result<Self> {
        let opened: Vec<_> = address
            .keys
            .iter()
            .filter_map(|key| open_key(provider, key))
            .collect();
        if opened.is_empty() {
            return Err(Error::Crypto("no usable private key for address".into()));
        }
        Ok(Self(opened))
    }

    /// Every one of them, to try on what was sealed to the address.
    fn all(&self) -> &[P::PrivateKey] {
        &self.0
    }

    /// The key that signs.
    fn signer(&self) -> &P::PrivateKey {
        &self.0[0]
    }

    /// The signer's public half, to which the sender's own copies are sealed.
    fn own_public_key(&self, provider: &P) -> Result<P::PublicKey> {
        provider
            .private_key_to_public_key(self.signer())
            .map_err(crypto_error("derive pubkey"))
    }

    /// The session key that `key_packets` seal to one of these keys.
    fn unseal(&self, provider: &P, key_packets: &[u8]) -> proton_crypto::Result<P::SessionKey> {
        provider
            .new_decryptor()
            .with_decryption_keys(self.all())
            .decrypt_session_key(key_packets)
    }
}

/// `key` opened from its unlocked copy, which skips deriving its passphrase,
/// or else from its armor and passphrase; `None` when it does not open.
fn open_key<P: PGPProviderSync>(provider: &P, key: &StoredKey) -> Option<P::PrivateKey> {
    let opened = match &key.unlocked {
        Some(unlocked) => {
            provider.private_key_import_unlocked(unlocked.as_slice(), DataEncoding::Bytes)
        }
        None => {
            let passphrase = key.passphrase.expose_secret().as_bytes();
            provider.private_key_import(key.armored.as_bytes(), passphrase, DataEncoding::Armor)
        }
    };
    opened.ok()
}

/// Turns a failure into a crypto error that names the `step` that failed.
fn crypto_error<E: std::fmt::Display>(step: &'static str) -> impl FnOnce(E) -> Error {
    move |error| Error::Crypto(format!("{step}: {error}"))
}

/// The armored public keys that import, skipping any that do not.
fn import_public_keys<P: PGPProviderSync>(provider: &P, armored: &[String]) -> Vec<P::PublicKey> {
    armored
        .iter()
        .filter_map(|key| {
            provider
                .public_key_import(key.as_bytes(), DataEncoding::Armor)
                .ok()
        })
        .collect()
}

/// A fresh AES-256 session key, and a copy of it that outlives this step.
fn fresh_session_key<P: PGPProviderSync>(
    provider: &P,
) -> Result<(P::SessionKey, SessionKeyMaterial)> {
    let session_key = provider
        .session_key_generate(SessionKeyAlgorithm::Aes256)
        .map_err(crypto_error("session key gen"))?;
    let material = export_session_key(provider, &session_key)?;
    Ok((session_key, material))
}

/// A session key copied out of the provider, so it outlives this step.
fn export_session_key<P: PGPProviderSync>(
    provider: &P,
    session_key: &P::SessionKey,
) -> Result<SessionKeyMaterial> {
    let (bytes, algo) = provider
        .session_key_export(session_key)
        .map_err(crypto_error("sk export"))?;
    Ok(SessionKeyMaterial {
        key: Zeroizing::new(bytes.as_ref().to_vec()),
        algo,
    })
}

/// A copied session key turned back into the provider's own.
fn import_session_key<P: PGPProviderSync>(
    provider: &P,
    material: &SessionKeyMaterial,
) -> Result<P::SessionKey> {
    provider
        .session_key_import(&material.key, material.algo)
        .map_err(crypto_error("sk import"))
}

/// `session_key` sealed in a key packet that `recipient`'s private key
/// opens. `step` names this in an error.
fn seal_to<P: PGPProviderSync>(
    provider: &P,
    recipient: &P::PublicKey,
    session_key: &P::SessionKey,
    step: &'static str,
) -> Result<Vec<u8>> {
    provider
        .new_encryptor()
        .with_encryption_key(recipient)
        .encrypt_session_key(session_key)
        .map_err(crypto_error(step))
}

/// The session key of an attachment, from its base64 key packets.
fn attachment_session_key<P: PGPProviderSync>(
    provider: &P,
    opened: &OpenedKeys<P>,
    key_packets_b64: &str,
) -> Result<P::SessionKey> {
    let key_packets = STANDARD
        .decode(key_packets_b64)
        .map_err(crypto_error("bad attachment key packet"))?;
    opened
        .unseal(provider, &key_packets)
        .map_err(crypto_error("attachment session key decrypt failed"))
}

/// An encrypted message in ASCII armor. `step` names armoring in an error.
fn armored_text<M: PGPMessage>(message: &M, step: &'static str) -> Result<String> {
    let armor = message.armor().map_err(crypto_error(step))?;
    String::from_utf8(armor).map_err(crypto_error("armor utf8"))
}

/// Opens an armored body with `addr`'s keys, and checks its signature with
/// `verify_pubs_armored`, the sender's armored public keys. Only failing to
/// decrypt is an error: what the signature check found is the [`Verdict`].
/// What is not valid UTF-8 in the body is replaced with U+FFFD.
pub fn decrypt_body<P: PGPProviderSync>(
    provider: &P,
    addr: &AddressKeys,
    verify_pubs_armored: &[String],
    armored_body: &str,
) -> Result<(String, Verdict)> {
    let (body, verdict) = decrypt_body_bytes(provider, addr, verify_pubs_armored, armored_body)?;
    Ok((utf8_text(body), verdict))
}

/// [`decrypt_body`] as the bytes that were encrypted, so the parts of a MIME
/// body keep their own charsets until it is parsed. They are wiped when
/// dropped.
pub(crate) fn decrypt_body_bytes<P: PGPProviderSync>(
    provider: &P,
    addr: &AddressKeys,
    verify_pubs_armored: &[String],
    armored_body: &str,
) -> Result<(Zeroizing<Vec<u8>>, Verdict)> {
    let opened = OpenedKeys::of(provider, addr)?;
    let senders_keys = import_public_keys(provider, verify_pubs_armored);
    tracing::debug!(target: "ruston_core::crypto", address = %addr.email, decrypt_keys = opened.all().len(), verify_keys = senders_keys.len(), armored_len = armored_body.len(), "decrypting message body");

    // A signature is looked for only when there are keys to check it with.
    let checks_signature = !senders_keys.is_empty();
    let mut decryptor = provider.new_decryptor().with_decryption_keys(opened.all());
    if checks_signature {
        decryptor = decryptor.with_verification_keys(&senders_keys);
    }
    let decrypted = decryptor
        .decrypt(armored_body, DataEncoding::Armor)
        .map_err(crypto_error("body decrypt failed"))?;
    let verdict = match checks_signature.then(|| decrypted.verification_result()) {
        None => Verdict::Unverified,
        Some(Ok(_)) => Verdict::Verified,
        Some(Err(VerificationError::NotSigned(_))) => Verdict::Unsigned,
        // A signature none of the sender's keys made may be anyone's, as a
        // forged one is: it fails like an altered one.
        Some(Err(_)) => Verdict::Invalid,
    };
    // Moved out rather than copied, so no second plaintext is freed unwiped.
    let body = Zeroizing::new(decrypted.into_vec());
    tracing::debug!(target: "ruston_core::crypto", verdict = ?verdict, plaintext_len = body.len(), "message body decrypted");
    Ok((body, verdict))
}

/// Decrypted `bytes` as text: moved into the string when they are UTF-8, or
/// else copied with U+FFFD for what is not, and wiped.
pub(crate) fn utf8_text(mut bytes: Zeroizing<Vec<u8>>) -> String {
    match String::from_utf8(std::mem::take(&mut *bytes)) {
        Ok(text) => text,
        Err(error) => {
            let bytes = Zeroizing::new(error.into_bytes());
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }
}

/// The bytes of an attachment: its base64 key packets give the session key
/// that opens its data packet. Its detached signature is not checked here:
/// [`verify_attachment_signature`] must vouch for the bytes before they are
/// released, as the `Client` download methods do.
pub fn decrypt_attachment<P: PGPProviderSync>(
    provider: &P,
    addr: &AddressKeys,
    key_packets_b64: &str,
    data_packet: &[u8],
) -> Result<Vec<u8>> {
    let opened = OpenedKeys::of(provider, addr)?;
    tracing::debug!(target: "ruston_core::crypto", address = %addr.email, data_packet = data_packet.len(), "decrypting attachment (unwrap session key → decrypt data packet)");
    let session_key = attachment_session_key(provider, &opened, key_packets_b64)?;
    let file = provider
        .new_decryptor()
        .with_session_key(session_key)
        .decrypt(data_packet, DataEncoding::Bytes)
        .map_err(crypto_error("attachment data decrypt failed"))?;
    // Moved out rather than copied, so no second plaintext is freed unwiped.
    Ok(file.into_vec())
}

/// Verify a detached armored attachment signature against the sender's keys.
/// Missing usable keys, malformed signatures and mismatches all fail closed.
pub fn verify_attachment_signature<P: PGPProviderSync>(
    provider: &P,
    verify_pubs_armored: &[String],
    plaintext: &[u8],
    signature: &str,
) -> Result<()> {
    let pubs = import_public_keys(provider, verify_pubs_armored);
    if pubs.is_empty() {
        return Err(Error::AttachmentVerificationFailed);
    }
    provider
        .new_verifier()
        .with_verification_keys(pubs.iter())
        .verify_detached(plaintext, signature.as_bytes(), DataEncoding::Armor)
        .map_err(|_| Error::AttachmentVerificationFailed)?;
    Ok(())
}

/// A draft body, signed by the sender and sealed to the sender's own key,
/// in ASCII armor.
pub fn encrypt_self_draft<P: PGPProviderSync>(
    provider: &P,
    addr: &AddressKeys,
    body: &str,
) -> Result<String> {
    let opened = OpenedKeys::of(provider, addr)?;
    tracing::debug!(target: "ruston_core::crypto", address = %addr.email, body_len = body.len(), "encrypting + signing draft body to self");
    let own_key = opened.own_public_key(provider)?;
    let draft = provider
        .new_encryptor()
        .with_encryption_key(&own_key)
        .with_signing_key(opened.signer())
        .with_utf8()
        .encrypt(body)
        .map_err(crypto_error("draft encrypt failed"))?;
    armored_text(&draft, "armor failed")
}

/// A body signed by the sender and encrypted for sending under a fresh
/// session key, as a bare data packet. The session key comes back with it,
/// to be sealed for each recipient.
pub fn encrypt_for_transport<P: PGPProviderSync>(
    provider: &P,
    addr: &AddressKeys,
    body: &str,
) -> Result<(SessionKeyMaterial, Vec<u8>)> {
    let opened = OpenedKeys::of(provider, addr)?;
    tracing::debug!(target: "ruston_core::crypto", address = %addr.email, body_len = body.len(), "encrypting transport body (fresh AES-256 session key, signed, no key packet)");
    let (session_key, material) = fresh_session_key(provider)?;
    let sealed = provider
        .new_encryptor()
        .with_session_key(session_key)
        .with_signing_key(opened.signer())
        .with_utf8()
        .encrypt_raw(body, DataEncoding::Bytes)
        .map_err(crypto_error("transport body encrypt failed"))?;
    Ok((material, sealed))
}

/// An attachment encrypted for upload under a fresh session key, with that
/// key sealed to the sender's own and the sender's detached signature over
/// the file.
pub fn encrypt_attachment<P: PGPProviderSync>(
    provider: &P,
    addr: &AddressKeys,
    bytes: &[u8],
) -> Result<UploadedAttachment> {
    let opened = OpenedKeys::of(provider, addr)?;
    tracing::debug!(target: "ruston_core::crypto", address = %addr.email, bytes = bytes.len(), "encrypting attachment (session key + data packet + detached signature)");
    let own_key = opened.own_public_key(provider)?;
    let (session_key, material) = fresh_session_key(provider)?;
    let data_packet = provider
        .new_encryptor()
        .with_session_key(session_key)
        .encrypt_raw(bytes, DataEncoding::Bytes)
        .map_err(crypto_error("attachment encrypt failed"))?;
    let signature = provider
        .new_signer()
        .with_signing_key(opened.signer())
        .sign_detached(bytes, DataEncoding::Bytes)
        .map_err(crypto_error("attachment sign failed"))?;
    // The encryptor took the session key; the copy brings it back.
    let session_key = import_session_key(provider, &material)?;
    let key_packet = seal_to(provider, &own_key, &session_key, "wrap attachment sk")?;
    Ok(UploadedAttachment {
        key_packet,
        data_packet,
        signature,
        material,
    })
}

/// `mat`'s session key sealed to a recipient's armored public key: the key
/// packet that lets them open what it encrypts.
pub fn wrap_session_key<P: PGPProviderSync>(
    provider: &P,
    recipient_pub_armored: &str,
    mat: &SessionKeyMaterial,
) -> Result<Vec<u8>> {
    tracing::trace!(target: "ruston_core::crypto", "wrapping session key to a recipient public key");
    let session_key = import_session_key(provider, mat)?;
    let recipient = provider
        .public_key_import(recipient_pub_armored, DataEncoding::Armor)
        .map_err(crypto_error("recipient pubkey import"))?;
    seal_to(provider, &recipient, &session_key, "wrap session key")
}

/// `mat`'s session key sealed to the sender's own address key: the key
/// packet of the sender's copy of a body.
pub fn wrap_session_key_to_self<P: PGPProviderSync>(
    provider: &P,
    addr: &AddressKeys,
    mat: &SessionKeyMaterial,
) -> Result<Vec<u8>> {
    let own_key = OpenedKeys::of(provider, addr)?.own_public_key(provider)?;
    let session_key = import_session_key(provider, mat)?;
    seal_to(provider, &own_key, &session_key, "wrap session key to self")
}

/// An attachment's session key, opened with `src`'s keys from its base64
/// key packets and sealed again to `dst`'s own key, so that a message from
/// `dst` can forward the file.
pub fn rewrap_attachment_session_key<P: PGPProviderSync>(
    provider: &P,
    src: &AddressKeys,
    dst: &AddressKeys,
    key_packets_b64: &str,
) -> Result<Vec<u8>> {
    let from = OpenedKeys::of(provider, src)?;
    let to = OpenedKeys::of(provider, dst)?.own_public_key(provider)?;
    let key_packets = STANDARD
        .decode(key_packets_b64)
        .map_err(crypto_error("bad key packet"))?;
    let session_key = from
        .unseal(provider, &key_packets)
        .map_err(crypto_error("decrypt att session key"))?;
    seal_to(provider, &to, &session_key, "rewrap att session key")
}

/// The session key an attachment's key packet seals to `addr`, so it can be
/// wrapped for recipients without encrypting the attachment again.
pub fn decrypt_session_key<P: PGPProviderSync>(
    provider: &P,
    addr: &AddressKeys,
    key_packets_b64: &str,
) -> Result<SessionKeyMaterial> {
    let opened = OpenedKeys::of(provider, addr)?;
    let session_key = attachment_session_key(provider, &opened, key_packets_b64)?;
    let material = export_session_key(provider, &session_key)?;
    // Packages name the algorithm, and an unknown one cannot be named.
    if matches!(material.algo, SessionKeyAlgorithm::Unknown) {
        return Err(Error::Crypto(
            "attachment session key uses an unsupported algorithm".into(),
        ));
    }
    Ok(material)
}

/// A fresh AES-256 session key, and its bytes in base64 to serve as a
/// token.
pub fn new_session_key<P: PGPProviderSync>(provider: &P) -> Result<(SessionKeyMaterial, String)> {
    let (_, material) = fresh_session_key(provider)?;
    let token = STANDARD.encode(material.key.as_slice());
    Ok((material, token))
}

/// `text` sealed with `password` alone, in ASCII armor: the `EncToken` of a
/// message to a recipient outside Proton.
pub fn encrypt_text_with_password<P: PGPProviderSync>(
    provider: &P,
    password: &str,
    text: &str,
) -> Result<String> {
    let token = provider
        .new_encryptor()
        .with_passphrase(password)
        .with_utf8()
        .encrypt(text)
        .map_err(crypto_error("password encrypt"))?;
    armored_text(&token, "armor")
}

/// `mat`'s session key sealed with `password` alone: the `BodyKeyPacket` of
/// a message to a recipient outside Proton.
pub fn wrap_session_key_with_password<P: PGPProviderSync>(
    provider: &P,
    password: &str,
    mat: &SessionKeyMaterial,
) -> Result<Vec<u8>> {
    let session_key = import_session_key(provider, mat)?;
    provider
        .new_encryptor()
        .with_passphrase(password)
        .encrypt_session_key(&session_key)
        .map_err(crypto_error("password-wrap session key"))
}

/// How a send request names the cipher `algo` of a session key it hands
/// Proton in the clear.
pub fn algo_name(algo: SessionKeyAlgorithm) -> &'static str {
    match algo {
        SessionKeyAlgorithm::Aes256 => "aes256",
        SessionKeyAlgorithm::Aes128 => "aes128",
        // No package of this crate names one: fresh keys are AES-256, and
        // `decrypt_session_key` refuses an unknown algorithm.
        SessionKeyAlgorithm::Unknown => "aes256",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::keys::StoredKey;
    use proton_crypto::crypto::{
        ArmorerSync, KeyGenerator, KeyGeneratorAlgorithm, KeyGeneratorSync, SessionKey,
    };
    use secrecy::SecretString;

    /// A fresh key for `email` as its owner holds it, and its armored public
    /// half as correspondents know it.
    fn address(email: &str) -> (AddressKeys, String) {
        let provider = crate::crypto::provider();
        let generated = provider
            .new_key_generator()
            .with_user_id("Test", email)
            .with_algorithm(KeyGeneratorAlgorithm::ECC)
            .generate()
            .unwrap();
        let text = |bytes: &[u8]| String::from_utf8(bytes.to_vec()).unwrap();
        let locked = provider.private_key_export(&generated, "key passphrase", DataEncoding::Armor);
        let public = provider.private_key_to_public_key(&generated).unwrap();
        let public = provider.public_key_export(&public, DataEncoding::Armor);
        let owned = StoredKey {
            armored: text(locked.unwrap().as_ref()),
            passphrase: SecretString::from("key passphrase"),
            unlocked: None,
        };
        let keys = AddressKeys {
            address_id: email.into(),
            email: email.into(),
            keys: vec![owned],
        };
        (keys, text(public.unwrap().as_ref()))
    }

    /// Fails the test unless `outcome` is a crypto error whose text starts
    /// with `step`.
    #[track_caller]
    fn assert_fails_at<T>(outcome: Result<T>, step: &str) {
        match outcome {
            Err(Error::Crypto(text)) => assert!(text.starts_with(step), "{text:?} for {step:?}"),
            Err(other) => panic!("not a crypto error: {other}"),
            Ok(_) => panic!("{step:?} did not fail"),
        }
    }

    #[test]
    fn a_body_reads_back_with_what_its_signature_says() {
        let provider = crate::crypto::provider();
        let (mut me, my_key) = address("me@proton.me");
        let (_, strangers_key) = address("stranger@proton.me");
        // A key that no longer opens is passed over, and the next one signs.
        let (mut stale, _) = address("me@proton.me");
        stale.keys[0].passphrase = SecretString::from("changed since");
        me.keys.insert(0, stale.keys.remove(0));
        let signed = encrypt_self_draft(&provider, &me, "hello body").unwrap();
        let recipient = provider
            .public_key_import(&my_key, DataEncoding::Armor)
            .unwrap();
        let unsigned = provider
            .new_encryptor()
            .with_encryption_key(&recipient)
            .encrypt("hello body")
            .and_then(|message| message.armor())
            .unwrap();
        let unsigned = String::from_utf8(unsigned).unwrap();

        for (body, senders_keys, verdict) in [
            (&signed, vec![my_key.clone()], Verdict::Verified),
            // A key the sender lacks made it: as good as forged.
            (&signed, vec![strangers_key], Verdict::Invalid),
            (&signed, Vec::new(), Verdict::Unverified),
            (&unsigned, vec![my_key], Verdict::Unsigned),
        ] {
            let read = decrypt_body(&provider, &me, &senders_keys, body).unwrap();
            assert_eq!(read, ("hello body".to_owned(), verdict));
        }
    }

    #[test]
    fn decrypted_text_keeps_utf8_and_marks_what_is_not() {
        assert_eq!(
            utf8_text(Zeroizing::new("Café".as_bytes().to_vec())),
            "Café"
        );
        assert_eq!(
            utf8_text(Zeroizing::new(b"Caf\xe9".to_vec())),
            "Caf\u{FFFD}"
        );
    }

    #[test]
    fn detached_attachment_signatures_require_matching_content_and_sender_keys() {
        let provider = proton_crypto::new_pgp_provider();
        let (sender, sender_pub) = address("sender@proton.me");
        let (_, wrong_pub) = address("other@proton.me");
        let payload = b"signed attachment payload";
        let upload = encrypt_attachment(&provider, &sender, payload).unwrap();
        let signature = String::from_utf8(
            provider
                .armorer()
                .armor_signature(&upload.signature)
                .unwrap(),
        )
        .unwrap();
        // Old or malformed keys in the ring must not hide a usable signer key.
        let keys = vec!["not a key".into(), wrong_pub.clone(), sender_pub.clone()];
        verify_attachment_signature(&provider, &keys, payload, &signature).unwrap();
        for (keys, data, signature) in [
            (
                vec![sender_pub.clone()],
                b"changed payload".as_slice(),
                signature.as_str(),
            ),
            (vec![wrong_pub], payload.as_slice(), signature.as_str()),
            (Vec::new(), payload.as_slice(), signature.as_str()),
            (
                vec!["not a key".into()],
                payload.as_slice(),
                signature.as_str(),
            ),
            (vec![sender_pub.clone()], payload.as_slice(), "malformed"),
            (vec![sender_pub], payload.as_slice(), ""),
        ] {
            assert!(matches!(
                verify_attachment_signature(&provider, &keys, data, signature),
                Err(Error::AttachmentVerificationFailed)
            ));
        }
    }

    #[test]
    fn a_transport_body_is_signed_and_an_attachments_data_is_not() {
        let provider = crate::crypto::provider();
        let (me, my_key) = address("me@proton.me");
        let (material, body) = encrypt_for_transport(&provider, &me, "transport body").unwrap();
        let file = encrypt_attachment(&provider, &me, b"file").unwrap();

        // The body's key packet comes apart from it, and opens it like an
        // attachment's.
        let key_packet = wrap_session_key_to_self(&provider, &me, &material).unwrap();
        let opened = decrypt_attachment(&provider, &me, &STANDARD.encode(key_packet), &body);
        assert_eq!(opened.unwrap(), b"transport body");

        let signer = provider
            .public_key_import(&my_key, DataEncoding::Armor)
            .unwrap();
        let signature_check = |material: &SessionKeyMaterial, packet: &[u8]| {
            let session_key = provider.session_key_import(&material.key, material.algo);
            provider
                .new_decryptor()
                .with_session_key(session_key.unwrap())
                .with_verification_key(&signer)
                .decrypt(packet, DataEncoding::Bytes)
                .unwrap()
                .verification_result()
        };
        assert!(signature_check(&material, &body).is_ok());
        // An attachment's signature travels apart from it.
        assert!(matches!(
            signature_check(&file.material, &file.data_packet),
            Err(VerificationError::NotSigned(_))
        ));
    }

    #[test]
    fn text_is_sealed_with_canonical_line_endings_and_files_as_they_are() {
        let provider = crate::crypto::provider();
        let (me, _) = address("me@proton.me");
        let (text, canonical) = ("one\ntwo", "one\r\ntwo");

        let draft = encrypt_self_draft(&provider, &me, text).unwrap();
        assert_eq!(
            decrypt_body(&provider, &me, &[], &draft).unwrap().0,
            canonical
        );
        let (material, body) = encrypt_for_transport(&provider, &me, text).unwrap();
        let key_packet = wrap_session_key_to_self(&provider, &me, &material).unwrap();
        let opened = decrypt_attachment(&provider, &me, &STANDARD.encode(key_packet), &body);
        assert_eq!(opened.unwrap(), canonical.as_bytes());
        let token = encrypt_text_with_password(&provider, "password", text).unwrap();
        let opened = provider
            .new_decryptor()
            .with_passphrase("password")
            .decrypt(token, DataEncoding::Armor)
            .unwrap();
        assert_eq!(opened.as_bytes(), canonical.as_bytes());

        let file = encrypt_attachment(&provider, &me, text.as_bytes()).unwrap();
        let key_packet = STANDARD.encode(&file.key_packet);
        let opened = decrypt_attachment(&provider, &me, &key_packet, &file.data_packet);
        assert_eq!(opened.unwrap(), text.as_bytes());
    }

    #[test]
    fn nothing_is_compressed_and_only_drafts_and_tokens_are_armored() {
        let provider = crate::crypto::provider();
        let (me, _) = address("me@proton.me");
        // Compressed, this would shrink to a small fraction of its size.
        let repeated = "a".repeat(64 * 1024);

        let draft = encrypt_self_draft(&provider, &me, &repeated).unwrap();
        let token = encrypt_text_with_password(&provider, "password", &repeated).unwrap();
        for armored in [draft, token] {
            assert!(armored.starts_with("-----BEGIN PGP MESSAGE-----"));
            assert!(armored.len() > repeated.len());
        }
        let (_, body) = encrypt_for_transport(&provider, &me, &repeated).unwrap();
        let file = encrypt_attachment(&provider, &me, repeated.as_bytes()).unwrap();
        for packet in [body, file.data_packet] {
            assert!(!packet.starts_with(b"-----BEGIN"));
            assert!(packet.len() > repeated.len());
        }
    }

    #[test]
    fn fresh_session_keys_are_aes_256() {
        let provider = crate::crypto::provider();
        let (me, _) = address("me@proton.me");
        let (fresh, token) = new_session_key(&provider).unwrap();
        let (for_body, _) = encrypt_for_transport(&provider, &me, "body").unwrap();
        let for_file = encrypt_attachment(&provider, &me, b"file")
            .unwrap()
            .material;
        for material in [&fresh, &for_body, &for_file] {
            assert!(matches!(material.algo, SessionKeyAlgorithm::Aes256));
            assert_eq!(material.key.len(), 32);
        }
        // The token is the key itself, in base64.
        assert_eq!(STANDARD.decode(token).unwrap(), fresh.key.as_slice());
        for (algorithm, name) in [
            (SessionKeyAlgorithm::Aes256, "aes256"),
            (SessionKeyAlgorithm::Aes128, "aes128"),
            (SessionKeyAlgorithm::Unknown, "aes256"),
        ] {
            assert_eq!(algo_name(algorithm), name);
        }
    }

    #[test]
    fn password_protected_text_and_key_packets_open_with_the_password() {
        let provider = crate::crypto::provider();
        let password = "s3cret-eo";
        let opener = || provider.new_decryptor().with_passphrase(password);

        // The `EncToken`: text sealed with the password alone.
        let token = encrypt_text_with_password(&provider, password, "hello eo").unwrap();
        let opened = opener().decrypt(token, DataEncoding::Armor).unwrap();
        assert_eq!(opened.as_bytes(), b"hello eo");
        // The `BodyKeyPacket`: a session key sealed with it.
        let (material, _) = new_session_key(&provider).unwrap();
        let key_packet = wrap_session_key_with_password(&provider, password, &material).unwrap();
        let session_key = opener().decrypt_session_key(key_packet).unwrap();
        assert_eq!(session_key.export().as_ref(), material.key.as_slice());
    }

    #[test]
    fn an_attachment_key_reaches_recipients_and_other_addresses() {
        let provider = crate::crypto::provider();
        let (me, _) = address("me@proton.me");
        let (alias, _) = address("alias@proton.me");
        let (friend, friends_key) = address("friend@proton.me");
        let file = encrypt_attachment(&provider, &me, b"forwarded file").unwrap();
        let key_packets = STANDARD.encode(&file.key_packet);

        let recovered = decrypt_session_key(&provider, &me, &key_packets).unwrap();
        assert_eq!(recovered.key.as_slice(), file.material.key.as_slice());
        assert!(matches!(recovered.algo, SessionKeyAlgorithm::Aes256));
        let for_friend = wrap_session_key(&provider, &friends_key, &recovered).unwrap();
        let for_alias = rewrap_attachment_session_key(&provider, &me, &alias, &key_packets);
        for (reader, key_packet) in [(&friend, for_friend), (&alias, for_alias.unwrap())] {
            let key_packet = STANDARD.encode(key_packet);
            let opened = decrypt_attachment(&provider, reader, &key_packet, &file.data_packet);
            assert_eq!(opened.unwrap(), b"forwarded file");
        }
    }

    #[test]
    fn failures_name_the_step_that_failed() {
        let provider = crate::crypto::provider();
        let (me, _) = address("me@proton.me");
        let (stranger, _) = address("stranger@proton.me");
        let mut locked = me.clone();
        locked.keys[0].passphrase = SecretString::from("not its passphrase");
        let file = encrypt_attachment(&provider, &me, b"file").unwrap();
        let packets = STANDARD.encode(&file.key_packet);
        let data = file.data_packet.as_slice();

        let no_key = "no usable private key for address";
        assert_fails_at(encrypt_self_draft(&provider, &locked, "body"), no_key);
        let body = decrypt_body(&provider, &me, &[], "not a message");
        assert_fails_at(body, "body decrypt failed: ");
        let undecodable = "bad attachment key packet: ";
        assert_fails_at(decrypt_attachment(&provider, &me, "!", data), undecodable);
        assert_fails_at(decrypt_session_key(&provider, &me, "!"), undecodable);
        let unopened = "attachment session key decrypt failed: ";
        assert_fails_at(
            decrypt_attachment(&provider, &stranger, &packets, data),
            unopened,
        );
        let damaged = "attachment data decrypt failed: ";
        assert_fails_at(decrypt_attachment(&provider, &me, &packets, b"x"), damaged);
        let forward = rewrap_attachment_session_key(&provider, &me, &stranger, "!");
        assert_fails_at(forward, "bad key packet: ");
        let forward = rewrap_attachment_session_key(&provider, &stranger, &me, &packets);
        assert_fails_at(forward, "decrypt att session key: ");
        let to_nobody = wrap_session_key(&provider, "not a key", &file.material);
        assert_fails_at(to_nobody, "recipient pubkey import: ");
    }
}
