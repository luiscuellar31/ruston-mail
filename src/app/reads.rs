//! Marking a conversation read once its content shows, unless Settings say
//! not to. The demo marks it when it is selected; Proton's conversations are
//! marked once the detail the reader waits for arrives, or one cached opens. Either way, while a draft covers the reader the conversation opens
//! where nobody sees it, so `postpone_read_while_covered` keeps it unread
//! until `update` sees the reader uncovered and `mark_read_once_shown` marks
//! it, if it is still the one open.

use super::{ActionRequest, App, Effects, Message, run_action};
use crate::mail::{MailBackend, MailboxError, demo::DemoMailbox};

impl App {
    /// Once an unread Proton row's content is shown, marks it read on Proton.
    /// Demo rows are already marked read when they are selected, unless a
    /// draft covers the reader.
    pub(super) fn mark_opened_read(&mut self) -> Effects {
        if !self.settings.mark_read_on_open {
            return Effects::none();
        }
        let Some(MailBackend::Proton(service)) = self.backend.clone() else {
            return Effects::none();
        };
        if let Some(id) = self.selected_conversation_id()
            && self.postpone_read_while_covered(&id)
        {
            return Effects::none();
        }
        let request = self.next_request();
        let Some(request) = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_mark_read(request))
        else {
            return Effects::none();
        };
        // An automatic read goes on its own.
        run_action(service, vec![request], |mut sent, result| {
            Message::MarkReadFinished(sent.remove(0), result)
        })
    }

    /// Whether a draft covers the reader, so the conversation `id` opens
    /// where nobody sees it. It then stays unread until the reader shows it.
    pub(super) fn postpone_read_while_covered(&mut self, id: &str) -> bool {
        if !self.draft_covers_reader() {
            return false;
        }
        self.session.read_once_shown = Some(id.to_owned());
        true
    }

    /// Marks read what was opened while a draft covered the reader, now that
    /// the reader shows it; nothing if another conversation is open by then.
    pub(super) fn mark_read_once_shown(&mut self) -> Effects {
        let Some(id) = self.session.read_once_shown.take() else {
            return Effects::none();
        };
        if self.selected_conversation_id().as_deref() != Some(id.as_str()) {
            return Effects::none();
        }
        match self.backend.clone() {
            Some(MailBackend::Demo(service)) => {
                if self.settings.mark_read_on_open {
                    self.mark_demo_read(&service, &id);
                }
                Effects::none()
            }
            Some(MailBackend::Proton(_)) => self.mark_opened_read(),
            None => Effects::none(),
        }
    }

    pub(super) fn finish_mark_read(
        &mut self,
        request: ActionRequest,
        result: Result<(), MailboxError>,
    ) -> Effects {
        let Some(completion) = self
            .mailbox
            .as_mut()
            .map(|mailbox| mailbox.finish_mark_read(&request, result))
        else {
            return Effects::none();
        };
        // The mailbox also records an error for any queued explicit action.
        // An expired session closes the mailbox, and then nothing is sent.
        if let Some(error) = completion.error {
            if self.end_session_if_expired(Some(error)) {
                return Effects::none();
            }
            return self.send_action(completion.next_action);
        }
        let next = self.send_action(completion.next_action);
        let counts = if completion.read_confirmed {
            self.reload_counts()
        } else {
            Effects::none()
        };
        Effects::batch([next, counts])
    }

    /// Marks the demo's conversation `id` read, as Proton would, and lists
    /// it so; false when the demo has no such conversation.
    pub(super) fn mark_demo_read(&mut self, service: &DemoMailbox, id: &str) -> bool {
        if !service.set_unread(id, false) {
            return false;
        }
        self.apply_demo_snapshot(service);
        true
    }
}
