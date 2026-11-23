//! The recipients' keys, looked up a few at a time, and the protection
//! each copy of a message gets from them.

use crate::api;
use crate::error::{Error, Result};
use crate::transport::Doer;
use futures::{StreamExt, TryStreamExt, stream};
use serde_json::{Value, json};

/// How many recipients' keys a send asks Proton for at once.
const RECIPIENT_KEY_CONCURRENCY: usize = 4;

/// How a message to one recipient is protected when it carries no message
/// password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipientProtection {
    /// A Proton address: end-to-end encrypted to a key Proton vouches for.
    EndToEnd,
    /// Another provider's address with a published PGP key: encrypted to it.
    Pgp,
    /// No usable key, or the address's owner turned email encryption off:
    /// Proton delivers the message without end-to-end encryption.
    Unencrypted,
}

impl RecipientProtection {
    /// Reads a key lookup the way sending picks a recipient's package.
    pub(super) fn of(keys: &api::keys::RecipientKeys) -> Self {
        match RecipientKey::pick(keys) {
            RecipientKey::Verified(_) => Self::EndToEnd,
            RecipientKey::Published(_) => Self::Pgp,
            RecipientKey::Missing => Self::Unencrypted,
        }
    }
}

/// The armored key a recipient's copy is encrypted to, which decides the
/// package it goes in. A key that may not encrypt mail, being obsolete or on
/// an address whose owner turned email encryption off, counts as none.
pub(super) enum RecipientKey<'a> {
    /// A key Proton vouches for, so a Proton address.
    Verified(&'a str),
    /// A key published outside Proton, as through WKD.
    Published(&'a str),
    /// No key to encrypt to.
    Missing,
}

impl<'a> RecipientKey<'a> {
    pub(super) fn pick(keys: &'a api::keys::RecipientKeys) -> Self {
        if let Some(key) = api::keys::pick_encryption_key(&keys.keys) {
            Self::Verified(&key.public_key)
        } else if let Some(key) = api::keys::pick_encryption_key(&keys.unverified) {
            Self::Published(&key.public_key)
        } else {
            Self::Missing
        }
    }
}

/// Every address in `lists`, in order, each once: one that differs from an
/// earlier one only in the case of its ASCII letters is left out.
pub(super) fn dedupe(lists: &[&[String]]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    lists
        .iter()
        .flat_map(|list| list.iter())
        .filter(|address| seen.insert(address.to_ascii_lowercase()))
        .cloned()
        .collect()
}

/// Each recipient's keys, in the recipients' order, asked for a few at a
/// time.
pub(super) async fn look_up_recipients<D: Doer>(
    http: &D,
    recipients: Vec<String>,
) -> Result<Vec<(String, api::keys::RecipientKeys)>> {
    stream::iter(recipients)
        .map(|email| async move {
            let keys = api::keys::get_all_public_keys(http, &email).await?;
            Ok::<_, Error>((email, keys))
        })
        .buffered(RECIPIENT_KEY_CONCURRENCY)
        .try_collect()
        .await
}

pub(crate) fn recipient_list(addrs: &[String]) -> Value {
    Value::Array(
        addrs
            .iter()
            .map(|a| json!({ "Address": a, "Name": a }))
            .collect(),
    )
}

/// The most recipients of one message whose protection is looked up while
/// it is written or before it goes, one request each, as many as a `mailto:`
/// link may bring; the frontends say how many more went unchecked.
pub const PROTECTION_LOOKUP_LIMIT: usize = 100;
