//! Foreground and background refresh timing, independent of network work.

const INTERVAL: f64 = 60.0;
const BACKGROUND_INTERVAL: f64 = 180.0;
const AFTER_IDLE: f64 = 30.0;
const MAX_BACKOFF: f64 = 300.0;
/// How many times failed requests double the wait, at most.
const MAX_BACKOFF_DOUBLINGS: u32 = 3;
/// The shortest wait before the next check, even with a refresh overdue, so
/// the window is not repainted without pause.
const MIN_REPAINT_WAIT: f64 = 0.05;

/// What the app allows when a refresh is considered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Conditions {
    /// One of the app's windows has the focus.
    pub(super) focused: bool,
    /// A Proton mailbox is open; the demo never refreshes.
    pub(super) active: bool,
    /// Nothing else is changing the list.
    pub(super) available: bool,
}

/// Focused, with a mailbox open and nothing else changing it.
#[cfg(test)]
pub(super) const FOREGROUND: Conditions = Conditions {
    focused: true,
    active: true,
    available: true,
};
/// The same in the background.
#[cfg(test)]
pub(super) const BACKGROUND: Conditions = Conditions {
    focused: false,
    ..FOREGROUND
};
/// In the background while something else changes the list.
#[cfg(test)]
pub(super) const BUSY: Conditions = Conditions {
    available: false,
    ..BACKGROUND
};
/// In the background with no mailbox open.
#[cfg(test)]
pub(super) const SIGNED_OUT: Conditions = Conditions {
    active: false,
    ..BACKGROUND
};

/// Schedules mailbox refreshes using egui's monotonic clock. Network work
/// still belongs to `App`; this only decides when to ask for it.
#[derive(Debug, Default)]
pub(super) struct AutoRefresh {
    active: bool,
    focused: bool,
    away_since: Option<f64>,
    focus_due: bool,
    next_at: f64,
    last_activity: f64,
    pending: Option<u64>,
    failures: u32,
}

impl AutoRefresh {
    /// Whether to refresh now. The first call with a mailbox open only
    /// starts the clock, and closing the mailbox stops it.
    pub(super) fn should_start(&mut self, now: f64, conditions: Conditions) -> bool {
        if !conditions.active {
            *self = Self::default();
            return false;
        }
        if !self.active {
            self.active = true;
            self.focused = conditions.focused;
            self.away_since = (!conditions.focused).then_some(now);
            self.last_activity = now;
            self.next_at = now + self.delay();
            return false;
        }
        self.track_focus(now, conditions.focused);

        if self.pending.is_some() || !conditions.available {
            return false;
        }
        // Coming back to the app refreshes early, but never inside a failed
        // request's backoff.
        let backing_off = self.failures > 0;
        if now >= self.next_at || (self.focus_due && !backing_off) {
            self.focus_due = false;
            return true;
        }

        false
    }

    /// Follows the focus: coming back after a while away makes a refresh
    /// due, and the regular wait counts from the last activity again.
    fn track_focus(&mut self, now: f64, focused: bool) {
        if !focused {
            if self.focused {
                self.away_since = Some(now);
                self.focus_due = false;
            }
        } else if !self.focused {
            self.focus_due = self.away_since.is_some_and(|away| now - away >= AFTER_IDLE)
                && now - self.last_activity >= AFTER_IDLE;
            self.away_since = None;
        }
        if self.focused != focused {
            self.focused = focused;
            // Returning to the app must not shorten a failed request's backoff.
            if self.failures == 0 {
                self.next_at = self.last_activity + self.delay();
            }
        }
    }

    pub(super) fn started(&mut self, request: u64) {
        self.pending = Some(request);
    }

    /// Navigation can replace a page request without accepting its response.
    pub(super) fn cancel_superseded(&mut self, accepts: impl FnOnce(u64) -> bool, now: f64) {
        if self.pending.is_some_and(|request| !accepts(request)) {
            self.pending = None;
            self.postpone(now);
        }
    }

    pub(super) fn postpone(&mut self, now: f64) {
        self.focus_due = false;
        self.last_activity = now;
        self.next_at = now + self.delay();
    }

    pub(super) fn page_finished(&mut self, request: u64, succeeded: bool, now: f64) {
        if self.pending != Some(request) {
            // A manual refresh or a newly opened folder also proves the
            // connection recovered and makes an immediate poll redundant.
            if self.active && succeeded {
                self.failures = 0;
                self.postpone(now);
            }
            return;
        }
        self.pending = None;
        if succeeded {
            self.failures = 0;
        } else {
            self.failures = self.failures.saturating_add(1);
        }
        // After the failures are counted: they lengthen the wait.
        self.postpone(now);
    }

