//! Helpers the desktop's tests share.

use crate::app::RequestId;
use crate::mail::{Folder, MailFolder};

/// One of Proton's own folders, as a place to list from.
pub(crate) fn sys(folder: MailFolder) -> Folder {
    Folder::System(folder)
}

/// The requests a mailbox opens with in the tests, at boot and in their
/// `open` helpers: its first page, then its counts.
pub(crate) const OPENING_PAGE: RequestId = 1;
pub(crate) const OPENING_COUNTS: RequestId = 2;
/// A request ID no test issues: a reply to it answers nothing pending.
pub(crate) const NEVER_ISSUED: RequestId = 99;
