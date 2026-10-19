//! Sieve filters API (`/mail/v4/filters`).

use crate::error::Result;
use crate::model::enums::filter;
use crate::transport::{Doer, Request};
use serde::Deserialize;
use serde::de::IgnoredAny;

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

/// Validate a Sieve script (does not create a filter).
pub async fn check<D: Doer>(d: &D, sieve: &str) -> Result<()> {
    let body = serde_json::json!({ "Sieve": sieve, "Version": filter::SIEVE_VERSION });
    let _: IgnoredAny = d
        .decode(Request::put("/mail/v4/filters/check").json(body))
        .await?;
    Ok(())
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
