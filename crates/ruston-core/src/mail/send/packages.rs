//! The encrypted copies of a message, one package for each kind of
//! recipient, and the request that sends them.

use super::SendOptions;
use crate::crypto::SessionKeyMaterial;
use crate::crypto::keys::AddressKeys;
use crate::error::{Error, Result};
use crate::model::MimeType;
use crate::model::enums::package_type;
use crate::transport::Doer;
use crate::{api, crypto};
use base64::{Engine, engine::general_purpose::STANDARD};
use proton_srp::{SRPAuth, SRPVerifierB64};
use serde_json::{Map, Value, json};

/// An attachment of the draft and the session key that opens it, whether it
/// was uploaded for this message or carried over from the one being
/// forwarded. Each package wraps the key for its recipients.
#[derive(Clone)]
pub(in crate::mail) struct AttachmentKey {
    pub(super) id: String,
    pub(super) session_key: SessionKeyMaterial,
    /// Whether a detached signature of its bytes went up with it.
    pub(super) signed: bool,
}

/// The `Signature` value of a package that goes in the clear: always 0,
/// unlike an encrypted one's, which [`attachment_signatures`] gives.
const CLEAR_SIGNATURE: u8 = 0;

/// Whether Proton adds the recipients to the contacts as it sends: 0, no.
pub(super) const SAVE_RECIPIENTS_AS_CONTACTS: u8 = 0;

/// The `Signature` value of an encrypted package: 1 when every attachment
/// the message carries is signed, which has the recipient's app check those
/// signatures, and 0 otherwise. Proton's own apps set it the same way.
pub(super) fn attachment_signatures(attachments: &[AttachmentKey]) -> u8 {
    u8::from(attachments.iter().all(|attachment| attachment.signed))
}

/// A body encrypted for transport: the data packet a package carries, and
/// the session key that opens it, which is wrapped for each recipient.
pub(super) struct EncryptedBody {
    pub(super) session_key: SessionKeyMaterial,
    pub(super) data_packet: Vec<u8>,
}

impl EncryptedBody {
    pub(super) fn encrypt<P: proton_crypto::crypto::PGPProviderSync>(
        provider: &P,
        addr: &AddressKeys,
        text: &str,
    ) -> Result<Self> {
        let (session_key, data_packet) = crypto::encrypt_for_transport(provider, addr, text)?;
        Ok(Self {
            session_key,
            data_packet,
        })
    }
}

