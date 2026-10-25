//! Organizing: move / label / trash / delete / mark / star.

use super::Client;
use crate::api;
use crate::error::{Error, Result};
use crate::model::Label;
use crate::model::enums::{label_ids, label_type, resolve_folder};

impl Client {
    /// Move messages to a folder/label (accepts a folder name or label ID).
    pub async fn move_messages(&self, ids: &[String], folder: &str) -> Result<()> {
        api::messages::label(self.http(), &resolve_folder(folder), ids).await
    }
    /// Apply a label to messages.
    pub async fn apply_label(&self, ids: &[String], label_id: &str) -> Result<()> {
        api::messages::label(self.http(), label_id, ids).await
    }
    /// Remove a label from messages.
    pub async fn remove_label(&self, ids: &[String], label_id: &str) -> Result<()> {
        api::messages::unlabel(self.http(), label_id, ids).await
    }
    /// Move messages to Trash.
    pub async fn trash_messages(&self, ids: &[String]) -> Result<()> {
        api::messages::label(self.http(), label_ids::TRASH, ids).await
    }
    /// Permanently delete messages.
    pub async fn delete_messages(&self, ids: &[String]) -> Result<()> {
        api::messages::delete(self.http(), ids).await
    }
    /// Mark messages read or unread.
    pub async fn mark_messages_read(&self, ids: &[String], read: bool) -> Result<()> {
        if read {
            api::messages::mark_read(self.http(), ids).await
        } else {
            api::messages::mark_unread(self.http(), ids).await
        }
    }
    /// Star or unstar messages.
    pub async fn star_messages(&self, ids: &[String], starred: bool) -> Result<()> {
        if starred {
            api::messages::label(self.http(), label_ids::STARRED, ids).await
        } else {
            api::messages::unlabel(self.http(), label_ids::STARRED, ids).await
        }
    }

    // --- conversations ---
    /// Move conversations to a folder/label (accepts a folder name or label ID).
    pub async fn move_conversations(&self, ids: &[String], folder: &str) -> Result<()> {
        api::conversations::label(self.http(), &resolve_folder(folder), ids).await
    }
    /// Apply a label to conversations; Proton puts it on every message in them.
    pub async fn label_conversations(&self, ids: &[String], label_id: &str) -> Result<()> {
        api::conversations::label(self.http(), label_id, ids).await
    }
    /// Remove a label from conversations, and so from every message in them.
    pub async fn unlabel_conversations(&self, ids: &[String], label_id: &str) -> Result<()> {
        api::conversations::unlabel(self.http(), label_id, ids).await
    }
    /// Move conversations to Trash.
    pub async fn trash_conversations(&self, ids: &[String]) -> Result<()> {
        api::conversations::label(self.http(), label_ids::TRASH, ids).await
    }
    /// Permanently delete conversations from the given folder.
    pub async fn delete_conversations(&self, ids: &[String], folder: &str) -> Result<()> {
        api::conversations::delete(self.http(), ids, &resolve_folder(folder)).await
    }
    /// Mark conversations read or unread (unread is scoped to `folder`).
    pub async fn mark_conversations_read(
        &self,
        ids: &[String],
        read: bool,
        folder: &str,
    ) -> Result<()> {
        api::conversations::mark(self.http(), read, ids, &resolve_folder(folder)).await
    }
    /// Star or unstar conversations.
    pub async fn star_conversations(&self, ids: &[String], starred: bool) -> Result<()> {
        if starred {
            api::conversations::label(self.http(), label_ids::STARRED, ids).await
        } else {
            api::conversations::unlabel(self.http(), label_ids::STARRED, ids).await
        }
    }

    // --- labels / folders CRUD ---
    /// List the account's labels.
    pub async fn list_labels(&self) -> Result<Vec<Label>> {
        api::labels::list(self.http(), label_type::MESSAGE_LABEL).await
    }
    /// List the account's folders.
    pub async fn list_folders(&self) -> Result<Vec<Label>> {
        api::labels::list(self.http(), label_type::MESSAGE_FOLDER).await
    }

