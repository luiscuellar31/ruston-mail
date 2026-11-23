//! Proton's endpoints, a function for each, that send their requests
//! through any [`Doer`] and read the answers into
//! types.

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
use crate::model::enums::response_code;
use crate::transport::{Doer, Request};

/// The most IDs one request to Proton names. Proton refuses a request that
/// names more than its maximum for the endpoint, with code 2024: reported as
/// 150 for reading messages and 200 for deleting them. A page of the list
/// stays well under both.
pub const MAX_IDS_PER_REQUEST: usize = 50;

/// Puts `body` to `path` once for each [`MAX_IDS_PER_REQUEST`] of `ids`, in
/// turn, with that part of them as its `IDs`, and fails naming every ID
/// Proton refused in any. A request that fails as a whole stops the rest,
/// and those before it stay done; no IDs asks nothing.
pub(crate) async fn put_each<D: Doer>(
    d: &D,
    path: &str,
    ids: &[String],
    body: serde_json::Value,
) -> Result<()> {
    let mut answers = EachResponse::default();
    for part in ids.chunks(MAX_IDS_PER_REQUEST) {
        let mut body = body.clone();
        body["IDs"] = serde_json::json!(part);
        let answer: EachResponse = d.decode(Request::put(path).json(body)).await?;
        answers.responses.extend(answer.responses);
    }
    answers.into_result()
}

/// Proton's answer to a request about several IDs: `Code` 1001 with one
/// response per ID, which can refuse some while the request succeeds.
#[derive(serde::Deserialize, Default)]
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
        let total = self.responses.len();
        let refused: Vec<_> = self
            .responses
            .into_iter()
            .filter_map(|item| {
                item.response
                    .filter(|response| response.code != response_code::SUCCESS)
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
