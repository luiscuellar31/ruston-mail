//! Reads the `Retry-After` header Proton sends with a 429.

/// The longest wait honored, in seconds: a longer `Retry-After` is cut to
/// this, and the request is still sent again once.
pub const MAX_RATE_LIMIT_WAIT: u64 = 30;

/// Parse a `Retry-After` header value (integer seconds only), capped.
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