    /// The label ID `folder` names, for the operations that take a folder or
    /// a label, by the rule [`folder_id_among`] follows. Proton is asked for
    /// the account's folders and labels only when `folder` is neither a
    /// system folder nor shaped like a label ID.
    pub async fn folder_id(&self, folder: &str) -> Result<String> {
        if let Some(id) = known_label_id(folder) {
            return Ok(id);
        }
        let mut own = self.list_folders().await?;
        own.extend(self.list_labels().await?);
        folder_id_among(&own, folder)
    }
    /// Create a label, or a folder when `folder` is set, optionally nested under `parent`.
    pub async fn create_label(
        &self,
        name: &str,
        color: &str,
        folder: bool,
        parent: Option<&str>,
    ) -> Result<Label> {
        let t = if folder {
            label_type::MESSAGE_FOLDER
        } else {
            label_type::MESSAGE_LABEL
        };
        api::labels::create(self.http(), name, color, t, parent).await
    }
    /// Delete labels or folders by ID.
    pub async fn delete_labels(&self, ids: &[String]) -> Result<()> {
        api::labels::delete(self.http(), ids).await
    }

    // --- mailbox actions ---
    /// Report messages as spam (move to the Spam folder).
    pub async fn report_spam(&self, ids: &[String]) -> Result<()> {
        api::messages::label(self.http(), label_ids::SPAM, ids).await
    }
    /// Report messages as not-spam (trains the filter; server moves to Inbox).
    pub async fn report_ham(&self, ids: &[String]) -> Result<()> {
        for id in ids {
            api::messages::mark_ham(self.http(), id).await?;
        }
        Ok(())
    }
    /// One-click unsubscribe from a mailing list.
    pub async fn unsubscribe(&self, id: &str) -> Result<()> {
        api::messages::unsubscribe(self.http(), id).await
    }
    /// Permanently empty a folder (e.g. `trash`, `spam`).
    pub async fn empty_folder(&self, folder: &str) -> Result<()> {
        api::messages::empty(self.http(), &resolve_folder(folder)).await
    }
    /// Snooze conversations until a Unix timestamp.
    pub async fn snooze_conversations(&self, ids: &[String], until: i64) -> Result<()> {
        api::conversations::snooze(self.http(), ids, until).await
    }
    /// Unsnooze conversations.
    pub async fn unsnooze_conversations(&self, ids: &[String]) -> Result<()> {
        api::conversations::unsnooze(self.http(), ids).await
    }

    /// Fetch the account's mail settings (raw).
    pub async fn mail_settings(&self) -> Result<serde_json::Value> {
        api::settings::get_mail_settings(self.http()).await
    }

    /// Per-label message counts (total + unread).
    pub async fn message_counts(&self) -> Result<Vec<api::events::LabelCount>> {
        api::messages::counts(self.http()).await
    }
    /// Per-label conversation counts.
    pub async fn conversation_counts(&self) -> Result<Vec<api::events::LabelCount>> {
        api::conversations::counts(self.http()).await
    }
    /// Restore permanently-deleted messages.
    pub async fn undelete_messages(&self, ids: &[String]) -> Result<()> {
        api::messages::undelete(self.http(), ids).await
    }
    /// Rename / recolor / reparent a label or folder.
    pub async fn update_label(
        &self,
        id: &str,
        name: Option<&str>,
        color: Option<&str>,
        parent: Option<&str>,
    ) -> Result<()> {
        let mut body = serde_json::Map::new();
        if let Some(n) = name {
            body.insert("Name".into(), serde_json::json!(n));
        }
        if let Some(c) = color {
            body.insert("Color".into(), serde_json::json!(c));
        }
        if let Some(p) = parent {
            body.insert("ParentID".into(), serde_json::json!(p));
        }
        api::labels::update(self.http(), id, serde_json::Value::Object(body)).await
    }

    /// Send a read receipt for a message.
    pub async fn send_read_receipt(&self, id: &str) -> Result<()> {
        api::messages::send_receipt(self.http(), id).await
    }
    /// Toggle signing outgoing mail.
    pub async fn set_sign(&self, on: bool) -> Result<()> {
        api::settings::set_sign(self.http(), on).await
    }
    /// Toggle attaching the public key to outgoing mail.
    pub async fn set_attach_public_key(&self, on: bool) -> Result<()> {
        api::settings::set_attach_public_key(self.http(), on).await
    }
    /// Update an address (display name / signature).
    pub async fn update_address(
        &self,
        id: &str,
        display_name: Option<&str>,
        signature: Option<&str>,
    ) -> Result<()> {
        api::keys::update_address(self.http(), id, display_name, signature).await
    }
}

/// The label ID `folder` names, given `own`, the account's folders and
/// labels. A system folder's name or ID, and anything shaped like a label ID,
/// are taken as [`resolve_folder`] takes them. Anything else is the path, or
/// else the name, of one of `own`, ignoring case: a name that several share
/// is [`Error::Ambiguous`], and one that none has, [`Error::NotFound`].
pub fn folder_id_among(own: &[Label], folder: &str) -> Result<String> {
    match known_label_id(folder) {
        Some(id) => Ok(id),
        None => own_label_id(own, &resolve_folder(folder)),
    }
}

