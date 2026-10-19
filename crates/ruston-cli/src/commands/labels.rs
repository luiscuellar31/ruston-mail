//! Label / folder commands.

use crate::cli::Ctx;
use crate::cli::LabelsCmd;
use crate::commands::resume;
use crate::render;
use ruston_core::Result;

pub async fn run(ctx: &Ctx, cmd: LabelsCmd) -> Result<()> {
    let client = resume(&ctx.profile).await?;
    match cmd {
        LabelsCmd::List { folders } => {
            let kind = render::LabelKind::chosen(folders);
            let labels = match kind {
                render::LabelKind::Labels => client.list_labels().await?,
                render::LabelKind::Folders => client.list_folders().await?,
            };
            render::labels_list(ctx.json, &labels, kind);
        }
        LabelsCmd::Create {
            name,
            color,
            folder,
            parent,
        } => {
            let label = client
                .create_label(&name, &color, folder, parent.as_deref())
                .await?;
            render::label_created(ctx.json, &label);
        }
        LabelsCmd::Delete { ids } => {
            client.delete_labels(&ids).await?;
            render::action_result(ctx.json, "delete", &ids);
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
            render::action_result(ctx.json, "updated", std::slice::from_ref(&id));
        }
    }
    Ok(())
}