    pub(super) fn repaint_after(&self, now: f64, available: bool) -> Option<std::time::Duration> {
        (self.active && self.pending.is_none() && available)
            .then(|| std::time::Duration::from_secs_f64((self.next_at - now).max(MIN_REPAINT_WAIT)))
    }

    fn delay(&self) -> f64 {
        let interval = if self.focused {
            INTERVAL
        } else {
            BACKGROUND_INTERVAL
        };
        let doublings = self.failures.min(MAX_BACKOFF_DOUBLINGS);
        (interval * 2_f64.powi(doublings as i32)).min(MAX_BACKOFF)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_refresh() -> AutoRefresh {
        let mut refresh = AutoRefresh::default();
        assert!(!refresh.should_start(0.0, FOREGROUND));
        refresh
    }

    #[test]
    fn foreground_mail_is_refreshed_once_a_minute() {
        let mut refresh = active_refresh();

        assert!(!refresh.should_start(59.0, FOREGROUND));
        assert!(refresh.should_start(60.0, FOREGROUND));
        refresh.started(7);
        assert!(!refresh.should_start(61.0, FOREGROUND));

        refresh.page_finished(7, true, 61.0);
        assert!(!refresh.should_start(120.0, FOREGROUND));
        assert!(refresh.should_start(121.0, FOREGROUND));
    }

    #[test]
    fn returning_after_thirty_seconds_refreshes_immediately() {
        let mut refresh = active_refresh();

        assert!(!refresh.should_start(5.0, BACKGROUND));
        assert!(!refresh.should_start(34.0, BACKGROUND));
        assert!(refresh.should_start(35.0, FOREGROUND));
    }

    #[test]
    fn background_mail_runs_every_three_minutes_and_failures_back_off() {
        let mut refresh = active_refresh();
        assert!(!refresh.should_start(60.0, BACKGROUND));
        assert!(!refresh.should_start(120.0, BACKGROUND));

        assert!(refresh.repaint_after(120.0, true).is_some());
        assert!(refresh.should_start(180.0, BACKGROUND));
        refresh.started(8);
        refresh.page_finished(8, false, 181.0);

        assert!(!refresh.should_start(300.0, BACKGROUND));
        assert!(!refresh.should_start(301.0, FOREGROUND));
        assert!(!refresh.should_start(480.0, FOREGROUND));
        assert!(refresh.should_start(481.0, FOREGROUND));
    }

    #[test]
    fn failures_double_the_wait_up_to_five_minutes() {
        let mut refresh = active_refresh();
        // Each failed refresh doubles the minute: two, then four minutes.
        for (request, at) in [(1, 60.0), (2, 180.0), (3, 420.0)] {
            assert!(!refresh.should_start(at - 1.0, FOREGROUND), "{at}");
            assert!(refresh.should_start(at, FOREGROUND), "{at}");
            refresh.started(request);
            refresh.page_finished(request, false, at);
        }
        // Eight minutes would pass the longest wait.
        assert!(!refresh.should_start(719.0, FOREGROUND));
        assert!(refresh.should_start(720.0, FOREGROUND));
    }

    #[test]
    fn recent_background_success_avoids_a_redundant_focus_refresh() {
        let mut refresh = active_refresh();
        assert!(!refresh.should_start(5.0, BACKGROUND));
        assert!(refresh.should_start(180.0, BACKGROUND));
        refresh.started(8);
        assert!(!refresh.should_start(181.0, BACKGROUND));
        refresh.page_finished(8, true, 181.0);
        assert!(!refresh.should_start(185.0, FOREGROUND));
        assert!(refresh.should_start(241.0, FOREGROUND));
    }

    #[test]
    fn signing_out_and_busy_operations_stop_background_polls() {
        let mut refresh = active_refresh();
        assert!(!refresh.should_start(180.0, BUSY));
        assert!(refresh.repaint_after(180.0, false).is_none());
        assert!(!refresh.should_start(181.0, SIGNED_OUT));
        assert!(refresh.repaint_after(181.0, true).is_none());
        assert!(!refresh.should_start(182.0, BACKGROUND));
        assert!(!refresh.should_start(361.0, BACKGROUND));
        assert!(refresh.should_start(362.0, BACKGROUND));
    }

    #[test]
    fn a_successful_manual_refresh_clears_the_backoff() {
        let mut refresh = active_refresh();
        assert!(refresh.should_start(60.0, FOREGROUND));
        refresh.started(8);
        refresh.page_finished(8, false, 61.0);

        refresh.page_finished(99, true, 70.0);
        assert!(!refresh.should_start(129.0, FOREGROUND));
        assert!(refresh.should_start(130.0, FOREGROUND));
    }
}
