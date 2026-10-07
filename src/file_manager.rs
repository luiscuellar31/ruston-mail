//! Reveals downloaded files through the desktop's native file manager.

use std::path::Path;
use std::process::Command;

/// Uses argument vectors rather than interpreting paths through a shell.
pub async fn reveal(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        // Bound connection, service activation, and reply together. A desktop
        // without FileManager1 should still open the containing folder promptly.
        let selection = async {
            let connection = zbus::Connection::session().await?;
            show_items(&connection, path).await
        };
        if matches!(
            tokio::time::timeout(std::time::Duration::from_secs(3), selection).await,
            Ok(Ok(()))
        ) {
            return Ok(());
        }
    }

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg("-R").arg(path);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("explorer");
        command.arg(format!("/select,{}", path.display()));
        command
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(path.parent().unwrap_or(path));
        command
    };
    command.spawn().map(drop)
}

#[cfg(any(target_os = "linux", test))]
async fn show_items(connection: &zbus::Connection, path: &Path) -> zbus::Result<()> {
    let uri = url::Url::from_file_path(path)
        .map_err(|()| zbus::Error::Failure("Expected an absolute file path".into()))?;
    connection
        .call_method(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            Some("org.freedesktop.FileManager1"),
            "ShowItems",
            &(vec![uri.as_str()], ""),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct ShowItemsCall {
        uris: Vec<String>,
        startup_id: String,
    }

    struct FileManager {
        calls: Arc<Mutex<Vec<ShowItemsCall>>>,
        fail: bool,
    }

    #[zbus::interface(name = "org.freedesktop.FileManager1")]
    impl FileManager {
        fn show_items(&self, uris: Vec<String>, startup_id: String) -> zbus::fdo::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(ShowItemsCall { uris, startup_id });
            if self.fail {
                Err(zbus::fdo::Error::Failed("Selection unavailable".into()))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn file_manager_receives_encoded_uri_and_failure_is_reported() {
        // In-memory D-Bus peers exercise the real message signature and method
        // without launching a file manager or requiring a session bus in CI.
        for fail in [false, true] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let (client, server) = zbus::connection::socket::Channel::pair();
            let guid = zbus::Guid::generate();
            let server = zbus::connection::Builder::authenticated_socket(server, guid.clone())
                .unwrap()
                .p2p()
                .serve_at(
                    "/org/freedesktop/FileManager1",
                    FileManager {
                        calls: calls.clone(),
                        fail,
                    },
                )
                .unwrap()
                .build()
                .await
                .unwrap();
            let client = zbus::connection::Builder::authenticated_socket(client, guid)
                .unwrap()
                .p2p()
                .build()
                .await
                .unwrap();
            let path = std::env::temp_dir().join("informe español 100% # ' final.pdf");
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                show_items(&client, &path),
            )
            .await
            .unwrap();
            assert_eq!(result.is_err(), fail);
            assert!(
                show_items(&client, Path::new("relative.pdf"))
                    .await
                    .is_err()
            );
            let calls = calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].startup_id, "");
            assert_eq!(calls[0].uris.len(), 1);
            let uri = url::Url::parse(&calls[0].uris[0]).unwrap();
            assert_eq!(uri.to_file_path().unwrap(), path);
            assert!(uri.as_str().contains("%25%20%23"));
            drop(server);
        }
    }
}
