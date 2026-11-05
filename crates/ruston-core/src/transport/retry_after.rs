//! Reads the `Retry-After` header Proton sends with a 429.

/// The longest wait honored, in seconds: a longer `Retry-After` is cut to
/// this, and the request is still sent again once.
pub const MAX_RATE_LIMIT_WAIT: u64 = 30;

/// The wait a `Retry-After` value asks for, in seconds, cut to
/// [`MAX_RATE_LIMIT_WAIT`]. Only a whole number of seconds is read; a date
/// is not.
pub fn parse(value: &str) -> Option<u64> {
    let secs: u64 = value.trim().parse().ok()?;
    Some(secs.min(MAX_RATE_LIMIT_WAIT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_caps() {
        assert_eq!(parse("5"), Some(5));
        assert_eq!(parse("999"), Some(30));
        assert_eq!(parse("0"), Some(0));
        assert_eq!(parse("abc"), None);
    }
}
