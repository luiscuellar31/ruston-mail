//! Sieve filters API (`/mail/v4/filters`).

use crate::error::{Error, Result};
use crate::model::enums::filter;
use crate::transport::{Doer, Request};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;

/// A server-side (Sieve) filter.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Filter {
    /// Filter ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// Filter display name.
    #[serde(default)]
    pub name: String,
    /// Status: [`filter::ENABLED`] when the filter applies to new mail.
    #[serde(default)]
    pub status: i64,
    /// Sieve filter format version.
    #[serde(default)]
    pub version: i64,
}

impl Filter {
    /// Whether the filter applies to new mail.
    pub fn is_enabled(&self) -> bool {
        self.status == filter::ENABLED
    }
}

#[derive(Deserialize)]
struct ListResp {
    #[serde(rename = "Filters", default)]
    filters: Vec<Filter>,
}

#[derive(Deserialize)]
struct OneResp {
    #[serde(rename = "Filter")]
    filter: Filter,
}

/// List the account's Sieve filters.
pub async fn list<D: Doer>(d: &D) -> Result<Vec<Filter>> {
    let r: ListResp = d.decode(Request::get("/mail/v4/filters")).await?;
    Ok(r.filters)
}

/// Create a filter from a raw Sieve script.
pub async fn create<D: Doer>(d: &D, name: &str, sieve: &str) -> Result<Filter> {
    let body = serde_json::json!({
        "Name": name,
        "Sieve": sieve,
        "Version": filter::SIEVE_VERSION,
        "Status": filter::ENABLED,
    });
    let r: OneResp = d
        .decode(Request::post("/mail/v4/filters").json(body))
        .await?;
    Ok(r.filter)
}

/// What Proton's check says about a script. The form of `Issues` is not
/// documented, so it is read loosely.
#[derive(Deserialize)]
struct CheckResp {
    #[serde(rename = "Issues", default)]
    issues: serde_json::Value,
}

/// Validate a Sieve script (does not create a filter). Problems Proton
/// reports in it come back as an error that lists them.
pub async fn check<D: Doer>(d: &D, sieve: &str) -> Result<()> {
    let body = serde_json::json!({ "Sieve": sieve, "Version": filter::SIEVE_VERSION });
    let r: CheckResp = d
        .decode(Request::put("/mail/v4/filters/check").json(body))
        .await?;
    let issues = issue_texts(&r.issues);
    if issues.is_empty() {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "Proton reports problems in the Sieve script: {}",
            issues.join("; ")
        )))
    }
}

/// Each issue's text: its message when it has one, otherwise the issue as
/// JSON, so nothing Proton reports is lost whatever form it takes.
fn issue_texts(issues: &Value) -> Vec<String> {
    let text = |issue: &Value| match issue {
        Value::String(text) => text.clone(),
        Value::Object(fields) => ["message", "Message"]
            .iter()
            .find_map(|key| fields.get(*key)?.as_str().map(str::to_owned))
            .unwrap_or_else(|| issue.to_string()),
        other => other.to_string(),
    };
    match issues {
        Value::Null => Vec::new(),
        Value::Array(list) => list.iter().map(text).collect(),
        Value::Object(fields) if fields.is_empty() => Vec::new(),
        Value::String(text) if text.is_empty() => Vec::new(),
        other => vec![text(other)],
    }
}

/// Delete a filter by ID.
pub async fn delete<D: Doer>(d: &D, id: &str) -> Result<()> {
    let _: IgnoredAny = d
        .decode(Request::delete(format!("/mail/v4/filters/{id}")))
        .await?;
    Ok(())
}

/// Enable a filter by ID.
pub async fn enable<D: Doer>(d: &D, id: &str) -> Result<()> {
    let _: IgnoredAny = d
        .decode(Request::put(format!("/mail/v4/filters/{id}/enable")))
        .await?;
    Ok(())
}

/// Disable a filter by ID.
pub async fn disable<D: Doer>(d: &D, id: &str) -> Result<()> {
    let _: IgnoredAny = d
        .decode(Request::put(format!("/mail/v4/filters/{id}/disable")))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::HttpClient;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn a_new_filter_is_written_in_sieve_2_and_enabled() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mail/v4/filters"))
            .and(body_json(serde_json::json!({
                "Name": "news", "Sieve": "keep;", "Version": 2, "Status": 1
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Code": 1000, "Filter": {"ID": "filter-id"}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let http = HttpClient::new(server.uri(), "Other");

        let filter = create(&http, "news", "keep;").await.unwrap();
        assert_eq!(filter.id, "filter-id");
    }

    #[tokio::test]
    async fn a_script_is_checked_as_sieve_2() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/mail/v4/filters/check"))
            .and(body_json(
                serde_json::json!({ "Sieve": "keep;", "Version": 2 }),
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let http = HttpClient::new(server.uri(), "Other");

        check(&http, "keep;").await.unwrap();
    }

    #[tokio::test]
    async fn a_script_proton_finds_problems_in_is_refused_with_them() {
        let checked = |issues: serde_json::Value| async move {
            let server = MockServer::start().await;
            Mock::given(method("PUT"))
                .and(path("/mail/v4/filters/check"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "Code": 1000, "Issues": issues
                })))
                .mount(&server)
                .await;
            check(&HttpClient::new(server.uri(), "Other"), "kep;").await
        };

        let refused = checked(serde_json::json!([
            {"message": "unknown command 'kep'", "from": {"line": 1}},
            {"Unexpected": true},
            "line 2: missing semicolon",
        ]))
        .await
        .unwrap_err()
        .to_string();
        for said in [
            "unknown command 'kep'",
            r#"{"Unexpected":true}"#,
            "line 2: missing semicolon",
        ] {
            assert!(refused.contains(said), "{refused}");
        }

        for none in [serde_json::json!([]), serde_json::json!(null)] {
            assert!(checked(none).await.is_ok());
        }
    }

    #[test]
    fn only_the_enabled_status_applies_a_filter() {
        let with_status = |status| Filter {
            id: "filter-id".into(),
            name: "news".into(),
            status,
            version: filter::SIEVE_VERSION,
        };
        assert!(with_status(filter::ENABLED).is_enabled());
        assert!(!with_status(0).is_enabled());
    }
}
