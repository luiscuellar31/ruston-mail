//! Decides when Proton conversations are shown as threads.
//! Split only proven independent inbound mail; otherwise keep Proton's grouping.

use std::collections::HashSet;

/// The user's own addresses, normalized.
pub(super) type OwnAddresses = HashSet<String>;

/// What the policy needs from one message of a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MessageFacts {
    /// Normalized sender address; empty when unknown.
    pub sender: String,
    /// The user replied to or forwarded this message.
    pub answered: bool,
}

pub(super) fn normalize_address(address: &str) -> String {
    address.trim().to_ascii_lowercase()
}

/// Whether a multi-message conversation needs metadata inspection.
pub(super) fn needs_inspection(message_count: i64, senders: &[String], own: &OwnAddresses) -> bool {
    message_count > 1 && matches!(senders, [sender] if !sender.is_empty() && !own.contains(sender))
}

/// True only when every message is independent inbound mail from the same
/// external sender and nothing shows that the user took part.
pub(super) fn is_repeated_inbound(messages: &[MessageFacts], own: &OwnAddresses) -> bool {
    let [first, rest @ ..] = messages else {
        return false;
    };

    !rest.is_empty()
        && !first.sender.is_empty()
        && !own.contains(&first.sender)
        && messages
            .iter()
            .all(|message| message.sender == first.sender && !message.answered)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BANK: &str = "notifications@bank.example.com";
    const FRIEND: &str = "friend@example.com";
    const ME: &str = "me@proton.me";
    const ME_ALIAS: &str = "me@pm.me";

    fn own() -> OwnAddresses {
        [ME, ME_ALIAS].into_iter().map(str::to_owned).collect()
    }

    fn from(sender: &str) -> MessageFacts {
        MessageFacts {
            sender: normalize_address(sender),
            answered: false,
        }
    }

    fn senders(addresses: &[&str]) -> Vec<String> {
        addresses.iter().map(|a| normalize_address(a)).collect()
    }

    #[test]
    fn single_message_stays_a_single_row() {
        assert!(!needs_inspection(1, &senders(&[BANK]), &own()));
        assert!(!is_repeated_inbound(&[from(BANK)], &own()));
    }

    #[test]
    fn repeated_inbound_mail_from_one_sender_is_split() {
        assert!(needs_inspection(3, &senders(&[BANK]), &own()));
        assert!(is_repeated_inbound(
            &[from(BANK), from(BANK), from(BANK)],
            &own()
        ));
    }

    #[test]
    fn back_and_forth_exchange_stays_grouped() {
        assert!(!is_repeated_inbound(
            &[from(FRIEND), from(ME), from(FRIEND)],
            &own()
        ));
    }

    #[test]
    fn conversation_started_by_the_user_stays_grouped() {
        assert!(!needs_inspection(2, &senders(&[ME, FRIEND]), &own()));
        assert!(!is_repeated_inbound(&[from(ME), from(FRIEND)], &own()));
    }

    #[test]
    fn self_conversation_stays_grouped() {
        assert!(!needs_inspection(2, &senders(&[ME]), &own()));
        assert!(!is_repeated_inbound(&[from(ME), from(ME_ALIAS)], &own()));
        assert!(!is_repeated_inbound(&[from(ME), from(ME)], &own()));
    }

    #[test]
    fn several_external_senders_stay_grouped() {
        assert!(!needs_inspection(2, &senders(&[BANK, FRIEND]), &own()));
        assert!(!is_repeated_inbound(&[from(BANK), from(FRIEND)], &own()));
    }

    #[test]
    fn answered_messages_stay_grouped() {
        let mut replied = from(BANK);
        replied.answered = true;

        assert!(!is_repeated_inbound(&[from(BANK), replied], &own()));
    }

    #[test]
    fn unknown_senders_stay_grouped() {
        assert!(!needs_inspection(2, &senders(&[""]), &own()));
        assert!(!needs_inspection(2, &[], &own()));
        assert!(!is_repeated_inbound(&[from(""), from("")], &own()));
    }

    #[test]
    fn addresses_compare_case_insensitively() {
        assert!(is_repeated_inbound(
            &[from(BANK), from(&BANK.to_uppercase())],
            &own()
        ));
        assert!(!needs_inspection(2, &senders(&["  ME@Proton.me "]), &own()));
    }
}
