//! Mail settings (`/mail/v4/settings`): reading them, and turning some on or
//! off.

use crate::error::Result;
use crate::transport::{Doer, Request};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::json;

#[derive(Deserialize)]
struct Resp {
    #[serde(rename = "MailSettings", default)]
    mail_settings: serde_json::Value,
}

/// The account's mail settings, as Proton sends them.
pub async fn get_mail_settings<D: Doer>(d: &D) -> Result<serde_json::Value> {
    let Resp { mail_settings } = d.decode(Request::get("/mail/v4/settings")).await?;
    Ok(mail_settings)
}

/// Turns on or off the setting kept at `path`, which Proton names `field`
/// and reads as 1 or 0.
async fn switch<D: Doer>(d: &D, path: &str, field: &str, on: bool) -> Result<()> {
    let request =
        Request::put(format!("/mail/v4/settings/{path}")).json(json!({ field: i64::from(on) }));
    d.decode::<IgnoredAny>(request).await?;
    Ok(())
}

/// Turns signing outgoing mail on or off.
pub async fn set_sign<D: Doer>(d: &D, on: bool) -> Result<()> {
    switch(d, "sign", "Sign", on).await
}

/// Turns attaching the account's public key to outgoing mail on or off.
pub async fn set_attach_public_key<D: Doer>(d: &D, on: bool) -> Result<()> {
    switch(d, "attachpublic", "AttachPublicKey", on).await
}
