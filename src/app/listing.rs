//! The open folder's list and what goes with it: opening a folder, its
//! pages, refreshing them, a search, the account's folders and the counts.

use super::{
    App, Effects, ListStatus, Mailbox, Message, PageRequest, RequestId, SearchRequest, SessionEpoch,
};
use crate::mail::{ConversationPage, Folder, ListFilter, ListOrder, MailboxError, OwnPlaces};

impl App {
    /// Proton owns the folder counts; reload them after a change.
    pub(super) fn reload_counts(&mut self) -> Effects {
        let counts_request = self.next_request();
        let counts = self
            .active_mailbox()
            .map(|mailbox| mailbox.refresh_counts(counts_request));

        self.fetch_counts(counts)
    }

    /// Reconciles the saved folder with the account's current folders.
    pub(super) fn reconcile_open_folder(&mut self) -> Effects {
        let Some(open) = self
            .mailbox
            .as_ref()
            .map(|mailbox| mailbox.folder().clone())
        else {
            return Effects::none();
        };
        // Proton's own folders are always there, so only the account's own
        // can have gone stale.
        let Some(id) = open.custom_id() else {
            return Effects::none();
        };
        let current = self
            .session
            .folders
            .iter()
            .find(|place| place.custom_id() == Some(id))
            .cloned();
        // Gone from the account means gone from the sidebar, and the Inbox is
        // the one place that is always there to fall back to.
        let Some(current) = current else {
            return self.select_folder(Folder::INBOX);
        };
        if current == open {
            return Effects::none();
        }

        // A rename keeps the stable id, loaded mail and reader state.
        if let Some(mailbox) = self.active_mailbox() {
            mailbox.rename_folder(current.clone());
        }
        self.remember_folder(&current);

        Effects::none()
    }

    /// Remembers the last real folder, the open account's; demo navigation
    /// is process-local.
    pub(super) fn remember_folder(&mut self, folder: &Folder) {
        if !self.is_demo() {
            self.remember(|settings| settings.accounts.set_folder(folder.clone()));
        }
    }

    pub(super) fn select_folder(&mut self, folder: Folder) -> Effects {
        if self.auth_state.is_authenticated()
            && self
                .mailbox
                .as_ref()
                .is_some_and(|mailbox| mailbox.folder() != &folder)
        {
            self.inspections.cancel();
            self.abandon_reader_load();
        }
        self.showing_settings = false;
        self.session.pending_link = None;
        self.session.failed_save = None;
        self.remember_folder(&folder);
        let request = self.next_request();
        let page = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.select_folder(folder, request));

