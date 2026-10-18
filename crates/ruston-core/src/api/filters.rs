//! Sieve filters API (`/mail/v4/filters`).

use crate::error::Result;
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
    /// Status (`1` = enabled, `0` = disabled).
    #[serde(default)]
    pub status: i64,
    /// Sieve filter format version.
    #[serde(default)]
    pub version: i64,
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

/// The Sieve version filters are written in.
const SIEVE_VERSION: u8 = 2;
/// The status of a filter that applies to new mail.
const ENABLED_STATUS: u8 = 1;

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
        "Version": SIEVE_VERSION,
        "Status": ENABLED_STATUS,
    });
    let r: OneResp = d
        .decode(Request::post("/mail/v4/filters").json(body))
        .await?;
    Ok(r.filter)
}

/// Validate a Sieve script (does not create a filter).
pub async fn check<D: Doer>(d: &D, sieve: &str) -> Result<()> {
    let body = serde_json::json!({ "Sieve": sieve, "Version": SIEVE_VERSION });
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
}
