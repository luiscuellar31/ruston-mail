//! The unread count on the dock or panel icon on Linux, for the docks that
//! read Unity's LauncherEntry signal: Ubuntu's dock, Dash to Dock, KDE's task
//! manager and others.

use std::collections::HashMap;

/// The desktop entry the count belongs to, as the docks name it.
const APP_URI: &str = "application://com.luiscuellar.ruston-mail.desktop";
const INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
/// Where the signal comes from; the docks listen for it on any path.
const PATH: &str = "/com/luiscuellar/RustonMail";

/// Shows `count` on the icon, or takes the count away. The signal goes out
/// on a thread of its own, one update after another, so the interface never
/// waits for the session bus.
#[cfg(target_os = "linux")]
pub(super) fn show_unread(count: Option<u32>) {
    use std::sync::OnceLock;
    use tokio::sync::mpsc;

    static UPDATES: OnceLock<Option<mpsc::UnboundedSender<Option<u32>>>> = OnceLock::new();
    let updates = UPDATES.get_or_init(|| {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        std::thread::Builder::new()
            .name("ruston-launcher-count".into())
            .spawn(move || {
                runtime.block_on(async move {
                    // Without a session bus there is no dock to tell.
                    let Ok(connection) = zbus::Connection::session().await else {
                        return;
                    };
                    while let Some(count) = receiver.recv().await {
                        let _ = update(&connection, count).await;
                    }
                });
            })
            .ok()?;
        Some(sender)
    });
    if let Some(updates) = updates {
        let _ = updates.send(count);
    }
}

async fn update(connection: &zbus::Connection, count: Option<u32>) -> zbus::Result<()> {
    let properties: HashMap<&str, zbus::zvariant::Value<'_>> = [
        ("count", i64::from(count.unwrap_or(0)).into()),
        ("count-visible", count.is_some().into()),
    ]
    .into_iter()
    .collect();
    connection
        .emit_signal(
            None::<()>,
            PATH,
            INTERFACE,
            "Update",
            &(APP_URI, properties),
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn the_count_reaches_the_dock_as_a_launcher_entry_update() {
        let (client, server) = zbus::connection::socket::Channel::pair();
        let guid = zbus::Guid::generate();
        let server = zbus::connection::Builder::authenticated_socket(server, guid.clone())
            .unwrap()
            .p2p()
            .build()
            .await
            .unwrap();
        let client = zbus::connection::Builder::authenticated_socket(client, guid)
            .unwrap()
            .p2p()
            .build()
            .await
            .unwrap();
        let mut received = zbus::MessageStream::from(&server);

        update(&client, Some(7)).await.unwrap();
        update(&client, None).await.unwrap();

        for (count, visible) in [(7, true), (0, false)] {
            let message = received.next().await.unwrap().unwrap();
            let header = message.header();
            assert_eq!(header.interface().unwrap().as_str(), INTERFACE);
            assert_eq!(header.member().unwrap().as_str(), "Update");
            let (uri, properties): (String, HashMap<String, zbus::zvariant::OwnedValue>) =
                message.body().deserialize().unwrap();
            assert_eq!(uri, APP_URI);
            assert_eq!(i64::try_from(&properties["count"]).unwrap(), count);
            assert_eq!(
                bool::try_from(&properties["count-visible"]).unwrap(),
                visible
            );
        }
    }
}
