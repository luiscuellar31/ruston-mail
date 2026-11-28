//! Whether a Linux desktop asks for less motion, as its settings portal
//! tells: `reduced-motion` where the portal has that key, and otherwise
//! GNOME's `enable-animations`, which older portals pass on.

use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt;
use zbus::zvariant::{OwnedValue, Value};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
/// The portal's own setting, a number: 1 asks for less motion.
const REDUCED_MOTION: (&str, &str) = ("org.freedesktop.appearance", "reduced-motion");
/// GNOME's, a boolean: off asks for less motion.
const ANIMATIONS: (&str, &str) = ("org.gnome.desktop.interface", "enable-animations");

/// Whether the desktop asks for less motion. The first call starts a thread
/// that asks the portal and then follows its changes, so the interface never
/// waits for the session bus; until the portal answers, and where there is
/// none, the answer is no.
#[cfg(target_os = "linux")]
pub(super) fn asked() -> bool {
    use std::sync::{Arc, OnceLock};

    static REDUCED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    REDUCED
        .get_or_init(|| {
            let reduced = Arc::new(AtomicBool::new(false));
            let told = reduced.clone();
            let followed = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map(|runtime| {
                    std::thread::Builder::new()
                        .name("ruston-reduced-motion".into())
                        .spawn(move || {
                            runtime.block_on(async move {
                                if let Ok(connection) = zbus::Connection::session().await {
                                    let _ = follow(&connection, &told).await;
                                }
                            });
                        })
                });
            // Without the thread nothing is asked, and the answer stays no.
            drop(followed);
            reduced
        })
        .load(Ordering::Relaxed)
}

/// Keeps `reduced` as the portal on `connection` has it, for as long as the
/// connection lasts.
async fn follow(connection: &zbus::Connection, reduced: &AtomicBool) -> zbus::Result<()> {
    let settings = zbus::Proxy::new(connection, PORTAL, PATH, SETTINGS).await?;
    // Listening starts before the first reading, so no change falls between.
    let mut changes = settings.receive_signal("SettingChanged").await?;
    reduced.store(read(&settings).await, Ordering::Relaxed);
    while let Some(change) = changes.next().await {
        let body = change.body();
        let Ok((namespace, key, _)) = body.deserialize::<(&str, &str, Value<'_>)>() else {
            continue;
        };
        if [REDUCED_MOTION, ANIMATIONS].contains(&(namespace, key)) {
            reduced.store(read(&settings).await, Ordering::Relaxed);
        }
    }
    Ok(())
}

/// What the portal says now: its own setting when it has one, and GNOME's
/// otherwise.
async fn read(settings: &zbus::Proxy<'_>) -> bool {
    match setting(settings, REDUCED_MOTION).await {
        Some(Value::U32(preference)) => preference == 1,
        _ => matches!(
            setting(settings, ANIMATIONS).await,
            Some(Value::Bool(false))
        ),
    }
}

/// One setting's value, or `None` when the portal has none by that name.
/// Portals before version 2 only have `Read`, which wraps the value twice.
async fn setting(
    settings: &zbus::Proxy<'_>,
    (namespace, key): (&str, &str),
) -> Option<Value<'static>> {
    let mut value: OwnedValue = match settings.call("ReadOne", &(namespace, key)).await {
        Ok(value) => value,
        Err(_) => settings.call("Read", &(namespace, key)).await.ok()?,
    };
    while let Value::Value(inner) = &*value {
        value = inner.try_to_owned().ok()?;
    }
    Some(value.into())
}

#[cfg(test)]
mod tests;