/// What `folder` names without a look at the account's own folders: a
/// system folder, or a label by its ID.
fn known_label_id(folder: &str) -> Option<String> {
    let resolved = resolve_folder(folder);
    (label_ids::name(&resolved).is_some() || looks_like_label_id(&resolved)).then_some(resolved)
}

/// Whether `text` is shaped like the ID Proton gives a label: base64 with
/// its padding, which no name a person types looks like.
fn looks_like_label_id(text: &str) -> bool {
    let digits = text.trim_end_matches('=');
    text.len() >= 16
        && text.ends_with("==")
        && digits
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'+' | b'/'))
}

/// The ID of the label in `own` whose path, or else whose name, is `wanted`,
/// ignoring case.
fn own_label_id(own: &[Label], wanted: &str) -> Result<String> {
    let wanted = wanted.to_lowercase();
    let named = |key: fn(&Label) -> &str| -> Vec<&Label> {
        own.iter()
            .filter(|label| key(label).to_lowercase() == wanted)
            .collect()
    };
    let mut found = named(|label| &label.path);
    if found.is_empty() {
        found = named(|label| &label.name);
    }
    match found.as_slice() {
        [label] => Ok(label.id.clone()),
        [] => Err(Error::NotFound {
            kind: "folder or label".into(),
        }),
        several => Err(Error::Ambiguous(several.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::test_support;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn label(id: &str, name: &str, path: &str) -> Label {
        Label {
            id: id.into(),
            name: name.into(),
            path: path.into(),
            ..Label::default()
        }
    }

    #[test]
    fn an_own_folder_is_found_by_its_path_or_its_name_ignoring_case() {
        let own = [
            label("taxes", "Impuestos", "Impuestos"),
            label("taxes-2024", "2024", "Impuestos/2024"),
            label("travel-2024", "2024", "Viajes/2024"),
            label("work", "Trabajo", "Trabajo"),
        ];
        assert_eq!(folder_id_among(&own, "trabajo").unwrap(), "work");
        assert_eq!(
            folder_id_among(&own, "IMPUESTOS/2024").unwrap(),
            "taxes-2024"
        );
        assert!(matches!(
            folder_id_among(&own, "2024"),
            Err(Error::Ambiguous(2))
        ));
        assert!(matches!(
            folder_id_among(&own, "Recibos"),
            Err(Error::NotFound { .. })
        ));
        // A system folder is never one of the account's own.
        assert_eq!(
            folder_id_among(&own, "Archive").unwrap(),
            label_ids::ARCHIVE
        );
    }

    #[test]
    fn label_ids_are_told_from_names() {
        assert!(looks_like_label_id("qBIcv1_Wv5X4hLpEo0Tz9A=="));
        assert!(looks_like_label_id(&format!("{}==", "aB3-_".repeat(17))));
        for name in ["Work", "Impuestos/2024", "a==", "My folder==", "2024"] {
            assert!(!looks_like_label_id(name), "{name}");
        }
    }

    #[tokio::test]
    async fn only_a_name_proton_has_to_be_asked_about_lists_folders_and_labels() {
        let server = MockServer::start().await;
        for (kind, labels) in [
            (
                label_type::MESSAGE_FOLDER,
                serde_json::json!([{"ID": "work-id", "Name": "Work", "Path": "Work"}]),
            ),
            (
                label_type::MESSAGE_LABEL,
                serde_json::json!([{"ID": "late-id", "Name": "Later", "Path": "Later"}]),
            ),
        ] {
            Mock::given(method("GET"))
                .and(path("/core/v4/labels"))
                .and(query_param("Type", kind.to_string()))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({"Code": 1000, "Labels": labels})),
                )
                .expect(1)
                .mount(&server)
                .await;
        }
        let client = test_support::client(&server, Vec::new());

        // Neither a system folder nor an ID is looked up.
        assert_eq!(client.folder_id("Inbox").await.unwrap(), label_ids::INBOX);
        assert_eq!(client.folder_id("5").await.unwrap(), label_ids::ALL_MAIL);
        let id = "qBIcv1_Wv5X4hLpEo0Tz9A==";
        assert_eq!(client.folder_id(id).await.unwrap(), id);
        // A name is, once, among folders and labels alike.
        assert_eq!(client.folder_id(" later ").await.unwrap(), "late-id");
    }
}
