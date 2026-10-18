//! Watch command: continuously sync the cache from the event stream.

use crate::cli::Ctx;
use crate::commands::resume;
use crate::terminal;
use ruston_core::{Client, Error, Result, SyncReport};
use std::time::Duration;

/// The first tick caches the newest 200 messages of the folder.
const CACHE_PAGES: u32 = 4;
/// Indexing covers the newest 100 messages of the folder.
const INDEX_PAGES: u32 = 2;
const PAGE_SIZE: u32 = 50;
/// The longest wait between attempts while errors keep coming.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(10 * 60);

/// Flag and label changes leave indexed bodies current, so only new or
/// rewritten mail, or a rebuilt cache, is worth indexing again. A `forced`
/// tick indexes anyway: the first, and any after an error, which may have
/// come after its sync took in changes it did not index.
fn needs_index(report: &SyncReport, forced: bool) -> bool {
    forced
        || report.created > 0
        || report.updated > report.flags_updated
        || report.rebuilt.is_some()
}

/// Whether `error` may pass if `watch` tries again later: the network,
/// Proton being busy or down, or the cache changing during a sync. Anything
/// else, a refused session first, stops it.
fn is_transient(error: &Error) -> bool {
    match error {
        Error::Http(_) => true,
        Error::Api(api) => api.http_status == 429 || api.http_status >= 500,
        Error::Cache(message) => message.contains("retry"),
        _ => false,
    }
}

/// How long to wait after `failures` errors in a row: the interval, doubled
/// for each further one, up to [`MAX_RETRY_DELAY`] unless the interval is
/// longer still.
fn retry_delay(interval: Duration, failures: u32) -> Duration {
    let doubled = interval.saturating_mul(2_u32.saturating_pow(failures.saturating_sub(1)));
    doubled.min(MAX_RETRY_DELAY.max(interval))
}

pub async fn run(ctx: &Ctx, interval: u64, folder: Option<String>) -> Result<()> {
    let client = resume(&ctx.profile).await?;
    terminal::eprintln!("watching (every {interval}s; Ctrl-C to stop)…");
    let interval = Duration::from_secs(interval);
    let mut first_tick = true;
    let mut failures = 0;
    loop {
        let forced = first_tick || failures > 0;
        match tick(&client, folder.as_deref(), first_tick, forced).await {
            Ok(r) => {
                first_tick = false;
                failures = 0;
                let tag = if r.initialized {
                    " (initialized)".to_string()
                } else if let Some(rebuilt) = r.rebuilt {
                    format!(" (rebuilt {rebuilt})")
                } else {
                    String::new()
                };
                terminal::println!("[sync] +{} ~{} -{}{tag}", r.created, r.updated, r.deleted);
                tokio::time::sleep(interval).await;
            }
            Err(error) if is_transient(&error) => {
                failures += 1;
                let delay = retry_delay(interval, failures);
                terminal::eprintln!("[sync] {error}; trying again in {}s", delay.as_secs());
                tokio::time::sleep(delay).await;
            }
            Err(error) => return Err(error),
        }
    }
}

/// One round: a sync, then for a folder its cache on the first round, and
/// its index when mail changed or the round is `forced`.
async fn tick(
    client: &Client,
    folder: Option<&str>,
    first_tick: bool,
    forced: bool,
) -> Result<SyncReport> {
    let report = client.sync().await?;
    if let Some(folder) = folder {
        if first_tick {
            client.cache_folder(folder, CACHE_PAGES, PAGE_SIZE).await?;
        }
        if needs_index(&report, forced) {
            client.index_folder(folder, INDEX_PAGES, PAGE_SIZE).await?;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruston_core::ApiError;

    #[test]
    fn only_errors_that_may_pass_are_tried_again() {
        let api = |http_status| {
            Error::Api(ApiError {
                http_status,
                code: 0,
                message: String::new(),
                raw_body: String::new(),
            })
        };
        for status in [429, 500, 503] {
            assert!(is_transient(&api(status)), "{status}");
        }
        assert!(is_transient(&Error::Cache(
            "mailbox changed during resync; retry".into()
        )));
        // A refused session, or a request Proton will refuse again, stops it.
        for status in [401, 403, 422] {
            assert!(!is_transient(&api(status)), "{status}");
        }
        assert!(!is_transient(&Error::Unauthorized));
        assert!(!is_transient(&Error::Session("keyring locked".into())));
        assert!(!is_transient(&Error::Cache(
            "database disk image is malformed".into()
        )));
    }

    #[test]
    fn errors_in_a_row_wait_longer_up_to_a_limit() {
        let interval = Duration::from_secs(30);
        assert_eq!(retry_delay(interval, 1), Duration::from_secs(30));
        assert_eq!(retry_delay(interval, 2), Duration::from_secs(60));
        assert_eq!(retry_delay(interval, 3), Duration::from_secs(120));
        assert_eq!(retry_delay(interval, 40), MAX_RETRY_DELAY);
        // An interval longer than the limit is kept.
        let hourly = Duration::from_secs(60 * 60);
        assert_eq!(retry_delay(hourly, 5), hourly);
    }

    #[test]
    fn idle_delete_or_flag_only_ticks_skip_reindexing() {
        let mut report = SyncReport {
            created: 0,
            updated: 0,
            flags_updated: 0,
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
        // Read, starred or moved elsewhere: the indexed bodies still match.
        report.flags_updated = 1;
        assert!(!needs_index(&report, false));
        report.updated = 2;
        assert!(needs_index(&report, false), "one message was rewritten");
        report.updated = 0;
        report.flags_updated = 0;
        report.rebuilt = Some(0);
        assert!(needs_index(&report, false));
    }
}
