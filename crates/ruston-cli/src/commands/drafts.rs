//! Draft commands: list, save, edit, delete.

use crate::cli::{Ctx, DraftsCmd};
use crate::commands::messages::{confirm_deletion, require_consent, send_options};
use crate::commands::{read_body, resume};
use crate::{render, terminal};
use ruston_core::Result;
use std::io::{self, Write};

pub async fn run(ctx: &Ctx, cmd: DraftsCmd) -> Result<()> {
    if let DraftsCmd::Delete { yes, .. } = &cmd {
        // Find out whether anyone can be asked before using the network.
        require_consent(ctx, *yes)?;
    }
    let client = resume(&ctx.profile).await?;
    match cmd {
        DraftsCmd::List { page, page_size } => {
            let (total, msgs) = client.list_drafts(page, page_size).await?;
            render::messages_list(ctx.json, total, &msgs);
        }
        DraftsCmd::Save(args) => {
            let body = read_body(&args.body)?;
            let opts = send_options(args, body);
            let id = client.save_draft(&opts).await?;
            render::sent(ctx.json, &id);
        }
        DraftsCmd::Edit { id, args } => {
            let body = read_body(&args.body)?;
            let opts = send_options(args, body);
            client.update_draft(&id, &opts).await?;
            render::action_result(ctx.json, "draft updated", &[id]);
        }
        DraftsCmd::Delete { ids, yes } => {
            if !yes {
                let mut doomed = Vec::with_capacity(ids.len());
                for id in &ids {
                    let draft = client.draft_metadata(id).await?;
                    let to: Vec<&str> = draft.to_list.iter().map(|r| r.address.as_str()).collect();
                    let recipients = if to.is_empty() {
                        "without recipients".to_owned()
                    } else {
                        format!("to {}", to.join(", "))
                    };
                    doomed.push(format!("\"{}\" {recipients} (id {id})", draft.subject));
                }
                let question = format!(
                    "Permanently delete {} draft(s)? This cannot be undone.",
                    ids.len()
                );
                let mut output = io::stderr().lock();
                if !confirm_deletion(&doomed, &question, &mut io::stdin().lock(), &mut output)? {
                    terminal::writeln!(output, "Nothing was deleted.")?;
                    return Ok(());
                }
            }
            client.delete_draft(&ids).await?;
            render::action_result(ctx.json, "draft deleted", &ids);
        }
    }
    Ok(())
}
