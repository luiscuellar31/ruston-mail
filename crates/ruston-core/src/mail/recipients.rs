//! Reading what was typed into a recipient field: the addresses to send to,
//! the pieces that do not look like one, and domains typed in another
//! script, which can imitate a familiar one. Every frontend reads fields
//! this way, so they all send to the same addresses.

use std::collections::HashSet;

/// Which domains a recipient field takes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Domains {
    /// Only domains written in ASCII. A domain spelled with letters from
    /// another script can pass for a familiar one, as `pаypal.com` with a
    /// Cyrillic "а" does; typed in its `xn--` form, the difference shows.
    #[default]
    Ascii,
    /// Domains in other scripts as well, each sent in its ASCII `xn--` form
    /// and listed beside it so a look-alike still shows.
    International,
}

/// A domain typed in another script, and the ASCII form it is sent in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternationalDomain {
    /// The domain as it was typed.
    pub typed: String,
    /// Its `xn--` form, which the message goes to.
    pub ascii: String,
}

/// What a recipient field came to: the addresses to send to, and the pieces
/// that do not look like an address, kept so the field can point at them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Recipients {
    /// The addresses to send to, each once, in the order typed.
    pub accepted: Vec<String>,
    /// The pieces that do not look like an address, each once, as typed.
    pub rejected: Vec<String>,
    /// Domains typed in another script, once each. The addresses they are
    /// part of are accepted in ASCII form.
    pub international: Vec<InternationalDomain>,
}

/// Reads comma- or semicolon-separated recipients and removes duplicates.
pub fn recipients(field: &str, domains: Domains) -> Recipients {
    let mut result = Recipients::default();
    // What was already seen, so a long field is read in one pass.
    let mut refused = HashSet::new();
    let mut accepted = HashSet::new();
    for piece in field
        .split([',', ';'])
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
    {
        let Some(address) = address(piece, domains) else {
            if refused.insert(piece) {
                result.rejected.push(piece.to_owned());
            }
            continue;
        };
        if let Some(domain) = address.international
            && !result.international.contains(&domain)
        {
            result.international.push(domain);
        }
        // The same address in another case is the same person.
        if accepted.insert(address.sent.to_ascii_lowercase()) {
            result.accepted.push(address.sent);
        }
    }

    result
}

/// An address as it leaves: as typed, except for a domain in another script.
struct Address {
    sent: String,
    international: Option<InternationalDomain>,
}

/// Rejects obvious mistakes without reimplementing the full address grammar.
fn address(piece: &str, domains: Domains) -> Option<Address> {
    if piece.chars().any(char::is_whitespace) {
        return None;
    }
    let (local, domain) = piece.split_once('@')?;
    if local.is_empty() || domain.contains('@') {
        return None;
    }
    if domain.is_ascii() {
        return is_domain(domain).then(|| Address {
            sent: piece.to_owned(),
            international: None,
        });
    }
    if domains == Domains::Ascii {
        return None;
    }
    let ascii = to_ascii(domain).filter(|ascii| is_domain(ascii))?;

    Some(Address {
        sent: format!("{local}@{ascii}"),
        international: Some(InternationalDomain {
            typed: domain.to_owned(),
            ascii,
        }),
    })
}

/// The ASCII form IDNA gives a domain in another script: `münchen.de` is sent
/// as `xn--mnchen-3ya.de`.
fn to_ascii(domain: &str) -> Option<String> {
    // The host parser decodes percent escapes before IDNA sees the domain,
    // which would send something other than what was typed.
    if domain.contains('%') {
        return None;
    }
    match url::Host::parse(domain).ok()? {
        url::Host::Domain(ascii) => Some(ascii),
        // Digits from another script can spell an IP address, not a domain.
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => None,
    }
}

