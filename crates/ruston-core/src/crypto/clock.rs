//! The time OpenPGP runs at: this machine's clock, set right by what Proton's
//! API says its own reads. A signature dated by a correct clock then never
//! looks made in the future here, and ours never look so to others.

use std::sync::Once;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use proton_crypto::CryptoClockProvider;
use proton_crypto::crypto::UnixTimestamp;

/// How many milliseconds Proton's clock runs ahead of this machine's; less
/// than zero when it runs behind.
static PROTON_AHEAD_MS: AtomicI64 = AtomicI64::new(0);

/// Takes Proton's time from the `Date` of a response to a request this machine
/// sent at `sent` and heard back on at `received`. From then on every OpenPGP
/// operation in the process runs at it, through proton-crypto's shared clock.
pub(crate) fn note_proton_date(date: &str, sent: SystemTime, received: SystemTime) {
    let Some(date) = http_date_secs(date) else {
        return;
    };
    let ahead = proton_ahead_ms(date, unix_ms(sent), unix_ms(received));
    PROTON_AHEAD_MS.store(ahead, Ordering::Relaxed);
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| proton_crypto::crypto_clock().set_provider(Box::new(ProtonTime)));
}

/// How far Proton's clock runs ahead, given that it stamped `date_secs` on a
/// response between `sent_ms` and `received_ms` on this machine's clock. Only
/// what the stamp proves counts: within the round trip, and the second it is
/// rounded down to, the two clocks are taken to agree.
fn proton_ahead_ms(date_secs: i64, sent_ms: i64, received_ms: i64) -> i64 {
    let date_ms = date_secs.saturating_mul(1000);
    let least = date_ms.saturating_sub(received_ms);
    let most = date_ms.saturating_add(999).saturating_sub(sent_ms);
    if least > 0 {
        least
    } else if most < 0 {
        most
    } else {
        0
    }
}

/// The seconds since the epoch an HTTP `Date` names, as in
/// `Sun, 06 Nov 1994 08:49:37 GMT`.
fn http_date_secs(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|date| date.timestamp())
}

fn unix_ms(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
    }
}

/// proton-crypto's clock, reading this machine's time moved to Proton's.
#[derive(Debug)]
struct ProtonTime;

impl CryptoClockProvider for ProtonTime {
    fn unix_time(&self) -> UnixTimestamp {
        let now_ms =
            unix_ms(SystemTime::now()).saturating_add(PROTON_AHEAD_MS.load(Ordering::Relaxed));
        UnixTimestamp::new(u64::try_from(now_ms.div_euclid(1000)).unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks_that_agree_within_what_the_date_can_tell_are_left_alone() {
        // Stamped 1000.x s, heard back on between 999.4 s and 1000.6 s.
        assert_eq!(proton_ahead_ms(1000, 999_400, 1_000_600), 0);
        // A slow answer cannot tell a clock a few seconds off either.
        assert_eq!(proton_ahead_ms(1003, 999_000, 1_005_000), 0);
    }

    #[test]
    fn a_clock_off_moves_by_no_more_than_the_date_proves() {
        // An hour behind Proton: at least 3599.5 s, at most 3601 s.
        assert_eq!(proton_ahead_ms(4600, 1_000_000, 1_000_500), 3_599_500);
        // An hour ahead: at least 3599 s, at most 3600.5 s behind.
        assert_eq!(proton_ahead_ms(1000, 4_600_000, 4_600_500), -3_599_001);
    }

    #[test]
    fn a_clock_turned_back_meanwhile_still_gives_an_answer() {
        // Heard back on before it was sent: no span holds both bounds.
        assert_eq!(proton_ahead_ms(1000, 1_005_000, 995_000), 5_000);
        assert_eq!(proton_ahead_ms(1000, 1_010_000, 1_008_000), -9_001);
    }

    #[test]
    fn reads_an_http_date() {
        assert_eq!(
            http_date_secs("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(784_111_777)
        );
        assert_eq!(http_date_secs("yesterday"), None);
        assert_eq!(http_date_secs(""), None);
    }

    #[test]
    fn times_before_the_epoch_count_back_from_it() {
        assert_eq!(
            unix_ms(UNIX_EPOCH + std::time::Duration::from_millis(1500)),
            1500
        );
        assert_eq!(
            unix_ms(UNIX_EPOCH - std::time::Duration::from_millis(1500)),
            -1500
        );
    }
}
