//! `attachments list` and `attachments download`, which find a message's
//! attachments and save them.

use crate::cli::AttachmentsCmd;
use crate::cli::Ctx;
use crate::render;
use ruston_core::mail::attachments::save_under_free_name;
use ruston_core::{Client, Result};
use std::path::{Path, PathBuf};

/// Lists the attachments of the message a reference names, or saves them.
pub async fn run(ctx: &Ctx, cmd: AttachmentsCmd) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    // Both commands start from the message, found the same way.
    let (AttachmentsCmd::List { message, .. } | AttachmentsCmd::Download { message, .. }) = &cmd;
    let message_id = client.resolve_ref(message).await?;
    match cmd {
        AttachmentsCmd::List { include_inline, .. } => {
            let listed = client.list_attachments(&message_id, include_inline).await?;
            render::attachments_list(ctx.json, &listed);
        }
        AttachmentsCmd::Download {
            attachment,
            output_dir,
            include_inline,
            ..
        } => {
            let folder = output_dir.unwrap_or_else(|| PathBuf::from("."));
            std::fs::create_dir_all(&folder)?;
            // Parsing takes an attachment ID or `--all`, never both or
            // neither, so the ID alone says which.
            let saved = save_attachments(
                &client,
                &message_id,
                attachment.as_deref(),
                include_inline,
                &folder,
            )
            .await?;
            render::downloaded(ctx.json, &saved);
        }
    }
    Ok(())
}

/// Saves into `folder` the attachment of the message that `attachment`
/// names, or without one all of them, inline images too with
/// `include_inline`. Each is written as it arrives, under a name nothing in
/// the folder holds yet; the paths come back in that order.
async fn save_attachments(
    client: &Client,
    message_id: &str,
    attachment: Option<&str>,
    include_inline: bool,
    folder: &Path,
) -> Result<Vec<String>> {
    let mut saved = Vec::new();
    let mut save = |name: String, contents: Vec<u8>| -> Result<()> {
        let path = save_under_free_name(folder, &name, &contents)?;
        saved.push(path.to_string_lossy().into_owned());
        Ok(())
    };
    match attachment {
        Some(attachment) => {
            let (name, contents) = client.download_attachment(message_id, attachment).await?;
            save(name, contents)?;
        }
        None => {
            client
                .for_each_attachment(message_id, include_inline, save)
                .await?
        }
    }
    Ok(saved)
}
