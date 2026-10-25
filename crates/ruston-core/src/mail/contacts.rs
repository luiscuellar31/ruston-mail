//! Contacts (read) + address listing.

use super::Client;
use crate::api;
use crate::api::contacts::{Contact, ContactEmail};
use crate::error::Result;
use std::collections::HashSet;

/// Contact email entries asked for at a time.
const CONTACT_EMAILS_PAGE_SIZE: u32 = 100;

/// A sending address.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AddressInfo {
    /// The address identifier.
    pub id: String,
    /// The address email.
    pub email: String,
}

impl Client {
    /// List contacts (paged), returning the total count and the page.
    pub async fn list_contacts(&self, page: u32, page_size: u32) -> Result<(u32, Vec<Contact>)> {
        api::contacts::list(self.http(), page, page_size).await
    }

    /// Every contact email entry, optionally only those for one address.
    /// They are asked for a page at a time until a page comes back short, or
    /// brings nothing new, which keeps a server that ignores the page number
    /// from looping forever.
    pub async fn list_contact_emails(&self, email: Option<&str>) -> Result<Vec<ContactEmail>> {
        let mut emails = Vec::new();
        let mut seen = HashSet::new();
        for page in 0.. {
            let entries =
                api::contacts::list_emails(self.http(), page, CONTACT_EMAILS_PAGE_SIZE, email)
                    .await?;
            let full = entries.len() >= CONTACT_EMAILS_PAGE_SIZE as usize;
            let before = emails.len();
            emails.extend(
                entries
                    .into_iter()
                    .filter(|entry| seen.insert(entry.id.clone())),
            );
            if !full || emails.len() == before {
                break;
            }
        }
        Ok(emails)
    }

    /// The account's sending addresses (from the unlocked key store).
    pub fn addresses(&self) -> Vec<AddressInfo> {
        self.keys()
            .addresses
            .iter()
            .map(|a| AddressInfo {
                id: a.address_id.clone(),
                email: a.email.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::mail::test_support;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A page of `count` contact email entries, numbered from `from`.
    fn page(from: usize, count: usize) -> serde_json::Value {
        let entries: Vec<_> = (from..from + count)
            .map(|n| {
                serde_json::json!({
                    "ID": format!("email-{n}"),
                    "Email": format!("person{n}@example.org"),
                    "Name": format!("Person {n}"),
                })
            })
            .collect();
        serde_json::json!({"Code": 1000, "ContactEmails": entries})
    }

    #[tokio::test]
    async fn every_contact_email_is_listed_past_the_first_page() {
        let server = MockServer::start().await;
        for (number, body) in [("0", page(0, 100)), ("1", page(100, 30))] {
            Mock::given(method("GET"))
                .and(path("/contacts/v4/contacts/emails"))
                .and(query_param("Page", number))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
        }

        let emails = test_support::client(&server, Vec::new())
            .list_contact_emails(None)
            .await
            .unwrap();

        assert_eq!(emails.len(), 130);
        assert_eq!(emails[129].email, "person129@example.org");
        server.verify().await;
    }

    #[tokio::test]
    async fn a_server_that_ignores_the_page_cannot_loop_forever() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/contacts/v4/contacts/emails"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(0, 100)))
            .expect(2)
            .mount(&server)
            .await;

        let emails = test_support::client(&server, Vec::new())
            .list_contact_emails(None)
            .await
            .unwrap();

        assert_eq!(emails.len(), 100);
        server.verify().await;
    }
}
