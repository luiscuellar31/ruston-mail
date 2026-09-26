//! Watch command: continuously sync the cache from the event stream.

use crate::cli::Ctx;
use crate::commands::resume;
use ruston_core::{Result, SyncReport};
use std::time::Duration;

fn needs_index(report: &SyncReport, first_tick: bool) -> bool {
    first_tick || report.created > 0 || report.updated > 0 || report.rebuilt.is_some()
}

pub async fn run(ctx: &Ctx, interval: u64, folder: Option<String>) -> Result<()> {
    let client = resume(&ctx.profile).await?;
    eprintln!("watching (every {interval}s; Ctrl-C to stop)…");
    let mut first_tick = true;
    loop {
        let r = client.sync().await?;
        if let Some(f) = &folder {
            if first_tick {
                client.cache_folder(f, 4, 50).await?;
            }
            if needs_index(&r, first_tick) {
                client.index_folder(f, 2, 50).await?;
            }
        }
        first_tick = false;
        let tag = if r.initialized {
            " (initialized)".to_string()
        } else if let Some(rebuilt) = r.rebuilt {
            format!(" (rebuilt {rebuilt})")
        } else {
            String::new()
        };
        println!("[sync] +{} ~{} -{}{tag}", r.created, r.updated, r.deleted);
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_or_delete_only_ticks_skip_reindexing() {
        let mut report = SyncReport {
            created: 0,
            updated: 0,
            deleted: 0,
            event_id: "cursor".into(),
            initialized: false,
            rebuilt: None,
        };
        assert!(needs_index(&report, true));
        assert!(!needs_index(&report, false));
        report.deleted = 1;
        assert!(!needs_index(&report, false));
        report.created = 1;
        assert!(needs_index(&report, false));
        report.created = 0;
        report.updated = 1;
        assert!(needs_index(&report, false));
        report.updated = 0;
        report.rebuilt = Some(0);
        assert!(needs_index(&report, false));
    }
}
