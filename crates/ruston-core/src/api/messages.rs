//! Messages: listing and searching them, writing and sending drafts, and
//! acting on one or many at a time.

use super::events::LabelCount;
use crate::error::Result;
use crate::model::message::{Message, MessageMetadata};
use crate::transport::{Doer, Request};
use reqwest::Method;
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::json;

/// What to list or search for, and which page of it. Each field that is set
/// becomes one query parameter.
#[derive(Debug, Default, Clone)]
pub struct ListQuery {
    /// The folder or label to list, by its ID.
    pub label_id: Option<String>,
    /// The page to return, counting from 0.
    pub page: Option<u32>,
    /// How many items a page holds.
    pub page_size: Option<u32>,
    /// Whether to list unread items only.
    pub unread: bool,
    /// Words to search for.
    pub keyword: Option<String>,
    /// The sender to match.
    pub from: Option<String>,
    /// The recipient to match.
    pub to: Option<String>,
    /// The subject to match.
    pub subject: Option<String>,
    /// The earliest time to include, in Unix seconds.
    pub begin: Option<i64>,
    /// The latest time to include, in Unix seconds.
    pub end: Option<i64>,
    /// The account address to list for, by its ID.
    pub address_id: Option<String>,
}

impl ListQuery {
    /// One page of what is labelled `label_id`, its messages or its
    /// conversations.
    pub fn folder_page(label_id: String, page: u32, page_size: u32) -> Self {
        Self {
            label_id: Some(label_id),
            page: Some(page),
            page_size: Some(page_size),
            ..Self::default()
        }
    }

    /// `req` with this query's parameters added, newest items first. Both
    /// the messages and the conversations endpoint take the recipient filter
    /// as `Recipients`, as Proton's web app sends it.
    pub fn apply(&self, req: Request) -> Request {
        let filters = [
            ("LabelID", self.label_id.clone()),
            ("Page", self.page.map(|page| page.to_string())),
            ("PageSize", self.page_size.map(|size| size.to_string())),
            ("Unread", self.unread.then(|| "1".to_owned())),
            ("Keyword", self.keyword.clone()),
            ("From", self.from.clone()),
            ("Recipients", self.to.clone()),
            ("Subject", self.subject.clone()),
            ("Begin", self.begin.map(|time| time.to_string())),
            ("End", self.end.map(|time| time.to_string())),
            ("AddressID", self.address_id.clone()),
        ];
        let newest_first = req.query("Sort", "Time").query("Desc", "1");
        filters
            .into_iter()
            .filter_map(|(name, value)| Some((name, value?)))
            .fold(newest_first, |req, (name, value)| req.query(name, value))
    }
}

#[derive(Deserialize)]
struct ListResp {
    #[serde(rename = "Total", default)]
    total: u32,
    #[serde(rename = "Messages", default)]
    messages: Vec<MessageMetadata>,
}

/// The answer to fetching, creating or saving a message.
#[derive(Deserialize)]
struct MessageResp {
    #[serde(rename = "Message")]
    message: Message,
}

/// The answer of both `count` endpoints, the messages' and the
/// conversations'.
#[derive(Deserialize)]
struct CountsResp {
    #[serde(rename = "Counts", default)]
    counts: Vec<LabelCount>,
}

/// Whether a page of `listed` items is a listing's last: Proton returns a
/// short page only at the end.
pub(crate) fn is_last_page(listed: usize, page_size: u32) -> bool {
    (listed as u32) < page_size
}

/// One page of the messages `q` asks for, and how many match in all.
pub async fn list_messages<D: Doer>(d: &D, q: &ListQuery) -> Result<(u32, Vec<MessageMetadata>)> {
    d.decode(q.apply(Request::get("/mail/v4/messages")))
        .await
        .map(|page: ListResp| (page.total, page.messages))
}

/// The message `id` whole, with its body and its attachments' details.
pub async fn get_message<D: Doer>(d: &D, id: &str) -> Result<Message> {
    whole_message(d, Request::get(format!("/mail/v4/messages/{id}"))).await
}

/// Creates a draft from `body`: its `Message`, with `ParentID` and `Action`
/// when it answers or forwards another, and `AttachmentKeyPackets` for the
/// attachments it takes from there.
pub async fn create_draft<D: Doer>(d: &D, body: serde_json::Value) -> Result<Message> {
    whole_message(d, Request::post("/mail/v4/messages").json(body)).await
}

/// Saves the draft `id` from `body`: its `Message`, with
/// `AttachmentKeyPackets` when there are attachment keys to give.
pub async fn update_draft<D: Doer>(d: &D, id: &str, body: serde_json::Value) -> Result<Message> {
    let request = Request::put(format!("/mail/v4/messages/{id}")).json(body);
    whole_message(d, request).await
}

/// Sends the draft `id`. `body` holds the `Packages` its recipients get, and
/// how it is sent, such as `ExpirationTime` and `AutoSaveContacts`.
pub async fn send_message<D: Doer>(d: &D, id: &str, body: serde_json::Value) -> Result<()> {
    let request = Request::post(format!("/mail/v4/messages/{id}")).json(body);
    act(d, request).await
}

/// Sends `request` and returns the message Proton answers with.
async fn whole_message<D: Doer>(d: &D, request: Request) -> Result<Message> {
    let MessageResp { message } = d.decode(request).await?;
    Ok(message)
}

