//! Attachment commands.

use crate::cli::AttachmentsCmd;
use crate::cli::Ctx;
use crate::commands::resume;
use crate::render;
use ruston_core::mail::attachments::safe_attachment_name;
use ruston_core::{Error, Result};
use serde_json::json;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

const MAX_NAME_ATTEMPTS: u32 = 10_000;

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

            let files: Vec<(String, Vec<u8>)> = if all {
                client
                    .download_all_attachments(&msg_id, include_inline)
                    .await?
            } else if let Some(att_id) = attachment {
                vec![client.download_attachment(&msg_id, &att_id).await?]
            } else {
                return Err(Error::Other(
                    "specify an attachment ID or use --all".to_string(),
                ));
            };

            let mut written = Vec::with_capacity(files.len());
            for (name, bytes) in &files {
                let path = save_attachment(&dir, name, bytes)?;
                written.push(path.to_string_lossy().to_string());
            }

            if ctx.json {
                render::json_out(&json!({
                    "status": "ok",
                    "count": written.len(),
                    "files": written,
                }));
            } else {
                println!("Downloaded {} file(s):", written.len());
                for f in &written {
                    println!("  {f}");
                }
            }
            Ok(())
        }
    }
}

/// Create the destination exclusively, then write it. A concurrent download or
/// symlink cannot replace an existing file between choosing its name and opening it.
fn save_attachment(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let name = safe_attachment_name(name);
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name.as_str(), String::new()),
    };

    for attempt in 0..MAX_NAME_ATTEMPTS {
        let path = if attempt == 0 {
            dir.join(&name)
        } else {
            dir.join(format!("{stem} ({attempt}){extension}"))
        };
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        if let Err(error) = file.write_all(bytes) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        return Ok(path);
    }

    Err(std::io::Error::new(
        ErrorKind::AlreadyExists,
        "no available filename for attachment",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ruston-cli-attachments-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn untrusted_names_stay_inside_the_output_directory() {
        let dir = test_dir("names");
        for (untrusted, expected) in [
            ("../../outside.txt", "outside.txt"),
            ("/tmp/absolute.txt", "absolute.txt"),
            (r"..\windows\file.txt", "file.txt"),
            ("C:drive.txt", "attachment"),
            ("CON.txt", "_CON.txt"),
            ("CON .txt", "_CON .txt"),
            ("...", "attachment (1)"),
        ] {
            let path = save_attachment(&dir, untrusted, b"mail").unwrap();
            assert_eq!(path.parent(), Some(dir.as_path()));
            assert_eq!(path.file_name().unwrap(), expected);
            assert_eq!(std::fs::read(path).unwrap(), b"mail");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_saves_claim_different_names_without_overwriting() {
        let dir = test_dir("concurrent");
        std::fs::write(dir.join("report.txt"), b"existing").unwrap();
        let handles: Vec<_> = (0..8)
            .map(|value| {
                let dir = dir.clone();
                std::thread::spawn(move || save_attachment(&dir, "report.txt", &[value]).unwrap())
            })
            .collect();
        let mut paths = std::collections::HashSet::new();
        let mut contents = std::collections::HashSet::new();
        for handle in handles {
            let path = handle.join().unwrap();
            assert!(paths.insert(path.clone()));
            contents.insert(std::fs::read(path).unwrap()[0]);
        }
        assert_eq!(contents.len(), 8);
        assert_eq!(std::fs::read(dir.join("report.txt")).unwrap(), b"existing");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_never_followed() {
        let dir = test_dir("symlink");
        let outside = dir.parent().unwrap().join(format!(
            "ruston-cli-attachments-outside-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&outside);
        std::os::unix::fs::symlink(&outside, dir.join("report.txt")).unwrap();
        let path = save_attachment(&dir, "report.txt", b"mail").unwrap();
        assert_eq!(path, dir.join("report (1).txt"));
        assert!(!outside.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
