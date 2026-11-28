//! Checking for a newer release while Settings ask for it: what the last
//! check found, and the notice a newer release gets the first time.

use super::{App, Effects, Message, Notice};
use crate::releases::{self, Version};

/// What checking for updates last found, which Settings → About shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStatus {
    /// The first check has not answered yet.
    Checking,
    /// No published release is newer than the one running.
    Latest,
    /// This newer release is out.
    Available(Version),
    /// GitHub could not be asked, or its reply could not be read.
    Failed,
}

impl App {
    /// What checking for updates last found, while it is on.
    pub fn update_status(&self) -> Option<UpdateStatus> {
        self.update_status
    }

    /// Turns checking on or off. Off, what was found goes, and an answer
    /// still on its way is not read.
    pub(super) fn set_check_for_updates(&mut self, on: bool) {
        self.remember(|settings| settings.check_for_updates = on);
        if !on {
            self.update_status = None;
        }
    }

    /// Asks GitHub for the latest release, while checking is on.
    pub(super) fn check_for_updates(&mut self) -> Effects {
        if !self.settings.check_for_updates {
            return Effects::none();
        }
        // Asking again keeps what the last check found on show meanwhile.
        if matches!(self.update_status, None | Some(UpdateStatus::Failed)) {
            self.update_status = Some(UpdateStatus::Checking);
        }
        Effects::perform(releases::latest_release(), Message::UpdatesChecked)
    }

    /// Keeps what a check found. A newer release is announced at the foot of
    /// the mailbox the first time it is found, and a check that fails after
    /// one that answered keeps that answer.
    pub(super) fn updates_checked(&mut self, latest: Option<Version>) {
        if !self.settings.check_for_updates {
            return;
        }
        let found = match latest {
            Some(latest) if latest.is_newer() => UpdateStatus::Available(latest),
            Some(_) => UpdateStatus::Latest,
            None if matches!(
                self.update_status,
                Some(UpdateStatus::Latest | UpdateStatus::Available(_))
            ) =>
            {
                return;
            }
            None => UpdateStatus::Failed,
        };
        if let UpdateStatus::Available(version) = found
            && self.update_status != Some(found)
        {
            self.session.notice = Some(Notice::UpdateAvailable(version));
        }
        self.update_status = Some(found);
    }
}
