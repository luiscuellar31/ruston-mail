//! Account commands: contacts, sending addresses, mail settings and counts.
//! Each makes at most one request, whose answer goes straight to its report.

use crate::cli::{AddressesCmd, ContactsCmd, Ctx, SettingsCmd};
use crate::render;
use ruston_core::{Client, Result};

/// Lists the account's contacts, a page at a time, or the email addresses
/// they hold.
pub async fn run(ctx: &Ctx, cmd: ContactsCmd) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    match cmd {
        ContactsCmd::List { page, page_size } => client
            .list_contacts(page, page_size)
            .await
            .map(|(total, contacts)| render::contacts_list(ctx.json, total, &contacts)),
        ContactsCmd::Emails { email } => client
            .list_contact_emails(email.as_deref())
            .await
            .map(|emails| render::contact_emails(ctx.json, &emails)),
    }
}

/// Lists the addresses the account sends from, which come with the session,
/// or changes the display name or signature of one.
pub async fn addresses(ctx: &Ctx, cmd: AddressesCmd) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    match cmd {
        AddressesCmd::List => {
            render::addresses_list(ctx.json, &client.addresses());
            Ok(())
        }
        AddressesCmd::Update {
            id,
            display_name,
            signature,
        } => client
            .update_address(&id, display_name.as_deref(), signature.as_deref())
            .await
            .map(|()| render::action_result(ctx.json, "update", &[id])),
    }
}

/// What `settings sign` and `settings attach-key` change, as their output
/// names them.
pub const SIGN: &str = "Sign outgoing mail";
pub const ATTACH_PUBLIC_KEY: &str = "Attach your public key to outgoing mail";

/// Shows the mail settings as Proton sends them, or turns one on or off.
pub async fn settings(ctx: &Ctx, cmd: SettingsCmd) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    match cmd {
        SettingsCmd::Get => client
            .mail_settings()
            .await
            .map(|settings| render::json_out(&settings)),
        SettingsCmd::Sign { value } => {
            let on = value.as_bool();
            client
                .set_sign(on)
                .await
                .map(|()| render::setting_changed(ctx.json, "sign", SIGN, on))
        }
        SettingsCmd::AttachPublicKey { value } => {
            let on = value.as_bool();
            client.set_attach_public_key(on).await.map(|()| {
                render::setting_changed(ctx.json, "attach_public_key", ATTACH_PUBLIC_KEY, on)
            })
        }
    }
}

/// Shows how many messages each folder and label holds, and how many of
/// them are unread; with `conversations`, how many conversations.
pub async fn counts(ctx: &Ctx, conversations: bool) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    let counted = if conversations {
        client.conversation_counts().await
    } else {
        client.message_counts().await
    };
    counted.map(|counts| render::counts(ctx.json, &counts))
}
