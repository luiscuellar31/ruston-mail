use std::collections::HashMap;

#[cfg(target_os = "linux")]
use super::Status;

#[cfg(target_os = "linux")]
pub(super) async fn show(
    sender: &str,
    subject: &str,
    enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Status {
    let request = async {
        let connection = zbus::Connection::session().await?;
        if !enabled.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(None);
        }
        notify(&connection, sender, subject).await.map(Some)
    };
    match tokio::time::timeout(std::time::Duration::from_secs(3), request).await {
        Ok(Ok(Some(_))) => Status::Ready,
        Ok(Ok(None)) => Status::Off,
        _ => Status::Unavailable,
    }
}

async fn notify(connection: &zbus::Connection, sender: &str, subject: &str) -> zbus::Result<u32> {
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
                0_u32,
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
    use std::sync::{Arc, Mutex};

    struct Received {
        body: String,
        app: String,
        icon: String,
        summary: String,
        hints: HashMap<String, zbus::zvariant::OwnedValue>,
    }
    struct Service(Arc<Mutex<Option<Received>>>);

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
            assert_eq!(replaces, 0);
            assert!(actions.is_empty());
            assert_eq!(timeout, -1);
            *self.0.lock().unwrap() = Some(Received {
                body,
                app,
                icon,
                summary,
                hints,
            });
            7
        }
    }

    #[tokio::test]
    async fn dbus_notification_has_native_identity_and_literal_body() {
        let received = Arc::new(Mutex::new(None));
        let (client, server) = zbus::connection::socket::Channel::pair();
        let guid = zbus::Guid::generate();
        let _server = zbus::connection::Builder::authenticated_socket(server, guid.clone())
            .unwrap()
            .p2p()
            .serve_at("/org/freedesktop/Notifications", Service(received.clone()))
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
        assert_eq!(
            notify(
                &client,
                "Alice & Bob",
                "<img src='https://example.com'> & hello"
            )
            .await
            .unwrap(),
            7
        );
        let received = received.lock().unwrap();
        let received = received.as_ref().unwrap();
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
}
