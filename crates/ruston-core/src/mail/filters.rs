//! The account's filters: Sieve scripts Proton runs on the mail that
//! arrives. They are listed here in the order a filter goes through them.

use super::Client;
use crate::api;
use crate::api::filters::Filter;
use crate::error::Result;

impl Client {
    /// Checks a Sieve script without saving it. Problems Proton finds in it
    /// come back as an error that lists them.
    pub async fn check_filter(&self, sieve: &str) -> Result<()> {
        api::filters::check(self.http(), sieve).await
    }

    /// Saves a Sieve script as a new filter called `name`, turned on.
    pub async fn create_filter(&self, name: &str, sieve: &str) -> Result<Filter> {
        api::filters::create(self.http(), name, sieve).await
    }

    /// The account's filters.
    pub async fn list_filters(&self) -> Result<Vec<Filter>> {
        api::filters::list(self.http()).await
    }

    /// Turns the filter `id` on, so it applies to mail that arrives.
    pub async fn enable_filter(&self, id: &str) -> Result<()> {
        api::filters::enable(self.http(), id).await
    }

    /// Turns the filter `id` off, keeping it.
    pub async fn disable_filter(&self, id: &str) -> Result<()> {
        api::filters::disable(self.http(), id).await
    }

    /// Deletes the filter `id`.
    pub async fn delete_filter(&self, id: &str) -> Result<()> {
        api::filters::delete(self.http(), id).await
    }
}
