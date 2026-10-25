use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[cfg(target_os = "linux")]
use super::Status;

/// The notification shown last, so the next one replaces it instead of
/// piling up beside it, and signing out can take it away. 0 is none, as in
/// the notification specification.
#[derive(Clone, Default)]
pub(super) struct Shown(Arc<AtomicU32>);

impl Shown {
    /// Whether a notification may still be on screen.
    #[cfg(target_os = "linux")]
    pub(super) fn is_some(&self) -> bool {
        self.0.load(Ordering::Acquire) != 0
    }
}

#[cfg(target_os = "linux")]
pub(super) async fn show(
    sender: &str,
    subject: &str,
    enabled: Arc<AtomicBool>,
    shown: Shown,
) -> Status {
    let request = async {
        let connection = zbus::Connection::session().await?;
        show_on(&connection, sender, subject, &enabled, &shown).await
    };
    match tokio::time::timeout(std::time::Duration::from_secs(3), request).await {
        Ok(Ok(true)) => Status::Ready,
        Ok(Ok(false)) => Status::Off,
        _ => Status::Unavailable,
    }
}

/// Takes away the notification shown last, if it is still there.
#[cfg(target_os = "linux")]
pub(super) async fn close(shown: Shown) {
    let request = async {
        let connection = zbus::Connection::session().await?;
        close_on(&connection, &shown).await
    };
    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), request).await;
}

/// Shows the notification in place of the one shown last, unless
/// notifications were turned off; one that arrives as they are turned off is
/// taken away again, since its account is gone. Whether it stays shown.
async fn show_on(
    connection: &zbus::Connection,
    sender: &str,
    subject: &str,
    enabled: &AtomicBool,
    shown: &Shown,
) -> zbus::Result<bool> {
    if !enabled.load(Ordering::Acquire) {
        return Ok(false);
    }
    let replaces = shown.0.load(Ordering::Acquire);
    let id = notify(connection, sender, subject, replaces).await?;
    shown.0.store(id, Ordering::Release);
    if !enabled.load(Ordering::Acquire) {
        close_on(connection, shown).await?;
        return Ok(false);
    }
    Ok(true)
}

async fn close_on(connection: &zbus::Connection, shown: &Shown) -> zbus::Result<()> {
    let id = shown.0.swap(0, Ordering::AcqRel);
    if id != 0 {
        connection
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.Notifications"),
                "CloseNotification",
                &(id,),
            )
            .await?;
    }
    Ok(())
}

