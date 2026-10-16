//! Export command: dump a folder to `.eml` files.

use crate::cli::Ctx;
use crate::commands::resume;
use crate::render;
use crate::terminal;
use ruston_core::{Error, Result};
use std::path::PathBuf;

pub async fn run(ctx: &Ctx, folder: String, out: PathBuf, max: u32) -> Result<()> {
    let client = resume(&ctx.profile).await?;
    let report = client.export_folder_report(&folder, &out, max).await?;
    if ctx.json {
        let skipped: Vec<_> = report
            .skipped
            .iter()
            .map(|skipped| {
                serde_json::json!({
                    "id": skipped.meta.id,
                    "subject": skipped.meta.subject,
                    "error": skipped.error.to_string(),
                })
            })
            .collect();
        render::json_out(&serde_json::json!({
            "exported": report.written,
            "already_exported": report.already_exported,
            "skipped": skipped,
            "out": out.display().to_string(),
        }));
    } else {
        terminal::println!(
            "Exported {} message(s) to {}",
            report.written,
            out.display()
        );
        if report.already_exported > 0 {
            terminal::println!(
                "{} message(s) were already exported there",
                report.already_exported
            );
        }
        for skipped in &report.skipped {
            terminal::println!(
                "Skipped {} \"{}\": {}",
                skipped.meta.id,
                skipped.meta.subject,
                skipped.error
            );
        }
    }
    // Exit 0 only for an export with every message in it.
    if !report.skipped.is_empty() {
        return Err(Error::Other(format!(
            "{} message(s) could not be exported",
            report.skipped.len()
        )));
    }
    Ok(())
}
