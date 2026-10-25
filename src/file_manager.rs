//! Reveals downloaded files through the desktop's native file manager.

use std::path::Path;
#[cfg(not(target_os = "windows"))]
use std::process::Command;

/// Uses argument vectors or the shell's API rather than interpreting paths
/// through a shell.
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

    #[cfg(target_os = "windows")]
    {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || select_in_explorer(&path))
            .await
            .map_err(std::io::Error::other)?
    }
    #[cfg(not(target_os = "windows"))]
    {
        #[cfg(target_os = "macos")]
        let command = {
            let mut command = Command::new("open");
            command.arg("-R").arg(path);
            command
        };
        #[cfg(not(target_os = "macos"))]
        let command = {
            let mut command = Command::new("xdg-open");
            command.arg(path.parent().unwrap_or(path));
            command
        };
        crate::runtime::run_to_exit(command).await.map(drop)
    }
}

/// Opens the file's folder in Explorer with the file selected. The shell API
/// takes the path whole: Explorer's command line splits `/select,` at commas,
/// and `explorer` named without a folder is looked for beside this
/// executable first.
#[cfg(target_os = "windows")]
fn select_in_explorer(path: &Path) -> std::io::Result<()> {
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows::Win32::UI::Shell::{ILFree, SHOpenFolderAndSelectItems, SHParseDisplayName};
    use windows::core::HSTRING;

    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            // SAFETY: created only after CoInitializeEx succeeded on this
            // blocking worker, which it cannot leave.
            unsafe { CoUninitialize() };
        }
    }

    // SAFETY: initializes COM for this blocking worker; the guard balances it.
    let initialized =
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
    // A worker already in the multithreaded apartment can use the shell too.
    if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
        return Err(windows::core::Error::from_hresult(initialized).into());
    }
    let _apartment = initialized.is_ok().then_some(Apartment);

    let mut item = std::ptr::null_mut();
    // SAFETY: the name is a live, NUL-terminated wide string, and `item`
    // receives an item ID list the shell allocates.
    unsafe { SHParseDisplayName(&HSTRING::from(path.as_os_str()), None, &mut item, 0, None) }?;
    // SAFETY: with no children, the full ID list names the item to select in
    // its parent folder; it is freed once the call has returned.
    let selected = unsafe { SHOpenFolderAndSelectItems(item, None, 0) };
    // SAFETY: `item` came from SHParseDisplayName and is not used again.
    unsafe { ILFree(Some(item)) };
    Ok(selected?)
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

    #[test]
    #[cfg(target_os = "windows")]
    fn explorer_stays_closed_for_a_file_that_is_gone() {
        // The shell resolves the path before Explorer opens, so no window
        // appears; a comma is where Explorer's command line used to split.
        let missing = std::env::temp_dir().join("ruston missing, informe ñ.pdf");
        assert!(select_in_explorer(&missing).is_err());
    }
}