fn is_domain(domain: &str) -> bool {
    domain.starts_with(|c: char| c.is_ascii_alphanumeric())
        && domain.ends_with(|c: char| c.is_ascii_alphanumeric())
        && domain.contains('.')
        && !domain.contains("..")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "pаypal.com" with a Cyrillic "а": it looks like PayPal's domain.
    const LOOK_ALIKE: &str = "p\u{430}ypal.com";
    /// "почта.рф", a real international domain.
    const CYRILLIC: &str = "\u{43f}\u{43e}\u{447}\u{442}\u{430}.\u{440}\u{444}";

    #[test]
    fn a_field_can_name_several_people() {
        let read = recipients(
            "alex@example.com, sam@example.org; jo@example.net",
            Domains::Ascii,
        );

        assert_eq!(
            read.accepted,
            ["alex@example.com", "sam@example.org", "jo@example.net"]
        );
        assert!(read.rejected.is_empty());
    }

    #[test]
    fn the_same_person_is_only_written_to_once() {
        // However it was typed: a duplicate is a slip, not an instruction.
        let read = recipients(
            "alex@example.com, Alex@Example.com , alex@example.com",
            Domains::Ascii,
        );

        assert_eq!(read.accepted, ["alex@example.com"]);
    }

    #[test]
    fn what_is_not_an_address_is_kept_to_be_pointed_at() {
        // Dropping it silently would send the mail to fewer people than the
        // sender believes, which is worse than refusing.
        let read = recipients(
            "alex@example.com, not-an-address, @nowhere, a@b",
            Domains::Ascii,
        );

        assert_eq!(read.accepted, ["alex@example.com"]);
        assert_eq!(read.rejected, ["not-an-address", "@nowhere", "a@b"]);
    }

    #[test]
    fn look_alike_domains_from_other_scripts_are_refused() {
        let read = recipients(
            &format!("ceo@{LOOK_ALIKE}, ivan@{CYRILLIC}"),
            Domains::Ascii,
        );
        assert!(read.accepted.is_empty());
        assert_eq!(read.rejected.len(), 2);

        // The same international domain typed in its xn-- form is accepted,
        // and so is a local part with accents.
        let read = recipients(
            "ivan@xn--80a1acny.xn--p1ai, jos\u{e9}@example.com",
            Domains::Ascii,
        );
        assert!(read.rejected.is_empty());
        assert_eq!(read.accepted.len(), 2);
        assert!(read.international.is_empty());
    }

    #[test]
    fn international_domains_leave_in_ascii_beside_what_was_typed() {
        let read = recipients(
            &format!("ana@M\u{fc}nchen.de, ceo@{LOOK_ALIKE}; ivan@{CYRILLIC}"),
            Domains::International,
        );

        assert!(read.rejected.is_empty());
        assert_eq!(
            read.accepted,
            [
                "ana@xn--mnchen-3ya.de",
                "ceo@xn--pypal-4ve.com",
                "ivan@xn--80a1acny.xn--p1ai",
            ]
        );
        // The look-alike is named next to a form that is plainly not paypal.com.
        let pairs: Vec<(&str, &str)> = read
            .international
            .iter()
            .map(|domain| (domain.typed.as_str(), domain.ascii.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("M\u{fc}nchen.de", "xn--mnchen-3ya.de"),
                (LOOK_ALIKE, "xn--pypal-4ve.com"),
                (CYRILLIC, "xn--80a1acny.xn--p1ai"),
            ]
        );
    }

    #[test]
    fn an_international_domain_and_its_ascii_form_are_one_recipient() {
        let read = recipients(
            &format!("ivan@{CYRILLIC}, ivan@xn--80a1acny.xn--p1ai, ivan@{CYRILLIC}"),
            Domains::International,
        );

        assert_eq!(read.accepted, ["ivan@xn--80a1acny.xn--p1ai"]);
        assert_eq!(read.international.len(), 1);
        // ASCII domains are sent exactly as typed.
        let read = recipients("Alex@Example.com", Domains::International);
        assert_eq!(read.accepted, ["Alex@Example.com"]);
        assert!(read.international.is_empty());
    }

    #[test]
    fn a_domain_that_does_not_convert_cleanly_is_refused() {
        for piece in [
            // Percent escapes would be decoded into something not typed.
            "a@m\u{fc}nchen%2Ede",
            // Full-width digits spell an IP address, not a domain.
            "a@\u{ff11}\u{ff12}\u{ff17}.\u{ff10}.\u{ff10}.\u{ff11}",
            // A full-width "@" turns into a second one.
            "a@m\u{fc}nchen\u{ff20}example.de",
            "a@m\u{fc}nchen",
            "a@m\u{fc}nchen..de",
            "a@.m\u{fc}nchen.de",
            "a@m\u{fc}nchen.de.",
            "a@m\u{fc}n chen.de",
        ] {
            let read = recipients(piece, Domains::International);
            assert!(read.accepted.is_empty(), "{piece} was accepted");
            assert!(read.international.is_empty(), "{piece} was converted");
        }
    }

    #[test]
    fn an_empty_field_names_nobody() {
        for domains in [Domains::Ascii, Domains::International] {
            assert_eq!(recipients("", domains), Recipients::default());
            assert_eq!(recipients("  ,  ; ", domains), Recipients::default());
        }
    }

    #[test]
    fn ordinary_addresses_are_not_turned_away() {
        for address in [
            "a@b.co",
            "first.last@example.co.uk",
            "user+tag@example.com",
            "user_name@sub.example.org",
            "1@2.com",
        ] {
            for domains in [Domains::Ascii, Domains::International] {
                assert_eq!(
                    recipients(address, domains).accepted,
                    [address],
                    "{address} was refused"
                );
            }
        }
    }
}
