//! Organizing mail: moving, labelling, starring, marking, trashing and
//! deleting messages and conversations; the account's folders and labels;
//! and the settings of its addresses and outgoing mail.

use super::Client;
use crate::api;
use crate::error::{Error, Result};
use crate::model::Label;
use crate::model::enums::{label_ids, label_type, resolve_folder};

/// What an action changes: single messages, or whole conversations, which
/// Proton changes by changing every message in them. Moving to a folder,
/// trashing, reporting spam and starring all put a label on.
#[derive(Clone, Copy)]
enum Items {
    Messages,
    Conversations,
}

impl Client {
    /// Puts the label `label_id` on the `ids` of `items`.
    async fn put_label(&self, items: Items, label_id: &str, ids: &[String]) -> Result<()> {
        match items {
            Items::Messages => api::messages::label(self.http(), label_id, ids).await,
            Items::Conversations => api::conversations::label(self.http(), label_id, ids).await,
        }
    }

    /// Takes the label `label_id` off the `ids` of `items`.
    async fn take_label(&self, items: Items, label_id: &str, ids: &[String]) -> Result<()> {
        match items {
            Items::Messages => api::messages::unlabel(self.http(), label_id, ids).await,
            Items::Conversations => api::conversations::unlabel(self.http(), label_id, ids).await,
        }
    }

    /// Stars the `ids` of `items`, or takes their star off.
    async fn star(&self, items: Items, ids: &[String], starred: bool) -> Result<()> {
        if starred {
            self.put_label(items, label_ids::STARRED, ids).await
        } else {
            self.take_label(items, label_ids::STARRED, ids).await
        }
    }
}

/// Actions on messages.
impl Client {
    /// Moves messages to `folder`: a system folder by the name
    /// [`resolve_folder`] reads, or a folder or label by its ID.
    pub async fn move_messages(&self, ids: &[String], folder: &str) -> Result<()> {
        self.put_label(Items::Messages, &resolve_folder(folder), ids)
            .await
    }

    /// Puts the label `label_id` on messages.
    pub async fn apply_label(&self, ids: &[String], label_id: &str) -> Result<()> {
        self.put_label(Items::Messages, label_id, ids).await
    }

    /// Takes the label `label_id` off messages.
    pub async fn remove_label(&self, ids: &[String], label_id: &str) -> Result<()> {
        self.take_label(Items::Messages, label_id, ids).await
    }

    /// Stars messages, or takes their star off.
    pub async fn star_messages(&self, ids: &[String], starred: bool) -> Result<()> {
        self.star(Items::Messages, ids, starred).await
    }

    /// Moves messages to Trash.
    pub async fn trash_messages(&self, ids: &[String]) -> Result<()> {
        self.put_label(Items::Messages, label_ids::TRASH, ids).await
    }

    /// Reports messages as spam, which moves them to Spam.
    pub async fn report_spam(&self, ids: &[String]) -> Result<()> {
        self.put_label(Items::Messages, label_ids::SPAM, ids).await
    }

    /// Reports messages as not spam, one request for each, stopping at the
    /// first that fails.
    pub async fn report_ham(&self, ids: &[String]) -> Result<()> {
        for id in ids {
            api::messages::mark_ham(self.http(), id).await?;
        }
        Ok(())
    }

    /// Marks messages read, or unread when `read` is false.
    pub async fn mark_messages_read(&self, ids: &[String], read: bool) -> Result<()> {
        if read {
            api::messages::mark_read(self.http(), ids).await
        } else {
            api::messages::mark_unread(self.http(), ids).await
        }
    }

    /// Deletes messages permanently.
    pub async fn delete_messages(&self, ids: &[String]) -> Result<()> {
        api::messages::delete(self.http(), ids).await
    }

    /// Brings back messages that were deleted permanently.
    pub async fn undelete_messages(&self, ids: &[String]) -> Result<()> {
        api::messages::undelete(self.http(), ids).await
    }

    /// Deletes every message in `folder` permanently, as emptying Trash or
    /// Spam does.
    pub async fn empty_folder(&self, folder: &str) -> Result<()> {
        api::messages::empty(self.http(), &resolve_folder(folder)).await
    }