/// How one recipient's copy goes out, which names its package.
pub(super) enum Delivery<'a> {
    /// End-to-end encrypted to a key Proton vouches for (Type 1).
    EndToEnd(&'a str),
    /// As PGP/Inline text, encrypted to a published key (Type 8).
    PgpInline(&'a str),
    /// Behind the message password, with the entry that opens it (Type 2).
    Password(Value),
    /// Without end-to-end encryption (Type 4).
    Clear,
}

/// The send request's packages, one for each way the copies go out, in that
/// order, each listing its recipients with the body's and the attachments'
/// keys wrapped for them. Password entries arrive built, so this asks the
/// network for nothing.
pub(super) fn build_packages<P: proton_crypto::crypto::PGPProviderSync>(
    provider: &P,
    addr: &AddressKeys,
    opts: &SendOptions,
    body: &EncryptedBody,
    attachments: &[AttachmentKey],
    deliveries: Vec<(&str, Delivery<'_>)>,
) -> Result<Vec<Value>> {
    let mime_type = opts.body_mime_type();
    // Each package's recipients, by address, with what opens their copy.
    let mut end_to_end = Map::new();
    let mut pgp_inline = Map::new();
    let mut by_password = Map::new();
    let mut in_clear = Map::new();
    // PGP/Inline carries plain text only, so an HTML body reaches those
    // recipients as text, under a session key of its own.
    let mut plain_text_body: Option<EncryptedBody> = None;

    // A recipient's entry in an encrypted package (Type 1 or 8): `body_key`
    // and each attachment's session key, sealed to `pubkey`.
    let entry_for = |pubkey: &str, pkg_type: i64, body_key: &SessionKeyMaterial| -> Result<Value> {
        let bkp = crypto::wrap_session_key(provider, pubkey, body_key)?;
        let mut entry = json!({
            "Type": pkg_type,
            "BodyKeyPacket": STANDARD.encode(&bkp),
            "Signature": attachment_signatures(attachments),
        });
        attachment_keys_field(
            &mut entry,
            "AttachmentKeyPackets",
            attachments,
            |attachment| {
                let packet = crypto::wrap_session_key(provider, pubkey, &attachment.session_key)?;
                Ok(json!(STANDARD.encode(packet)))
            },
        )?;
        Ok(entry)
    };

    for (email, delivery) in deliveries {
        match delivery {
            Delivery::EndToEnd(key) => {
                end_to_end.insert(
                    email.to_owned(),
                    entry_for(key, package_type::PM, &body.session_key)?,
                );
            }
            Delivery::PgpInline(key) => {
                if opts.html && plain_text_body.is_none() {
                    let text = crate::html::to_text(&opts.body);
                    plain_text_body = Some(EncryptedBody::encrypt(provider, addr, &text)?);
                }
                let text_body = plain_text_body.as_ref().unwrap_or(body);
                pgp_inline.insert(
                    email.to_owned(),
                    entry_for(key, package_type::PGP_INLINE, &text_body.session_key)?,
                );
            }
            Delivery::Password(entry) => {
                by_password.insert(email.to_owned(), entry);
            }
            Delivery::Clear => {
                in_clear.insert(
                    email.to_owned(),
                    json!({ "Type": package_type::CLEAR, "Signature": CLEAR_SIGNATURE }),
                );
            }
        }
    }

    let mut packages: Vec<Value> = Vec::new();
    if !end_to_end.is_empty() {
        let body_kp = crypto::wrap_session_key_to_self(provider, addr, &body.session_key)?;
        packages.push(json!({
            "Addresses": Value::Object(end_to_end),
            "MIMEType": mime_type,
            "Type": package_type::PM,
            "Body": STANDARD.encode(&body.data_packet),
            "BodyKeyPacket": STANDARD.encode(&body_kp),
        }));
    }
    if !pgp_inline.is_empty() {
        // PGP-inline is plain text: the text version of an HTML body, or
        // the plain body itself.
        let text_body = plain_text_body.as_ref().unwrap_or(body);
        let body_kp = crypto::wrap_session_key_to_self(provider, addr, &text_body.session_key)?;
        packages.push(json!({
            "Addresses": Value::Object(pgp_inline),
            "MIMEType": MimeType::PlainText.as_str(),
            "Type": package_type::PGP_INLINE,
            "Body": STANDARD.encode(&text_body.data_packet),
            "BodyKeyPacket": STANDARD.encode(&body_kp),
        }));
    }
    if !by_password.is_empty() {
        packages.push(json!({
            "Addresses": Value::Object(by_password),
            "MIMEType": mime_type,
            "Type": package_type::EO,
            "Body": STANDARD.encode(&body.data_packet),
        }));
    }
    if !in_clear.is_empty() {
        let mut pkg = json!({
            "Addresses": Value::Object(in_clear),
            "MIMEType": mime_type,
            "Type": package_type::CLEAR,
            "Body": STANDARD.encode(&body.data_packet),
            "BodyKey": {
                "Key": STANDARD.encode(&body.session_key.key),
                "Algorithm": crypto::algo_name(body.session_key.algo),
            },
        });
        attachment_keys_field(&mut pkg, "AttachmentKeys", attachments, |attachment| {
            let key = &attachment.session_key;
            Ok(
                json!({ "Key": STANDARD.encode(&key.key), "Algorithm": crypto::algo_name(key.algo) }),
            )
        })?;
        packages.push(pkg);
    }
    Ok(packages)
}

/// One password-protected (encrypted-outside) recipient's entry: an SRP
/// verifier, a token, and the body and attachment keys wrapped with the
/// password. The verifier and every password-wrapped key take a costly key
/// derivation, so callers run this on the blocking pool; the provider is
/// created there.
pub(super) fn password_entry(
    password: &str,
    hint: &str,
    modulus: &api::keys::SrpModulus,
    body_key: &SessionKeyMaterial,
    attachments: &[AttachmentKey],
) -> Result<Value> {
    let provider = crypto::provider();
    let verifier: SRPVerifierB64 =
        SRPAuth::generate_verifier_with_pgp(password, None, &modulus.modulus)
            .map_err(|e| Error::Srp(format!("EO verifier: {e}")))?
            .into();
    let (_fresh, token) = crypto::new_session_key(&provider)?;
    let enc_token = crypto::encrypt_text_with_password(&provider, password, &token)?;
    let body_key_packet = crypto::wrap_session_key_with_password(&provider, password, body_key)?;
    let mut entry = json!({
        "Type": package_type::EO,
        "Signature": attachment_signatures(attachments),
        "Token": token,
        "EncToken": enc_token,
        "BodyKeyPacket": STANDARD.encode(&body_key_packet),
        "Auth": {
            "Version": u8::from(verifier.version),
            "ModulusID": modulus.id,
            "Salt": verifier.salt,
            "Verifier": verifier.verifier,
        },
    });
    if !hint.is_empty() {
        entry["PasswordHint"] = json!(hint);
    }
    attachment_keys_field(
        &mut entry,
        "AttachmentKeyPackets",
        attachments,
        |attachment| {
            let packet = crypto::wrap_session_key_with_password(
                &provider,
                password,
                &attachment.session_key,
            )?;
            Ok(json!(STANDARD.encode(packet)))
        },
    )?;
    Ok(entry)
}

/// Puts under `field` of `entry` each attachment's key, by the attachment's
/// ID, as `key` gives it: wrapped to a recipient, wrapped with a password,
/// or in the clear. A message without attachments gets no such field.
fn attachment_keys_field(
    entry: &mut Value,
    field: &str,
    attachments: &[AttachmentKey],
    mut key: impl FnMut(&AttachmentKey) -> Result<Value>,
) -> Result<()> {
    if attachments.is_empty() {
        return Ok(());
    }
    let mut keys = Map::new();
    for attachment in attachments {
        keys.insert(attachment.id.clone(), key(attachment)?);
    }
    entry[field] = Value::Object(keys);
    Ok(())
}

pub(super) async fn submit_send<D: Doer>(http: &D, message_id: &str, request: Value) -> Result<()> {
    api::messages::send_message(http, message_id, request)
        .await
        .map_err(|error| {
            // A lost or unusable response does not prove that Proton rejected
            // the POST. Keep the draft ID so callers can check Sent first.
            let uncertain = matches!(
                &error,
                Error::Http(_) | Error::Json(_) | Error::ResponseTooLarge { .. }
            ) || matches!(&error, Error::Api(api) if api.is_server_error());
            if uncertain {
                Error::SendUnconfirmed {
                    message_id: message_id.to_owned(),
                    source: Box::new(error),
                }
            } else {
                error
            }
        })
}

/// Deletes the draft of a send that failed, unless the failure leaves open
/// whether Proton sent it: then the draft stays, so callers can check Sent.
pub(super) async fn cleanup_failed_draft<D: Doer>(http: &D, message_id: &str, error: &Error) {
    if matches!(error, Error::SendUnconfirmed { .. }) {
        tracing::warn!(target: "ruston_core::send", message_id = %message_id, error = %error, "send outcome unconfirmed — skipping draft cleanup");
    } else {
        tracing::warn!(target: "ruston_core::send", message_id = %message_id, error = %error, "send failed — deleting draft");
        let _ = api::messages::delete(http, &[message_id.to_owned()]).await;
    }
}

/// The message a reply or a forward answers, which its draft names.
/// What opens the copies for recipients with no key of their own: a
/// password the sender gives them some other way, and a hint to recall it.
#[derive(Clone, Copy)]
pub(super) struct EncryptedOutside<'a> {
    pub(super) password: &'a str,
    pub(super) hint: &'a str,
}
