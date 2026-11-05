//! Labels and folders, as Proton lists them.

use serde::{Deserialize, Serialize};

/// A label, a folder or a contact group, which `label_type` tells apart.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Label {
    /// The label's ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// The name it is shown by.
    #[serde(default)]
    pub name: String,
    /// Its color, in hex, such as `#8080ff`.
    #[serde(default)]
    pub color: String,
    /// The names of the folders it is in, then its own, joined by `/`.
    #[serde(default)]
    pub path: String,
    /// Which kind it is, one of
    /// [`label_type`](crate::model::enums::label_type).
    #[serde(rename = "Type", default)]
    pub label_type: i64,
    /// The folder it sits in, for a folder inside another.
    #[serde(rename = "ParentID", default)]
    pub parent_id: Option<String>,
    /// Where it comes among the others when they are listed.
    #[serde(default)]
    pub order: i64,
    /// `1` for a folder whose new mail Proton notifies.
    #[serde(default)]
    pub notify: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_label() {
        let json = serde_json::json!({
            "ID": "42", "Name": "Work", "Color": "#fff", "Type": 3, "Path": "Work/2026",
            "ParentID": "41"
        });
        let l: Label = serde_json::from_value(json).unwrap();
        assert_eq!(l.id, "42");
        assert_eq!(l.label_type, 3);
        assert_eq!(l.name, "Work");
        assert_eq!(l.parent_id.as_deref(), Some("41"));
    }
}
