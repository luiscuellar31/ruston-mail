//! Labels and folders (`/core/v4/labels`), which Proton keeps as one kind of
//! item told apart by its `Type`.

use crate::error::Result;
use crate::model::label::Label;
use crate::transport::{Doer, Request};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::json;

#[derive(Deserialize)]
struct ListResp {
    #[serde(rename = "Labels", default)]
    labels: Vec<Label>,
}

#[derive(Deserialize)]
struct CreateResp {
    #[serde(rename = "Label")]
    label: Label,
}

/// The labels of type `label_type`: 1 for labels, 3 for folders, as
/// [`label_type`](crate::model::enums::label_type) names them.
pub async fn list<D: Doer>(d: &D, label_type: i64) -> Result<Vec<Label>> {
    let of_type = Request::get("/core/v4/labels").query("Type", label_type.to_string());
    let ListResp { labels } = d.decode(of_type).await?;
    Ok(labels)
}

/// Makes a label, or a folder by `label_type`, named `name` in `color`;
/// with `parent_id`, inside that folder.
pub async fn create<D: Doer>(
    d: &D,
    name: &str,
    color: &str,
    label_type: i64,
    parent_id: Option<&str>,
) -> Result<Label> {
    let mut new_label = json!({ "Name": name, "Color": color, "Type": label_type });
    if let Some(parent_id) = parent_id {
        new_label["ParentID"] = parent_id.into();
    }
    let CreateResp { label } = d
        .decode(Request::post("/core/v4/labels").json(new_label))
        .await?;
    Ok(label)
}

/// Changes the label or folder `id` as `body` says: its `Name`, `Color` or
/// `ParentID`.
pub async fn update<D: Doer>(d: &D, id: &str, body: serde_json::Value) -> Result<()> {
    let changed = Request::put(format!("/core/v4/labels/{id}")).json(body);
    d.decode::<IgnoredAny>(changed).await?;
    Ok(())
}

/// Deletes the labels and folders `ids`.
pub async fn delete<D: Doer>(d: &D, ids: &[String]) -> Result<()> {
    let deleted = Request::delete("/core/v4/labels").json(json!({ "LabelIDs": ids }));
    d.decode::<IgnoredAny>(deleted).await?;
    Ok(())
}
