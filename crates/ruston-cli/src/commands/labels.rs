//! The `labels` commands, which handle labels and folders alike.

use crate::cli::Ctx;
use crate::cli::LabelsCmd;
use crate::render;
use ruston_core::{Client, Result};

/// Lists the account's labels or folders, creates one, or deletes or
/// changes some.
pub async fn run(ctx: &Ctx, cmd: LabelsCmd) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    // Listing and creating show what they find; a change is reported the
    // same way for every command that makes one.
    let (action, ids) = match cmd {
        LabelsCmd::List { folders } => {
            let kind = render::LabelKind::chosen(folders);
            let labels = match kind {
                render::LabelKind::Labels => client.list_labels().await?,
                render::LabelKind::Folders => client.list_folders().await?,
            };
            render::labels_list(ctx.json, &labels, kind);
            return Ok(());
        }
        LabelsCmd::Create {
            name,
            color,
            folder,
            parent,
        } => {
            render::label_created(
                ctx.json,
                &client
                    .create_label(&name, &color, folder, parent.as_deref())
                    .await?,
            );
            return Ok(());
        }
        LabelsCmd::Delete { ids } => {
            client.delete_labels(&ids).await?;
            ("delete", ids)
        }
        LabelsCmd::Update {
            id,
            name,
            color,
            parent,
        } => {
            client
                .update_label(&id, name.as_deref(), color.as_deref(), parent.as_deref())
                .await?;
            ("update", vec![id])
        }
    };
    render::action_result(ctx.json, action, &ids);
    Ok(())
}