        self.fetch_page(page)
    }

    /// Lists only the open folder's conversations that `filter` shows, or all
    /// of them again.
    pub(super) fn show_only(&mut self, filter: ListFilter) -> Effects {
        let request = self.next_request();
        let page = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.show_only(filter, request));
        self.fetch_page(page)
    }

    /// Lists the open folder's conversations in `order`.
    pub(super) fn sort_by(&mut self, order: ListOrder) -> Effects {
        let request = self.next_request();
        let page = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.sort_by(order, request));
        self.fetch_page(page)
    }

    /// Runs what is typed in the search field against the server, which sees
    /// every folder and every conversation, not just the loaded ones.
    pub(super) fn start_search(&mut self) -> Effects {
        let request = self.next_request();
        let Some(request) = self
            .active_mailbox()
            .and_then(|mailbox| mailbox.start_search(request))
        else {
            return Effects::none();
        };

        self.inspections.cancel();
        self.fetch_search(request)
    }

    pub(super) fn fetch_search(&self, request: SearchRequest) -> Effects {
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        let query = request.query.clone();
        let (page, page_size) = (request.page, request.page_size);

        Effects::perform(
            async move { backend.search(&query, page, page_size).await },
            move |result| Message::SearchLoaded(request.clone(), result),
        )
    }

    /// Shows the page of results a search brought, or asks this device's
    /// index instead when Proton was out of reach.
    pub(super) fn finish_search(
        &mut self,
        request: SearchRequest,
        result: Result<ConversationPage, MailboxError>,
    ) -> Effects {
        // With Proton out of reach, this device's index answers instead when
        // asked to; never for a later page, as the index gives all it finds
        // at once.
        let unreachable = matches!(result, Err(MailboxError::Connection));
        if unreachable
            && request.page == 0
            && self.settings.search_device_offline
            && self.backend.is_some()
        {
            return self.fetch_device_search(request);
        }
        let error = self
            .mailbox
            .as_mut()
            .and_then(|mailbox| mailbox.finish_search(&request, result));
        self.end_session_if_expired(error);
        Effects::none()
    }

    /// Looks in this device's index for a search Proton could not answer.
    pub(super) fn fetch_device_search(&self, request: SearchRequest) -> Effects {
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        let query = request.query.clone();
        Effects::perform(
            async move { backend.search_local(&query).await },
            move |found| Message::DeviceSearchLoaded(request, found),
        )
    }

    pub(super) fn refresh_mailbox(&mut self) -> Effects {
        let page_request = self.next_request();
        let counts_request = self.next_request();
        let Some(mailbox) = self.active_mailbox() else {
            return Effects::none();
        };
        let page = mailbox.refresh(page_request);
        let counts = mailbox.refresh_counts(counts_request);

        if page.is_some() {
            self.inspections.cancel();
        }
        let folders = if self.session.refetch_folders {
            self.fetch_folders()
        } else {
            Effects::none()
        };

        Effects::batch([
            self.fetch_page(page),
            self.fetch_counts(Some(counts)),
            folders,
        ])
    }

    pub(super) fn auto_refresh_mailbox(&mut self) -> Effects {
        if !self.auto_refresh_available() {
            return Effects::none();
        }
        if let Some(mailbox) = &mut self.mailbox {
            mailbox.invalidate_cached_listings();
        }

        let refresh = self.refresh_mailbox();
        // Nothing was loading before, so a refresh on its way is this one.
        self.session.automatic_refresh =
            self.mailbox
                .as_ref()
                .and_then(|mailbox| match mailbox.status() {
                    ListStatus::Refreshing(request) => Some(request),
                    _ => None,
                });
        let notifications = if self.is_demo() {
            Effects::none()
        } else {
            self.fetch_new_mail_notification()
        };
        Effects::batch([refresh, notifications])
    }

    pub(super) fn load_more_conversations(&mut self) -> Effects {
        let request = self.next_request();
        let Some(mailbox) = self.active_mailbox() else {
            return Effects::none();
        };
        if mailbox.shown_search_query().is_some() {
            let search = mailbox.load_more_search(request);
            search.map_or_else(Effects::none, |search| self.fetch_search(search))
        } else {
            let page = mailbox.load_more(request);
            self.fetch_page(page)
        }
    }

    pub(super) fn fetch_page(&self, request: Option<PageRequest>) -> Effects {
        let (Some(request), Some(backend)) = (request, self.backend.clone()) else {
            return Effects::none();
        };

        let PageRequest {
            id,
            folder,
            filter,
            order,
            page,
            page_size,
        } = request;

        Effects::perform(
            async move {
                backend
                    .list_conversations(&folder, filter, order, page, page_size)
                    .await
            },
            move |result| Message::ConversationsLoaded(id, result),
        )
    }

    /// Shows a page of the list, if it is the one awaited, and goes on with
    /// what waited for it: listing again when a change made meanwhile left
    /// it stale, a search submitted meanwhile, or the rows worth looking into.
    pub(super) fn finish_page(
        &mut self,
        request: RequestId,
        mut result: Result<ConversationPage, MailboxError>,
    ) -> Effects {
        if !self.accepts_page(request) {
            return Effects::none();
        }
        let candidates = match &mut result {
            Ok(page) => std::mem::take(&mut page.inspect_candidates),
            Err(_) => Vec::new(),
        };
        let error = self
            .mailbox
            .as_mut()
            .and_then(|mailbox| mailbox.finish_page(request, result));
        self.end_session_if_expired(error);
        if error.is_none() {
            let revalidating = self
                .mailbox
                .as_ref()
                .is_some_and(Mailbox::needs_revalidation);
            if revalidating {
                return self.refresh_mailbox();
            }
        }
        // A search submitted while this page loaded has waited for it.
        if self
            .mailbox
            .as_mut()
            .is_some_and(Mailbox::take_deferred_search)
        {
            return self.start_search();
        }
        if error.is_none() {
            let accepted = self
                .mailbox
                .as_ref()
                .is_some_and(|m| m.status() == ListStatus::Loaded);
            if accepted && !candidates.is_empty() {
                return self.inspect_candidates(candidates);
            }
        }
        Effects::none()
    }

    /// Asks for the folders the account made. They change rarely, so only a
    /// list that failed to load is asked for again, by the next refresh.
    pub(super) fn fetch_folders(&mut self) -> Effects {
        self.session.refetch_folders = false;
        let Some(backend) = self.backend.clone() else {
            return Effects::none();
        };
        let epoch = self.session_epoch;

        Effects::perform(async move { backend.list_folders().await }, move |result| {
            Message::FoldersLoaded(epoch, result)
        })
    }

    /// Keeps the folders the account made, which arrived, and asks for the
    /// counts again now that they are known.
    pub(super) fn finish_folders(
        &mut self,
        epoch: SessionEpoch,
        result: Result<OwnPlaces, MailboxError>,
    ) -> Effects {
        if !self.is_current_session(epoch) {
            return Effects::none();
        }
        match result {
            Ok(places) => {
                self.session.folders = places.folders;
                self.session.place_looks = places.looks;
            }
            // Without them the sidebar shows Proton's own until the next
            // refresh asks again. A list that did not arrive says nothing about
            // the open folder, which stays open.
            Err(error) => {
                self.session.refetch_folders = true;
                self.end_session_if_expired(Some(error));
                return Effects::none();
            }
        }
        // Counts were asked for before the folders were known, so ask again
        // now that they are.
        Effects::batch([self.reconcile_open_folder(), self.reload_counts()])
    }

    pub(super) fn fetch_counts(&self, request: Option<RequestId>) -> Effects {
        let (Some(request), Some(backend)) = (request, self.backend.clone()) else {
            return Effects::none();
        };

        let folders = self.session.folders.clone();

        Effects::perform(
            async move { backend.conversation_counts(&folders).await },
            move |result| Message::CountsLoaded(request, result),
        )
    }
}
