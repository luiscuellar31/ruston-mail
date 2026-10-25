//! Attachment commands.

use crate::cli::AttachmentsCmd;
use crate::cli::Ctx;
use crate::commands::resume;
use crate::render;
use ruston_core::mail::attachments::save_under_free_name;
use ruston_core::{Error, Result};
use std::path::PathBuf;

pub async fn run(ctx: &Ctx, cmd: AttachmentsCmd) -> Result<()> {
    let client = resume(&ctx.profile).await?;
    match cmd {
        AttachmentsCmd::List {
            message,
            include_inline,
        } => {
            let msg_id = client.resolve_ref(&message).await?;
            let atts = client.list_attachments(&msg_id, include_inline).await?;
            render::attachments_list(ctx.json, &atts);
            Ok(())
        }
        AttachmentsCmd::Download {
            message,
            attachment,
            output_dir,
            all,
            include_inline,
        } => {
            let msg_id = client.resolve_ref(&message).await?;
            let dir = output_dir.unwrap_or_else(|| PathBuf::from("."));
            std::fs::create_dir_all(&dir)?;

            let mut written = Vec::new();
            if all {
                client
                    .for_each_attachment(&msg_id, include_inline, |name, bytes| {
                        let path = save_under_free_name(&dir, &name, &bytes)?;
                        written.push(path.to_string_lossy().to_string());
                        Ok(())
                    })
                    .await?;
            } else if let Some(att_id) = attachment {
                let (name, bytes) = client.download_attachment(&msg_id, &att_id).await?;
                let path = save_under_free_name(&dir, &name, &bytes)?;
                written.push(path.to_string_lossy().to_string());
            } else {
                return Err(Error::Other(
                    "specify an attachment ID or use --all".to_string(),
                ));
            }

            render::downloaded(ctx.json, &written);
            Ok(())
        }
    }
}