    /// Sends the read receipt that message `id` asks for.
    pub async fn send_read_receipt(&self, id: &str) -> Result<()> {
        api::messages::send_receipt(self.http(), id).await
    }

    /// Unsubscribes from the mailing list that sent message `id`, in the one
    /// step the list offers for it.
    pub async fn unsubscribe(&self, id: &str) -> Result<()> {
        api::messages::unsubscribe(self.http(), id).await
    }

    /// How many messages each label holds, and how many of them are unread.
    pub async fn message_counts(&self) -> Result<Vec<api::events::LabelCount>> {
        api::messages::counts(self.http()).await
    }
}

/// Actions on whole conversations.
impl Client {
    /// Moves conversations to `folder`, named as for
    /// [`Client::move_messages`].
    pub async fn move_conversations(&self, ids: &[String], folder: &str) -> Result<()> {
        self.put_label(Items::Conversations, &resolve_folder(folder), ids)
            .await
    }

    /// Apply a label to conversations; Proton puts it on every message in them.
    pub async fn label_conversations(&self, ids: &[String], label_id: &str) -> Result<()> {
        self.put_label(Items::Conversations, label_id, ids).await
    }

    /// Remove a label from conversations, and so from every message in them.
    pub async fn unlabel_conversations(&self, ids: &[String], label_id: &str) -> Result<()> {
        self.take_label(Items::Conversations, label_id, ids).await
    }

    /// Stars conversations, or takes their star off.
    pub async fn star_conversations(&self, ids: &[String], starred: bool) -> Result<()> {
        self.star(Items::Conversations, ids, starred).await
    }

    /// Moves conversations to Trash.
    pub async fn trash_conversations(&self, ids: &[String]) -> Result<()> {
        self.put_label(Items::Conversations, label_ids::TRASH, ids)
            .await
    }

    /// Marks conversations read, or unread when `read` is false. Marking
    /// them unread tells Proton the folder they are in, `folder`.
    pub async fn mark_conversations_read(
        &self,
        ids: &[String],
        read: bool,
        folder: &str,
    ) -> Result<()> {
        api::conversations::mark(self.http(), read, ids, &resolve_folder(folder)).await
    }

    /// Deletes conversations permanently from `folder`.
    pub async fn delete_conversations(&self, ids: &[String], folder: &str) -> Result<()> {
        api::conversations::delete(self.http(), ids, &resolve_folder(folder)).await
    }

    /// Snoozes conversations until `until`, a Unix time.
    pub async fn snooze_conversations(&self, ids: &[String], until: i64) -> Result<()> {
        api::conversations::snooze(self.http(), ids, until).await
    }

    /// Wakes snoozed conversations before their time.
    pub async fn unsnooze_conversations(&self, ids: &[String]) -> Result<()> {
        api::conversations::unsnooze(self.http(), ids).await
    }

    /// How many conversations each label holds, and how many of them are
    /// unread.
    pub async fn conversation_counts(&self) -> Result<Vec<api::events::LabelCount>> {
        api::conversations::counts(self.http()).await
    }
}

/// The account's folders and labels.
impl Client {
    /// The account's labels.
    pub async fn list_labels(&self) -> Result<Vec<Label>> {
        api::labels::list(self.http(), label_type::MESSAGE_LABEL).await
    }

    /// The account's folders.
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

    /// Makes a label named `name` in `color`, or with `folder` a folder,
    /// inside `parent` when one is given.
    pub async fn create_label(
        &self,
        name: &str,
        color: &str,
        folder: bool,
        parent: Option<&str>,
    ) -> Result<Label> {
        let kind = if folder {
            label_type::MESSAGE_FOLDER
        } else {
            label_type::MESSAGE_LABEL
        };
        api::labels::create(self.http(), name, color, kind, parent).await
    }

    /// Renames, recolors or moves a label or folder. What is `None` stays as
    /// it is.
    pub async fn update_label(
        &self,
        id: &str,
        name: Option<&str>,
        color: Option<&str>,
        parent: Option<&str>,
    ) -> Result<()> {
        let changes: serde_json::Map<String, serde_json::Value> =
            [("Name", name), ("Color", color), ("ParentID", parent)]
                .into_iter()
                .filter_map(|(field, value)| Some((field.to_owned(), value?.into())))
                .collect();
        api::labels::update(self.http(), id, changes.into()).await
    }

