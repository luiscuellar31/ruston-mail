//! The `filters` commands, for the account's Sieve filters.

use crate::cli::{Ctx, FiltersCmd};
use crate::commands::read_body;
use crate::render;
use crate::terminal;
use ruston_core::{Client, Result};

/// Lists, creates or checks Sieve filters, or deletes, enables or disables
/// one.
pub async fn run(ctx: &Ctx, cmd: FiltersCmd) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    // Listing, creating and checking show what they find; a change to a
    // filter is reported the same way whichever it is.
    let (action, id) = match cmd {
        FiltersCmd::List => {
            render::filters_list(ctx.json, &client.list_filters().await?);
            return Ok(());
        }
        FiltersCmd::Create { name, sieve } => {
            let script = read_body(&sieve)?;
            render::filter_created(ctx.json, &client.create_filter(&name, &script).await?);
            return Ok(());
        }
        FiltersCmd::Check { sieve } => {
            client.check_filter(&read_body(&sieve)?).await?;
            if ctx.json {
                render::json_out(&serde_json::json!({ "valid": true }));
            } else {
                terminal::println!("Sieve script is valid.");
            }
            return Ok(());
        }
        FiltersCmd::Delete { id } => {
            client.delete_filter(&id).await?;
            ("delete", id)
        }
        FiltersCmd::Enable { id } => {
            client.enable_filter(&id).await?;
            ("enable", id)
        }
        FiltersCmd::Disable { id } => {
            client.disable_filter(&id).await?;
            ("disable", id)
        }
    };
    render::action_result(ctx.json, action, &[id]);
    Ok(())
}
