//! Foreground refresh timing, independent of rendering and network work.

const INTERVAL: f64 = 60.0;
const AFTER_IDLE: f64 = 30.0;
const MAX_BACKOFF: f64 = 300.0;

/// Schedules mailbox refreshes using egui's monotonic clock. Network work
/// still belongs to `App`; this only decides when to ask for it.
#[derive(Debug, Default)]
pub(super) struct AutoRefresh {
    active: bool,
    focused: bool,
    away_since: Option<f64>,
    focus_due: bool,
    next_at: f64,
    pending: Option<u64>,
    failures: u32,
}

impl AutoRefresh {
    pub(super) fn should_start(
        &mut self,
        now: f64,
        focused: bool,
        active: bool,
        available: bool,
    ) -> bool {
        if !active {
            *self = Self::default();
            return false;
        }
        if !self.active {
            self.active = true;
            self.focused = focused;
            self.away_since = (!focused).then_some(now);
            self.next_at = now + INTERVAL;
            return false;
        }

        if !focused {
            if self.focused {
                self.away_since = Some(now);
            }
            self.focused = false;
            return false;
        }
        if !self.focused {
            self.focus_due = self.away_since.is_some_and(|away| now - away >= AFTER_IDLE);
        }
        self.focused = true;
        self.away_since = None;

        if self.pending.is_some() || !available {
            return false;
        }
        let focus_allowed = self.failures == 0 || now >= self.next_at;
        if now >= self.next_at || self.focus_due && focus_allowed {
            self.focus_due = false;
            return true;
        }

        false
    }

    pub(super) fn started(&mut self, request: u64) {
        self.pending = Some(request);
    }

    pub(super) fn postpone(&mut self, now: f64) {
        self.focus_due = false;
        self.next_at = now + self.delay();
    }

    pub(super) fn page_finished(&mut self, request: u64, succeeded: bool, now: f64) {
        if self.pending != Some(request) {
            // A manual refresh or a newly opened folder also proves the
            // connection recovered and makes an immediate poll redundant.
            if self.active && succeeded {
                self.failures = 0;
                self.focus_due = false;
                self.next_at = now + INTERVAL;
            }
            return;
        }
        self.pending = None;
        if succeeded {
            self.failures = 0;
        } else {
            self.failures = self.failures.saturating_add(1);
        }
        self.next_at = now + self.delay();
    }

    pub(super) fn repaint_after(&self, now: f64, available: bool) -> Option<std::time::Duration> {
        (self.active && self.focused && self.pending.is_none() && available)
            .then(|| std::time::Duration::from_secs_f64((self.next_at - now).max(0.05)))
    }

    fn delay(&self) -> f64 {
        (INTERVAL * 2_f64.powi(self.failures.min(3) as i32)).min(MAX_BACKOFF)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_refresh() -> AutoRefresh {
        let mut refresh = AutoRefresh::default();
        assert!(!refresh.should_start(0.0, true, true, true));
        refresh
    }

    #[test]
    fn foreground_mail_is_refreshed_once_a_minute() {
        let mut refresh = active_refresh();

        assert!(!refresh.should_start(59.0, true, true, true));
        assert!(refresh.should_start(60.0, true, true, true));
        refresh.started(7);
        assert!(!refresh.should_start(61.0, true, true, true));

        refresh.page_finished(7, true, 61.0);
        assert!(!refresh.should_start(120.0, true, true, true));
        assert!(refresh.should_start(121.0, true, true, true));
    }

    #[test]
    fn returning_after_thirty_seconds_refreshes_immediately() {
        let mut refresh = active_refresh();

        assert!(!refresh.should_start(5.0, false, true, true));
        assert!(!refresh.should_start(34.0, false, true, true));
        assert!(refresh.should_start(35.0, true, true, true));
    }

    #[test]
    fn background_mail_is_paused_and_failures_back_off() {
        let mut refresh = active_refresh();
        assert!(!refresh.should_start(60.0, false, true, true));
        assert!(!refresh.should_start(120.0, false, true, true));

        assert!(refresh.should_start(120.0, true, true, true));
        refresh.started(8);
        refresh.page_finished(8, false, 121.0);

        assert!(!refresh.should_start(150.0, false, true, true));
        assert!(!refresh.should_start(160.0, true, true, true));
        assert!(!refresh.should_start(240.0, true, true, true));
        assert!(refresh.should_start(241.0, true, true, true));
    }

    #[test]
    fn a_successful_manual_refresh_clears_the_backoff() {
        let mut refresh = active_refresh();
        assert!(refresh.should_start(60.0, true, true, true));
        refresh.started(8);
        refresh.page_finished(8, false, 61.0);

        refresh.page_finished(99, true, 70.0);
        assert!(!refresh.should_start(129.0, true, true, true));
        assert!(refresh.should_start(130.0, true, true, true));
    }
}