    /// Deletes labels or folders by ID.
    pub async fn delete_labels(&self, ids: &[String]) -> Result<()> {
        api::labels::delete(self.http(), ids).await
    }
}

/// The settings of the account's addresses and outgoing mail.
impl Client {
    /// Changes an address's display name or signature. What is `None` stays
    /// as it is.
    pub async fn update_address(
        &self,
        id: &str,
        display_name: Option<&str>,
        signature: Option<&str>,
    ) -> Result<()> {
        api::keys::update_address(self.http(), id, display_name, signature).await
    }

    /// The account's mail settings, as Proton sends them.
    pub async fn mail_settings(&self) -> Result<serde_json::Value> {
        api::settings::get_mail_settings(self.http()).await
    }

    /// Turns signing outgoing mail on or off.
    pub async fn set_sign(&self, on: bool) -> Result<()> {
        api::settings::set_sign(self.http(), on).await
    }

    /// Turns attaching the account's public key to outgoing mail on or off.
    pub async fn set_attach_public_key(&self, on: bool) -> Result<()> {
        api::settings::set_attach_public_key(self.http(), on).await
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
    use wiremock::matchers::{body_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Has `server` expect one PUT of `body` to `endpoint`, and succeed.
    async fn expect_put(server: &MockServer, endpoint: &str, body: serde_json::Value) {
        Mock::given(method("PUT"))
            .and(path(endpoint))
            .and(body_json(body))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .expect(1)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn each_labelling_action_puts_on_or_takes_off_the_label_it_means() {
        let server = MockServer::start().await;
        for (endpoint, label) in [
            ("/mail/v4/messages/label", "6"),
            ("/mail/v4/messages/label", "own"),
            ("/mail/v4/messages/label", "10"),
            ("/mail/v4/messages/label", "3"),
            ("/mail/v4/messages/label", "4"),
            ("/mail/v4/messages/unlabel", "own"),
            ("/mail/v4/messages/unlabel", "10"),
            ("/mail/v4/conversations/label", "0"),
            ("/mail/v4/conversations/label", "own"),
            ("/mail/v4/conversations/label", "10"),
            ("/mail/v4/conversations/label", "3"),
            ("/mail/v4/conversations/unlabel", "own"),
            ("/mail/v4/conversations/unlabel", "10"),
        ] {
            let body = serde_json::json!({"LabelID": label, "IDs": ["id-1"]});
            expect_put(&server, endpoint, body).await;
        }
        let client = test_support::client(&server, Vec::new());
        let ids = ["id-1".to_string()];

        client.move_messages(&ids, " Archive ").await.unwrap();
        client.apply_label(&ids, "own").await.unwrap();
        client.star_messages(&ids, true).await.unwrap();
        client.trash_messages(&ids).await.unwrap();
        client.report_spam(&ids).await.unwrap();
        client.remove_label(&ids, "own").await.unwrap();
        client.star_messages(&ids, false).await.unwrap();
        client.move_conversations(&ids, "inbox").await.unwrap();
        client.label_conversations(&ids, "own").await.unwrap();
        client.star_conversations(&ids, true).await.unwrap();
        client.trash_conversations(&ids).await.unwrap();
        client.unlabel_conversations(&ids, "own").await.unwrap();
        client.star_conversations(&ids, false).await.unwrap();
    }

    #[tokio::test]
    async fn a_label_update_sends_only_what_changes() {
        let server = MockServer::start().await;
        let renamed = serde_json::json!({"Name": "Taxes", "ParentID": "folder-0"});
        expect_put(&server, "/core/v4/labels/label-1", renamed).await;
        let recolored = serde_json::json!({"Color": "#FF0000"});
        expect_put(&server, "/core/v4/labels/label-2", recolored).await;
        let client = test_support::client(&server, Vec::new());

        client
            .update_label("label-1", Some("Taxes"), None, Some("folder-0"))
            .await
            .unwrap();
        client
            .update_label("label-2", None, Some("#FF0000"), None)
            .await
            .unwrap();
    }

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
