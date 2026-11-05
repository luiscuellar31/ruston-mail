//! Contacts (`/contacts/v4/contacts`), read only: their names and the
//! addresses filed under them.

use crate::error::Result;
use crate::transport::{Doer, Request};
use serde::{Deserialize, Serialize};

/// One address filed under a contact, with the details Proton keeps
/// unencrypted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ContactEmail {
    /// This entry's ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// The address.
    #[serde(default)]
    pub email: String,
    /// The name shown for this address.
    #[serde(default)]
    pub name: String,
    /// The ID of the contact the address is filed under.
    #[serde(rename = "ContactID", default)]
    pub contact_id: String,
}

/// A contact: its name and the addresses filed under it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Contact {
    /// The contact's ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// The name shown for the contact.
    #[serde(default)]
    pub name: String,
    /// The addresses filed under the contact.
    #[serde(rename = "ContactEmails", default)]
    pub emails: Vec<ContactEmail>,
}

#[derive(Deserialize)]
struct ContactsResp {
    #[serde(rename = "Contacts", default)]
    contacts: Vec<Contact>,
    #[serde(rename = "Total", default)]
    total: u32,
}

#[derive(Deserialize)]
struct EmailsResp {
    #[serde(rename = "ContactEmails", default)]
    contact_emails: Vec<ContactEmail>,
}

/// A `GET` of page `page`, of `page_size` items, of the listing at `path`.
fn page_of(path: &str, page: u32, page_size: u32) -> Request {
    Request::get(path)
        .query("Page", page.to_string())
        .query("PageSize", page_size.to_string())
}

/// Page `page` of the contacts, `page_size` at a time, and how many there
/// are in all.
pub async fn list<D: Doer>(d: &D, page: u32, page_size: u32) -> Result<(u32, Vec<Contact>)> {
    d.decode(page_of("/contacts/v4/contacts", page, page_size))
        .await
        .map(|found: ContactsResp| (found.total, found.contacts))
}

/// Page `page` of the addresses filed under contacts, `page_size` at a
/// time; with `email`, only the entries for that address.
pub async fn list_emails<D: Doer>(
    d: &D,
    page: u32,
    page_size: u32,
    email: Option<&str>,
) -> Result<Vec<ContactEmail>> {
    let listing = page_of("/contacts/v4/contacts/emails", page, page_size);
    let request = match email {
        Some(email) => listing.query("Email", email),
        None => listing,
    };
    let EmailsResp { contact_emails } = d.decode(request).await?;
    Ok(contact_emails)
}