/// Sends `request` for its effect alone: the envelope says whether Proton
/// did it, and the rest of the answer is skipped.
async fn act<D: Doer>(d: &D, request: Request) -> Result<()> {
    d.decode::<IgnoredAny>(request).await?;
    Ok(())
}

/// Sends `method` to the endpoint `action` of the message `id`, for its
/// effect alone.
async fn act_on<D: Doer>(d: &D, method: Method, id: &str, action: &str) -> Result<()> {
    act(
        d,
        Request::new(method, format!("/mail/v4/messages/{id}/{action}")),
    )
    .await
}

/// Applies `action` to the messages whose `IDs` are in `body`, and fails
/// naming those Proton refused.
async fn act_on_each<D: Doer>(d: &D, action: &str, body: serde_json::Value) -> Result<()> {
    let request = Request::put(format!("/mail/v4/messages/{action}")).json(body);
    d.decode::<super::EachResponse>(request)
        .await?
        .into_result()
}

/// The counts the `count` endpoint at `path` gives: messages', or
/// conversations'.
pub(super) async fn label_counts<D: Doer>(d: &D, path: &str) -> Result<Vec<LabelCount>> {
    let CountsResp { counts } = d.decode(Request::get(path)).await?;
    Ok(counts)
}

/// Adds the label `label_id` to the messages `ids`; a folder's ID moves them
/// into that folder.
pub async fn label<D: Doer>(d: &D, label_id: &str, ids: &[String]) -> Result<()> {
    act_on_each(d, "label", json!({ "LabelID": label_id, "IDs": ids })).await
}
/// Takes the label `label_id` off the messages `ids`.
pub async fn unlabel<D: Doer>(d: &D, label_id: &str, ids: &[String]) -> Result<()> {
    act_on_each(d, "unlabel", json!({ "LabelID": label_id, "IDs": ids })).await
}

/// How many messages each label holds, and how many of them are unread.
pub async fn counts<D: Doer>(d: &D) -> Result<Vec<LabelCount>> {
    label_counts(d, "/mail/v4/messages/count").await
}

/// Restores the deleted messages `ids`.
pub async fn undelete<D: Doer>(d: &D, ids: &[String]) -> Result<()> {
    act_on_each(d, "undelete", json!({ "IDs": ids })).await
}

/// Tells the sender of the message `id`, who asked to know, that it was read.
pub async fn send_receipt<D: Doer>(d: &D, id: &str) -> Result<()> {
    act_on(d, Method::POST, id, "receipt").await
}

/// Marks the messages `ids` read.
pub async fn mark_read<D: Doer>(d: &D, ids: &[String]) -> Result<()> {
    act_on_each(d, "read", json!({ "IDs": ids })).await
}
/// Marks the messages `ids` unread.
pub async fn mark_unread<D: Doer>(d: &D, ids: &[String]) -> Result<()> {
    act_on_each(d, "unread", json!({ "IDs": ids })).await
}
/// Deletes the messages `ids` for good.
pub async fn delete<D: Doer>(d: &D, ids: &[String]) -> Result<()> {
    act_on_each(d, "delete", json!({ "IDs": ids })).await
}

/// Calls off the scheduled sending of the message `id`.
pub async fn cancel_send<D: Doer>(d: &D, id: &str) -> Result<()> {
    act_on(d, Method::POST, id, "cancel_send").await
}

/// Reports the message `id` as not spam: the spam filter learns from it, and
/// Proton moves the message to the inbox.
pub async fn mark_ham<D: Doer>(d: &D, id: &str) -> Result<()> {
    act_on(d, Method::PUT, id, "mark/ham").await
}

/// Leaves the mailing list the message `id` came from, the one-click way of
/// RFC 8058.
pub async fn unsubscribe<D: Doer>(d: &D, id: &str) -> Result<()> {
    act_on(d, Method::POST, id, "unsubscribe").await
}

/// Deletes everything in the folder or label `label_id` for good, as
/// emptying Trash or Spam does.
pub async fn empty<D: Doer>(d: &D, label_id: &str) -> Result<()> {
    let request = Request::delete("/mail/v4/messages/empty").query("LabelID", label_id);
    act(d, request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The query parameters `q` adds, as `name=value` before encoding.
    fn sent(q: &ListQuery) -> Vec<String> {
        q.apply(Request::get("/x"))
            .query
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect()
    }

    #[test]
    fn a_query_sends_what_is_set_in_a_fixed_order_after_the_sort() {
        assert_eq!(sent(&ListQuery::default()), ["Sort=Time", "Desc=1"]);
        let every = ListQuery {
            label_id: Some("0".into()),
            page: Some(2),
            page_size: Some(50),
            unread: true,
            keyword: Some("invoice".into()),
            from: Some("a@example.com".into()),
            to: Some("b@example.com".into()),
            subject: Some("Hello".into()),
            begin: Some(1_700_000_000),
            end: Some(1_800_000_000),
            address_id: Some("address".into()),
        };
        assert_eq!(
            sent(&every),
            [
                "Sort=Time",
                "Desc=1",
                "LabelID=0",
                "Page=2",
                "PageSize=50",
                "Unread=1",
                "Keyword=invoice",
                "From=a@example.com",
                "Recipients=b@example.com",
                "Subject=Hello",
                "Begin=1700000000",
                "End=1800000000",
                "AddressID=address",
            ]
        );
    }
}
