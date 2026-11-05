//! The event stream (`/core/v4/events`): what changed since a cursor, so a
//! local copy of the mailbox can follow along.

use crate::error::Result;
use crate::model::message::MessageMetadata;
use crate::transport::{Doer, Request};
use serde::Deserialize;

/// What happened to the item an event names, as its `Action` gives it.
pub mod action {
    /// It was deleted.
    pub const DELETE: u8 = 0;
    /// It was created.
    pub const CREATE: u8 = 1;
    /// It changed, and the event carries it whole.
    pub const UPDATE: u8 = 2;
    /// Only its flags or labels changed.
    pub const UPDATE_FLAGS: u8 = 3;
}

#[derive(Deserialize)]
struct LatestResp {
    #[serde(rename = "EventID")]
    event_id: String,
}

/// A change to one message.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageEvent {
    /// The message's ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// What happened to it, one of the [`action`] codes.
    #[serde(rename = "Action")]
    pub action: u8,
    /// Its metadata, when the action brings it.
    #[serde(rename = "Message", default)]
    pub message: Option<MessageMetadata>,
}

/// How many items a label holds, and how many of them are unread.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct LabelCount {
    /// The label's or folder's ID.
    #[serde(rename = "LabelID")]
    pub label_id: String,
    /// How many items it holds.
    #[serde(rename = "Total", default)]
    pub total: i64,
    /// How many of them are unread.
    #[serde(rename = "Unread", default)]
    pub unread: i64,
}

#[derive(Deserialize)]
struct EventResp {
    #[serde(rename = "EventID")]
    event_id: String,
    #[serde(rename = "More", default)]
    more: i64,
    #[serde(rename = "Refresh", default)]
    refresh: i64,
    #[serde(rename = "Messages", default)]
    messages: Vec<MessageEvent>,
    #[serde(rename = "MessageCounts", default)]
    counts: Vec<LabelCount>,
}

/// The events after a cursor, read.
#[derive(Debug, Clone)]
pub struct EventBatch {
    /// The cursor to ask from next.
    pub event_id: String,
    /// Whether more events wait to be fetched right away.
    pub more: bool,
    /// Whether Proton wants the local cache listed again from scratch.
    pub refresh: bool,
    /// The messages that changed.
    pub messages: Vec<MessageEvent>,
    /// The labels whose counts changed, with their counts now.
    pub counts: Vec<LabelCount>,
}

/// The newest event's ID, where following the stream starts.
pub async fn get_latest_event_id<D: Doer>(d: &D) -> Result<String> {
    let LatestResp { event_id } = d.decode(Request::get("/core/v4/events/latest")).await?;
    Ok(event_id)
}

/// The events after the cursor `event_id`.
pub async fn get_events<D: Doer>(d: &D, event_id: &str) -> Result<EventBatch> {
    let since = Request::get(format!("/core/v4/events/{event_id}"));
    let events: EventResp = d.decode(since).await?;
    Ok(EventBatch {
        event_id: events.event_id,
        // `More` is 0 or 1, while `Refresh` holds flags: its lowest bit, the
        // mail's, asks for the resync.
        more: events.more == 1,
        refresh: events.refresh & 1 != 0,
        messages: events.messages,
        counts: events.counts,
    })
}
