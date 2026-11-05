//! `sync`, which brings the local cache up to date with Proton's events, and
//! can cache pages of a folder as well.

use crate::cli::Ctx;
use crate::render;
use crate::terminal;
use ruston_core::{Client, Result, SyncReport};
use serde_json::json;

/// Brings the local cache up to date with Proton's event stream, then, with
/// `backfill`, caches up to `max_pages` pages of that folder too.
pub async fn run(
    ctx: &Ctx,
    backfill: Option<String>,
    max_pages: u32,
    page_size: u32,
) -> Result<()> {
    let client = Client::resume(&ctx.profile).await?;
    // Found before syncing, so a folder that does not exist stops the
    // command before the cache changes.
    let backfill = match backfill {
        Some(folder) => Some(client.folder_id(&folder).await?),
        None => None,
    };
    let report = client.sync().await?;
    let backfilled = match backfill {
        Some(folder) => client.cache_folder(&folder, max_pages, page_size).await?,
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
    } else {
        for line in summary(&report, backfilled) {
            terminal::println!("{line}");
        }
    }
    Ok(())
}

/// The text report of a sync: that a first one only stored where the next
/// one starts, or what changed and whether the cache was rebuilt; then how
/// many messages a backfill added, if any.
fn summary(report: &SyncReport, backfilled: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if report.initialized {
        lines.push("Sync cursor initialized — run `sync` again to pull deltas.".to_owned());
    } else {
        lines.push(format!(
            "Synced: {} created, {} updated, {} deleted.",
            report.created, report.updated, report.deleted
        ));
        if let Some(rebuilt) = report.rebuilt {
            lines.push(format!("Rebuilt local cache: {rebuilt} message(s)."));
            lines.push("Run `index` to rebuild local search.".to_owned());
        }
    }
    if backfilled > 0 {
        lines.push(format!("Backfilled {backfilled} message(s)."));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(initialized: bool, rebuilt: Option<usize>) -> SyncReport {
        SyncReport {
            created: 1,
            updated: 2,
            flags_updated: 0,
            deleted: 3,
            event_id: "event".into(),
            initialized,
            rebuilt,
        }
    }

    #[test]
    fn a_sync_reports_what_changed_and_what_a_backfill_added() {
        let first = "Sync cursor initialized — run `sync` again to pull deltas.";
        assert_eq!(summary(&report(true, None), 0), [first]);
        assert_eq!(
            summary(&report(true, None), 4),
            [first, "Backfilled 4 message(s)."]
        );

        let synced = "Synced: 1 created, 2 updated, 3 deleted.";
        assert_eq!(summary(&report(false, None), 0), [synced]);
        assert_eq!(
            summary(&report(false, Some(5)), 2),
            [
                synced,
                "Rebuilt local cache: 5 message(s).",
                "Run `index` to rebuild local search.",
                "Backfilled 2 message(s).",
            ]
        );
    }
}
