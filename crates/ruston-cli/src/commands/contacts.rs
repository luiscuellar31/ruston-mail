//! Contacts + addresses commands.

use crate::cli::{AddressesCmd, ContactsCmd, Ctx, SettingsCmd};
use crate::commands::resume;
use crate::render;
use ruston_core::Result;

pub async fn run(ctx: &Ctx, cmd: ContactsCmd) -> Result<()> {
    let client = resume(&ctx.profile).await?;
    match cmd {
        ContactsCmd::List { page, page_size } => {
            let (total, contacts) = client.list_contacts(page, page_size).await?;
            render::contacts_list(ctx.json, total, &contacts);
        }
        ContactsCmd::Emails { email } => {
            let emails = client.list_contact_emails(email.as_deref()).await?;
            render::contact_emails(ctx.json, &emails);
        }
    }
    Ok(())
}

pub async fn addresses(ctx: &Ctx, cmd: AddressesCmd) -> Result<()> {
    let client = resume(&ctx.profile).await?;
    match cmd {
        AddressesCmd::List => {
            render::addresses_list(ctx.json, &client.addresses());
        }
        AddressesCmd::Update {
            id,
            display_name,
            signature,
        } => {
            client
                .update_address(&id, display_name.as_deref(), signature.as_deref())
                .await?;
            render::action_result(ctx.json, "address updated", std::slice::from_ref(&id));
        }
    }
    Ok(())
}

/// What `settings sign` and `settings attach-key` change, as their output
/// names them.
pub const SIGN: &str = "Sign outgoing mail";
pub const ATTACH_PUBLIC_KEY: &str = "Attach your public key to outgoing mail";

pub async fn settings(ctx: &Ctx, cmd: SettingsCmd) -> Result<()> {
    let client = resume(&ctx.profile).await?;
    match cmd {
        SettingsCmd::Get => {
            let s = client.mail_settings().await?;
            render::json_out(&s);
        }
        SettingsCmd::Sign { value } => {
            client.set_sign(value.as_bool()).await?;
            render::setting_changed(ctx.json, "sign", SIGN, value.as_bool());
        }
        SettingsCmd::AttachPublicKey { value } => {
            client.set_attach_public_key(value.as_bool()).await?;
            render::setting_changed(
                ctx.json,
                "attach_public_key",
                ATTACH_PUBLIC_KEY,
                value.as_bool(),
            );
        }
    }
    Ok(())
}

pub async fn counts(ctx: &Ctx, conversations: bool) -> Result<()> {
    let client = resume(&ctx.profile).await?;
    let counts = if conversations {
        client.conversation_counts().await?
    } else {
        client.message_counts().await?
    };
    render::counts(ctx.json, &counts);
    Ok(())
}
