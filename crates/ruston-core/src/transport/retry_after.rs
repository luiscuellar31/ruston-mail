//! Reads the `Retry-After` header Proton sends with a 429 or a 503, and how
//! long the transport waits before sending such a request again.

use std::time::Duration;

/// The longest `Retry-After` honored, in seconds. A request asked to wait
/// longer is not sent again: its error comes back at once, as Proton's web
/// client gives up when asked for 10 seconds or more, instead of being sent
/// before Proton said it would take it.
pub const MAX_WAIT: u64 = 30;

/// The wait when Proton gives no `Retry-After`: long enough not to ask a
/// busy Proton again at once, short enough that a request's three retries
/// wait 15 seconds in all. go-proton-api waits 10 to 19 seconds.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(5);

/// The wait a `Retry-After` value asks for, in seconds. Only a whole number
/// of seconds is read; a date is not.
pub fn parse(value: &str) -> Option<u64> {
    value.trim().parse().ok()
}

/// How long to wait before sending a request again after Proton answered
/// it busy, asking to wait `retry_after` seconds: [`DEFAULT_WAIT`] when it
/// does not say, and `None`, not to send it again, when it asks for more
/// than [`MAX_WAIT`].
pub fn wait(retry_after: Option<u64>) -> Option<Duration> {
    match retry_after {
        None => Some(DEFAULT_WAIT),
        Some(secs) if secs <= MAX_WAIT => Some(Duration::from_secs(secs)),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whole_seconds() {
        assert_eq!(parse("5"), Some(5));
        assert_eq!(parse(" 999 "), Some(999));
        assert_eq!(parse("0"), Some(0));
        assert_eq!(parse("abc"), None);
        assert_eq!(parse("Wed, 21 Oct 2026 07:28:00 GMT"), None);
    }

    #[test]
    fn waits_what_is_asked_up_to_the_cap() {
        assert_eq!(wait(Some(0)), Some(Duration::ZERO));
        assert_eq!(wait(Some(MAX_WAIT)), Some(Duration::from_secs(MAX_WAIT)));
        assert_eq!(wait(Some(MAX_WAIT + 1)), None);
        assert_eq!(wait(None), Some(DEFAULT_WAIT));
    }
}
