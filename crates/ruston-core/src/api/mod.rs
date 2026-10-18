//! Typed API wrappers over the transport `Doer`.

pub mod attachments;
pub mod contacts;
pub mod conversations;
pub mod events;
pub mod filters;
pub mod keys;
pub mod labels;
pub mod messages;
pub mod settings;

pub use keys::{ApiPublicKey, RecipientKeys};
pub use messages::ListQuery;

use crate::error::{ApiError, Error, Result};

/// Proton's answer to a request about several IDs: `Code` 1001 with one
/// response per ID, which can refuse some while the request succeeds.
#[derive(serde::Deserialize)]
pub(crate) struct EachResponse {
    #[serde(rename = "Responses", default)]
    responses: Vec<IdResponse>,
}

#[derive(serde::Deserialize)]
struct IdResponse {
    #[serde(rename = "ID", default)]
    id: String,
    #[serde(rename = "Response")]
    response: Option<IdCode>,
}

#[derive(serde::Deserialize)]
struct IdCode {
    #[serde(rename = "Code")]
    code: i64,
    #[serde(rename = "Error", default)]
    error: String,
}

impl EachResponse {
    /// `Ok` when Proton took every ID, or an error naming those it refused.
    /// Only a response that gives another code than success refuses one.
    pub(crate) fn into_result(self) -> Result<()> {
        const SUCCESS: i64 = 1000;
        let total = self.responses.len();
        let refused: Vec<_> = self
            .responses
            .into_iter()
            .filter_map(|item| {
                item.response
                    .filter(|response| response.code != SUCCESS)
                    .map(|response| (item.id, response))
            })
            .collect();
        let Some((_, first)) = refused.first() else {
            return Ok(());
        };
        let code = first.code;
        let named: Vec<_> = refused
            .iter()
            .map(|(id, response)| {
                if response.error.is_empty() {
                    id.clone()
                } else {
                    format!("{id} ({})", response.error)
                }
            })
            .collect();
        Err(Error::Api(ApiError {
            http_status: 200,
            code,
            message: format!(
                "Proton refused {} of {total}: {}",
                refused.len(),
                named.join(", ")
            ),
            raw_body: String::new(),
        }))
    }
}
