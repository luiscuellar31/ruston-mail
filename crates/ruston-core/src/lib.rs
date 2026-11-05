//! The Proton Mail client that both Ruston Mail apps are built on: the
//! desktop app, `ruston-mail`, and the command line, `ruston-cli`.
//!
//! Signing in and end-to-end encryption go through Proton's own crates:
//! [proton-crypto](https://github.com/ProtonMail/proton-crypto-rs), with its
//! Rust backend, for OpenPGP, and `proton-srp` for proving the password
//! without sending it.
//!
//! # Overview
//!
//! Everything starts from a [`Client`]. Signing in, with `ruston-cli login`
//! or in the desktop app, saves a session under a profile, and
//! [`Client::resume`] picks it up again. The session's tokens and the
//! passphrase of its keys stay in the system's credential store, never in a
//! file.
//!
//! What it covers:
//! - **Signing in**: the SRP password proof, a TOTP code, a separate mailbox
//!   password, and a callback, [`HvResolver`], for when Proton asks for a
//!   CAPTCHA.
//! - **Keys**: unlocking the account's keys, encrypting and decrypting with
//!   them, and checking signatures, which gives a [`Verdict`].
//! - **Mail**: listing, reading and searching messages and conversations;
//!   sending, replying and forwarding to Proton addresses, to other
//!   providers' PGP keys, behind a password, or in the clear; drafts and
//!   attachments; moving, labeling, starring, snoozing, reporting spam and
//!   deleting; Sieve filters, contacts, the account's addresses and its
//!   settings; and saving a folder's mail as `.eml` files.
//! - **Offline**: a SQLite cache that Proton's events keep current, with a
//!   full-text index of the decrypted bodies.
//!
//! New mail goes out from the account's primary address, the first in
//! Proton's `Order`. [`SendOptions`]'s `from` names another of the account's
//! addresses.
//!
//! # Modules
//!
//! - [`mail`]: [`Client`] and what it does with the mailbox.
//! - [`auth`]: signing in with SRP and a second factor, and signing out.
//! - [`crypto`]: unlocking keys, encrypting, decrypting, checking signatures.
//! - [`api`]: Proton's endpoints, a function for each, and what they answer.
//! - [`transport`]: the HTTP client every request goes through.
//! - [`model`]: [`Message`], [`Conversation`], [`Label`] and the other types
//!   Proton's answers are read into.
//! - [`session`]: saved sessions, the metadata in a file and the secrets in
//!   the credential store.
//! - [`cache`]: the SQLite cache and its full-text index.
//! - [`error`]: [`Error`], every way an operation fails, and [`Result`].
//! - [`html`]: HTML bodies made safe to show, and read as plain text.
//! - [`browser`]: the system's opener for web addresses, which both apps use.
//!
//! # Example
//!
//! Resume the session `ruston-cli login` or the desktop app saved, under the
//! profile they share by default, and show the address new mail goes out
//! from:
//!
//! ```no_run
//! # async fn demo() -> ruston_core::Result<()> {
//! use ruston_core::{Client, DEFAULT_SESSION_PROFILE};
//!
//! let client = Client::resume(DEFAULT_SESSION_PROFILE).await?;
//! match client.primary_email() {
//!     Some(address) => println!("new mail goes out from {address}"),
//!     None => println!("no address to send from"),
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Ruston Mail is not affiliated with or endorsed by Proton AG.
#![warn(missing_docs)]

pub mod api;
pub mod auth;
pub mod browser;
pub mod cache;
pub mod crypto;
pub mod error;
pub mod html;
pub mod mail;
pub mod model;
pub mod session;
pub mod transport;

/// Shared local storage identity for sessions, credentials, and the CLI cache.
pub(crate) const STORAGE_NAME: &str = "ruston-mail";

/// Session profile used by both frontends unless the CLI selects another.
pub const DEFAULT_SESSION_PROFILE: &str = "ruston";

/// The environment variable that opens either frontend's fictional mailbox
/// instead of Proton.
pub const DEMO_ENV: &str = "RUSTON_DEMO";

/// Whether a [`DEMO_ENV`] value asks for the fictional mailbox: `1`, `true`,
/// `yes` or `on`, in any case.
pub fn demo_requested(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Proton API address used unless a login selects another.
pub(crate) const DEFAULT_BASE_URL: &str = "https://mail.proton.me/api";

pub use api::contacts::{Contact, ContactEmail};
pub use api::events::LabelCount;
pub use api::filters::Filter;
pub use auth::TotpPrompt;
pub use crypto::Verdict;
pub use error::{ApiError, Error, HvChallenge, Result};
pub use mail::contacts::AddressInfo;
pub use mail::export::ExportReport;
pub use mail::read::{ConversationMessage, FullMessage, SearchOpts, UnreadableMessage};
pub use mail::send::{
    Quote, RecipientProtection, ReplyRecipients, SendOptions, reply_recipients, reply_redirected,
};
pub use mail::sync::{LocalCache, SyncReport};
pub use mail::{Client, LoginOptions, LoginPrompts, MailboxPasswordPrompt};
pub use model::{Attachment, Conversation, Label, Message, MessageMetadata, Recipient};
pub use transport::{AuthState, Body, Doer, HttpClient, HvResolver, Request, Response};

/// The version this crate was built as, from its package.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_opens_for_one_true_yes_or_on_in_any_case() {
        for value in ["1", "true", "yes", "on", " TRUE ", "On"] {
            assert!(demo_requested(value), "{value:?}");
        }
        for value in ["", "0", "false", "no", "off", "01", "demo"] {
            assert!(!demo_requested(value), "{value:?}");
        }
    }
}
