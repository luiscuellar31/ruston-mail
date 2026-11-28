//! A link clicked in a message: where it really goes, the question that
//! waits for a yes when Settings ask for one, and opening it.

use super::{App, Effects, MailtoError, open_in_browser};
use crate::mailto::Request;

/// A link clicked in a message, waiting for the user to confirm it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLink {
    /// The complete, normalized address that opens.
    pub url: String,
    /// Where the link really goes: the web host or the mail address.
    pub target: String,
}

impl PendingLink {
    /// Accepts only web and mail links.
    pub(super) fn parse(link: &str) -> Option<Self> {
        let url = url::Url::parse(link.trim()).ok()?;
        let target = match url.scheme() {
            "http" | "https" => url.host_str()?.to_owned(),
            "mailto" => mail_target(url.as_str()).unwrap_or_else(|| url.path().to_owned()),
            _ => return None,
        };

        Some(Self {
            url: url.into(),
            target,
        })
    }
}

/// Who a mail link writes to: the addresses the composer would take from it,
/// those after `?to=` included, as the first few and how many more. `None`
/// when the composer would refuse the link.
fn mail_target(link: &str) -> Option<String> {
    const NAMED: usize = 3;
    let request = Request::parse(link)?;
    let addresses = request.recipients();
    if addresses.len() <= NAMED {
        return Some(addresses.join(", "));
    }
    Some(format!(
        "{} and {} more",
        addresses[..NAMED].join(", "),
        addresses.len() - NAMED
    ))
}

impl App {
    pub fn pending_link(&self) -> Option<&PendingLink> {
        self.session.pending_link.as_ref()
    }

    pub(super) fn open_link_target(&mut self, url: String) -> Effects {
        if !Request::is_mailto(&url) {
            return open_in_browser(url);
        }
        match Request::parse(&url) {
            Some(request) => self.receive_mailto(request),
            None => self.session.mailto_error = Some(MailtoError::Unreadable),
        }
        Effects::none()
    }
}
