//! Sync command: pull events into the local cache, optionally backfill a folder.

use crate::cli::Ctx;
use crate::commands::resume;
use crate::render;
use crate::terminal;
use ruston_core::Result;
use serde_json::json;

pub async fn run(
    ctx: &Ctx,
    backfill: Option<String>,
    max_pages: u32,
    page_size: u32,
) -> Result<()> {
    let client = resume(&ctx.profile).await?;
    let report = client.sync().await?;
    let backfilled = match &backfill {
        Some(folder) => client.cache_folder(folder, max_pages, page_size).await?,
        None => 0,
    };

    if ctx.json {
        render::json_out(&json!({
            "initialized": report.initialized,
            "created": report.created,
            "updated": report.updated,
            "deleted": report.deleted,
            "event_id": report.event_id,
            "rebuilt": report.rebuilt,
            "backfilled": backfilled,
        }));
    } else if report.initialized {
        terminal::println!("Sync cursor initialized — run `sync` again to pull deltas.");
        if backfilled > 0 {
            terminal::println!("Backfilled {backfilled} message(s).");
        }
    } else {
        terminal::println!(
            "Synced: {} created, {} updated, {} deleted.",
            report.created,
            report.updated,
            report.deleted
        );
        if let Some(rebuilt) = report.rebuilt {
            terminal::println!("Rebuilt local cache: {rebuilt} message(s).");
            terminal::println!("Run `index` to rebuild local search.");
        }
        if backfilled > 0 {
            terminal::println!("Backfilled {backfilled} message(s).");
        }
    }
    Ok(())
}
