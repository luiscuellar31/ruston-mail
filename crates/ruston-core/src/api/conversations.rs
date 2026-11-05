//! Conversations: listing them, opening one, and acting on several at a time.

use super::events::LabelCount;
use super::messages::ListQuery;
use crate::error::Result;
use crate::model::conversation::Conversation;
use crate::model::message::Message;
use crate::transport::{Doer, Request};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct ListResp {
    #[serde(rename = "Total", default)]
    total: u32,
    #[serde(rename = "Conversations", default)]
    conversations: Vec<Conversation>,
}

#[derive(Deserialize)]
struct GetResp {
    #[serde(rename = "Conversation")]
    conversation: Conversation,
    #[serde(rename = "Messages", default)]
    messages: Vec<Message>,
}

/// One page of the conversations `q` asks for, and how many match in all.
pub async fn list_conversations<D: Doer>(d: &D, q: &ListQuery) -> Result<(u32, Vec<Conversation>)> {
    d.decode(q.apply(Request::get("/mail/v4/conversations")))
        .await
        .map(|page: ListResp| (page.total, page.conversations))
}

/// The conversation `id` and its messages.
pub async fn get_conversation<D: Doer>(d: &D, id: &str) -> Result<(Conversation, Vec<Message>)> {
    d.decode(Request::get(format!("/mail/v4/conversations/{id}")))
        .await
        .map(|opened: GetResp| (opened.conversation, opened.messages))
}

/// Applies `action` to the conversations whose `IDs` are in `body`, and
/// fails naming those Proton refused.
async fn act_on_each<D: Doer>(d: &D, action: &str, body: serde_json::Value) -> Result<()> {
    let request = Request::put(format!("/mail/v4/conversations/{action}")).json(body);
    d.decode::<super::EachResponse>(request)
        .await?
        .into_result()
}

/// Adds the label `label_id` to the conversations `ids`; a folder's ID moves
/// them into that folder.
pub async fn label<D: Doer>(d: &D, label_id: &str, ids: &[String]) -> Result<()> {
    act_on_each(d, "label", json!({ "LabelID": label_id, "IDs": ids })).await
}
/// Takes the label `label_id` off the conversations `ids`.
pub async fn unlabel<D: Doer>(d: &D, label_id: &str, ids: &[String]) -> Result<()> {
    act_on_each(d, "unlabel", json!({ "LabelID": label_id, "IDs": ids })).await
}

/// Marks the conversations `ids` read, or unread. Marking unread names the
/// folder `label_id` it is done in; marking read takes no folder.
pub async fn mark<D: Doer>(d: &D, read: bool, ids: &[String], label_id: &str) -> Result<()> {
    if read {
        act_on_each(d, "read", json!({ "IDs": ids })).await
    } else {
        act_on_each(d, "unread", json!({ "IDs": ids, "LabelID": label_id })).await
    }
}

/// Deletes for good what the conversations `ids` hold in the folder
/// `label_id`.
pub async fn delete<D: Doer>(d: &D, ids: &[String], label_id: &str) -> Result<()> {
    act_on_each(d, "delete", json!({ "IDs": ids, "LabelID": label_id })).await
}

/// Puts the conversations `ids` away until `snooze_time`, in Unix seconds.
pub async fn snooze<D: Doer>(d: &D, ids: &[String], snooze_time: i64) -> Result<()> {
    let until = json!({ "IDs": ids, "SnoozeTime": snooze_time });
    act_on_each(d, "snooze", until).await
}

/// Brings the snoozed conversations `ids` back now.
pub async fn unsnooze<D: Doer>(d: &D, ids: &[String]) -> Result<()> {
    act_on_each(d, "unsnooze", json!({ "IDs": ids })).await
}

/// How many conversations each label holds, and how many of them are unread.
pub async fn counts<D: Doer>(d: &D) -> Result<Vec<LabelCount>> {
    super::messages::label_counts(d, "/mail/v4/conversations/count").await
}