async fn notify(
    connection: &zbus::Connection,
    sender: &str,
    subject: &str,
    replaces: u32,
) -> zbus::Result<u32> {
    // Notification bodies support markup; encode headers as literal text so
    // a sender cannot inject links or remotely loaded images.
    let body = subject
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let hints: HashMap<&str, zbus::zvariant::Value<'_>> = [
        ("desktop-entry", crate::ui::identity::APP_ID.into()),
        ("category", "email.arrived".into()),
    ]
    .into_iter()
    .collect();
    let reply = connection
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "Ruston Mail",
                replaces,
                crate::ui::identity::APP_ID,
                sender,
                body,
                Vec::<&str>::new(),
                hints,
                -1_i32,
            ),
        )
        .await?;
    reply.body().deserialize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Received {
        body: String,
        app: String,
        icon: String,
        summary: String,
        hints: HashMap<String, zbus::zvariant::OwnedValue>,
    }

    #[derive(Default)]
    struct Calls {
        last: Option<Received>,
        /// The `replaces_id` of each notification, in order.
        replaced: Vec<u32>,
        closed: Vec<u32>,
        issued: u32,
    }

    #[derive(Default)]
    struct Service {
        calls: Arc<Mutex<Calls>>,
        /// Turned off while a notification is on its way, when set.
        turn_off: Option<Arc<AtomicBool>>,
    }

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Service {
        #[allow(clippy::too_many_arguments)] // The freedesktop Notify signature.
        fn notify(
            &self,
            app: String,
            replaces: u32,
            icon: String,
            summary: String,
            body: String,
            actions: Vec<String>,
            hints: HashMap<String, zbus::zvariant::OwnedValue>,
            timeout: i32,
        ) -> u32 {
            assert!(actions.is_empty());
            assert_eq!(timeout, -1);
            if let Some(enabled) = &self.turn_off {
                enabled.store(false, Ordering::Release);
            }
            let mut calls = self.calls.lock().unwrap();
            calls.replaced.push(replaces);
            calls.last = Some(Received {
                body,
                app,
                icon,
                summary,
                hints,
            });
            // A replacement keeps the ID of the notification it replaces.
            if replaces == 0 {
                calls.issued += 7;
                calls.issued
            } else {
                replaces
            }
        }

        fn close_notification(&self, id: u32) {
            self.calls.lock().unwrap().closed.push(id);
        }
    }

    /// A connection to `service`, as to the session's notification server.
    async fn connect(service: Service) -> (zbus::Connection, zbus::Connection) {
        let (client, server) = zbus::connection::socket::Channel::pair();
        let guid = zbus::Guid::generate();
        let server = zbus::connection::Builder::authenticated_socket(server, guid.clone())
            .unwrap()
            .p2p()
            .serve_at("/org/freedesktop/Notifications", service)
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
        (client, server)
    }

    #[tokio::test]
    async fn dbus_notification_has_native_identity_and_literal_body() {
        let calls = Arc::new(Mutex::new(Calls::default()));
        let (client, _server) = connect(Service {
            calls: calls.clone(),
            ..Service::default()
        })
        .await;
        assert_eq!(
            notify(
                &client,
                "Alice & Bob",
                "<img src='https://example.com'> & hello",
                0
            )
            .await
            .unwrap(),
            7
        );
        let calls = calls.lock().unwrap();
        let received = calls.last.as_ref().unwrap();
        assert_eq!(received.app, "Ruston Mail");
        assert_eq!(received.icon, crate::ui::identity::APP_ID);
        assert_eq!(received.summary, "Alice & Bob");
        assert_eq!(
            received.body,
            "&lt;img src='https://example.com'&gt; &amp; hello"
        );
        assert_eq!(
            <&str>::try_from(&received.hints["desktop-entry"]).unwrap(),
            crate::ui::identity::APP_ID
        );
        assert_eq!(
            <&str>::try_from(&received.hints["category"]).unwrap(),
            "email.arrived"
        );
    }

    #[tokio::test]
    async fn each_notification_replaces_the_last_and_closing_takes_it_away() {
        let calls = Arc::new(Mutex::new(Calls::default()));
        let (client, _server) = connect(Service {
            calls: calls.clone(),
            ..Service::default()
        })
        .await;
        let (enabled, shown) = (AtomicBool::new(true), Shown::default());

        for subject in ["First", "Second"] {
            assert!(
                show_on(&client, "Ana", subject, &enabled, &shown)
                    .await
                    .unwrap()
            );
        }
        close_on(&client, &shown).await.unwrap();
        // Nothing is left to take away.
        close_on(&client, &shown).await.unwrap();
        enabled.store(false, Ordering::Release);
        assert!(
            !show_on(&client, "Ana", "Third", &enabled, &shown)
                .await
                .unwrap()
        );

        let calls = calls.lock().unwrap();
        assert_eq!(calls.replaced, [0, 7]);
        assert_eq!(calls.closed, [7]);
    }

    #[tokio::test]
    async fn a_notification_that_arrives_as_they_are_turned_off_is_taken_away() {
        let calls = Arc::new(Mutex::new(Calls::default()));
        let enabled = Arc::new(AtomicBool::new(true));
        let (client, _server) = connect(Service {
            calls: calls.clone(),
            turn_off: Some(enabled.clone()),
        })
        .await;
        let shown = Shown::default();

        assert!(
            !show_on(&client, "Ana", "Late", &enabled, &shown)
                .await
                .unwrap()
        );

        assert_eq!(calls.lock().unwrap().closed, [7]);
        assert_eq!(shown.0.load(Ordering::Acquire), 0);
    }
}
