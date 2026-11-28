use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zbus::object_server::SignalEmitter;

use super::*;

type Held = Arc<Mutex<HashMap<(&'static str, &'static str), OwnedValue>>>;

/// A settings portal that tells the settings it holds; one of `version` 1
/// has no `ReadOne`.
struct Portal {
    held: Held,
    version: u32,
}

impl Portal {
    fn value(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
        let held = self.held.lock().unwrap();
        held.iter()
            .find(|(name, _)| **name == (namespace, key))
            .and_then(|(_, value)| value.try_clone().ok())
            .ok_or_else(|| zbus::fdo::Error::Failed("no such setting".into()))
    }
}

#[zbus::interface(name = "org.freedesktop.portal.Settings")]
impl Portal {
    fn read_one(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
        if self.version < 2 {
            return Err(zbus::fdo::Error::UnknownMethod("ReadOne".into()));
        }
        self.value(namespace, key)
    }

    /// As the portal's first version did: the value inside a second variant.
    fn read(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
        let value = self.value(namespace, key)?;
        OwnedValue::try_from(Value::Value(Box::new(value.into())))
            .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
    }

    #[zbus(signal)]
    async fn setting_changed(
        emitter: &SignalEmitter<'_>,
        namespace: &str,
        key: &str,
        value: Value<'_>,
    ) -> zbus::Result<()>;
}

/// A connection to a portal of `version` that holds `settings`, with the
/// portal's own end and what it holds.
async fn portal(
    version: u32,
    settings: &[((&'static str, &'static str), Value<'static>)],
) -> (zbus::Connection, zbus::Connection, Held) {
    let held: Held = Arc::new(Mutex::new(
        settings
            .iter()
            .map(|(name, value)| (*name, OwnedValue::try_from(value.clone()).unwrap()))
            .collect(),
    ));
    let (client, server) = zbus::connection::socket::Channel::pair();
    let guid = zbus::Guid::generate();
    let server = zbus::connection::Builder::authenticated_socket(server, guid.clone())
        .unwrap()
        .p2p()
        .serve_at(
            PATH,
            Portal {
                held: held.clone(),
                version,
            },
        )
        .unwrap()
        .build();
    let client = zbus::connection::Builder::authenticated_socket(client, guid)
        .unwrap()
        .p2p()
        .build();
    let (server, client) = tokio::try_join!(server, client).unwrap();
    (client, server, held)
}

#[tokio::test]
async fn the_portal_s_setting_decides_and_gnome_s_stands_in_for_it() {
    let (reduce, animate) = (Value::U32(1), Value::Bool(true));
    for (version, settings, asked) in [
        (2, vec![], false),
        (2, vec![(REDUCED_MOTION, reduce.clone())], true),
        (2, vec![(REDUCED_MOTION, Value::U32(0))], false),
        // A value from a later version of the portal is no preference.
        (2, vec![(REDUCED_MOTION, Value::U32(7))], false),
        // The portal's own setting, when it has one, is the one that counts.
        (
            2,
            vec![
                (REDUCED_MOTION, Value::U32(0)),
                (ANIMATIONS, Value::Bool(false)),
            ],
            false,
        ),
        (2, vec![(ANIMATIONS, Value::Bool(false))], true),
        (2, vec![(ANIMATIONS, animate.clone())], false),
        // The first version of the portal wraps what it reads twice.
        (1, vec![(ANIMATIONS, Value::Bool(false))], true),
        (1, vec![(REDUCED_MOTION, reduce.clone())], true),
        (1, vec![(ANIMATIONS, animate.clone())], false),
    ] {
        let (client, _server, _) = portal(version, &settings).await;
        let proxy = zbus::Proxy::new(&client, PORTAL, PATH, SETTINGS)
            .await
            .unwrap();
        assert_eq!(read(&proxy).await, asked, "version {version}: {settings:?}");
    }
}

#[tokio::test]
async fn a_change_of_the_setting_is_followed() {
    let (client, server, held) = portal(2, &[(REDUCED_MOTION, Value::U32(0))]).await;
    let reduced = Arc::new(AtomicBool::new(false));
    let told = reduced.clone();
    let following = tokio::spawn(async move { follow(&client, &told).await });

    held.lock()
        .unwrap()
        .insert(REDUCED_MOTION, OwnedValue::from(1u32));
    let portal = server
        .object_server()
        .interface::<_, Portal>(PATH)
        .await
        .unwrap();
    // The signal is sent until the follower, once listening, has heard it.
    let heard = tokio::time::timeout(Duration::from_secs(5), async {
        while !reduced.load(Ordering::Relaxed) {
            let (namespace, key) = REDUCED_MOTION;
            Portal::setting_changed(portal.signal_emitter(), namespace, key, Value::U32(1))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    following.abort();
    assert!(heard.is_ok(), "the change was not followed");
}
