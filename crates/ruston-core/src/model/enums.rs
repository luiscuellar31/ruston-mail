//! Load-bearing Proton Mail enums and constants (verified against the spec §7.4).

/// System mailbox label IDs (string values, as the API uses them).
pub mod label_ids {
    /// Inbox.
    pub const INBOX: &str = "0";
    /// All drafts (across every folder).
    pub const ALL_DRAFTS: &str = "1";
    /// All sent messages (across every folder).
    pub const ALL_SENT: &str = "2";
    /// Trash.
    pub const TRASH: &str = "3";
    /// Spam.
    pub const SPAM: &str = "4";
    /// All mail.
    pub const ALL_MAIL: &str = "5";
    /// Archive.
    pub const ARCHIVE: &str = "6";
    /// Sent.
    pub const SENT: &str = "7";
    /// Drafts.
    pub const DRAFTS: &str = "8";
    /// Outbox.
    pub const OUTBOX: &str = "9";
    /// Starred.
    pub const STARRED: &str = "10";
    /// Scheduled send.
    pub const SCHEDULED: &str = "12";
    /// Almost all mail (everything except Spam and Trash).
    pub const ALMOST_ALL_MAIL: &str = "15";
    /// Snoozed.
    pub const SNOOZED: &str = "16";

    /// Every system label with the name the CLI shows for it and takes in
    /// its folder options.
    const NAMES: [(&str, &str); 14] = [
        (INBOX, "inbox"),
        (ALL_DRAFTS, "all-drafts"),
        (ALL_SENT, "all-sent"),
        (TRASH, "trash"),
        (SPAM, "spam"),
        (ALL_MAIL, "all-mail"),
        (ARCHIVE, "archive"),
        (SENT, "sent"),
        (DRAFTS, "drafts"),
        (OUTBOX, "outbox"),
        (STARRED, "starred"),
        (SCHEDULED, "scheduled"),
        (ALMOST_ALL_MAIL, "almost-all"),
        (SNOOZED, "snoozed"),
    ];

    /// The name of a system label, or `None` for any other label ID.
    pub fn name(id: &str) -> Option<&'static str> {
        NAMES
            .iter()
            .find(|(label, _)| *label == id)
            .map(|(_, name)| *name)
    }

    /// The system label with this lowercase name.
    pub(super) fn by_name(name: &str) -> Option<&'static str> {
        NAMES
            .iter()
            .find(|(_, label_name)| *label_name == name)
            .map(|(label, _)| *label)
    }
}

/// Resolve a human folder name to its label ID. Unknown names pass through
/// unchanged (so raw label IDs work anywhere).
pub fn resolve_folder(name: &str) -> String {
    let folded = name.trim().to_ascii_lowercase();
    let canonical = match folded.as_str() {
        "all" | "allmail" => "all-mail",
        "almost-all-mail" => "almost-all",
        other => other,
    };
    match label_ids::by_name(canonical) {
        Some(id) => id.to_string(),
        // The name is matched folded, but an unknown one is returned as it
        // was given: Proton's IDs are case-sensitive, so folding one names a
        // label that does not exist.
        None => name.trim().to_string(),
    }
}

/// Send package types (bitmask).
pub mod package_type {
    /// Internal Proton (end-to-end encrypted).
    pub const PM: i64 = 1; // internal Proton
    /// Encrypted-outside (password-protected).
    pub const EO: i64 = 2; // encrypted-outside (password)
    /// Cleartext (unencrypted).
    pub const CLEAR: i64 = 4;
    /// PGP inline.
    pub const PGP_INLINE: i64 = 8;
    /// PGP/MIME.
    pub const PGP_MIME: i64 = 16;
    /// Cleartext MIME.
    pub const CLEAR_MIME: i64 = 32;
}

/// Address-key flags (bitmask).
pub mod key_flag {
    /// Key is not compromised (may verify signatures).
    pub const NOT_COMPROMISED: u32 = 1; // may verify signatures
    /// Key is not obsolete (may encrypt).
    pub const NOT_OBSOLETE: u32 = 2; // may encrypt
    /// Email encryption is disabled for this address.
    pub const EMAIL_NO_ENCRYPT: u32 = 4;
    /// Email signing is disabled for this address.
    pub const EMAIL_NO_SIGN: u32 = 8;

    /// True if the key is not obsolete, so it may encrypt. Whether mail to
    /// its address may be encrypted is [`can_encrypt_mail`].
    pub fn can_encrypt(flags: u32) -> bool {
        flags & NOT_OBSOLETE != 0
    }

    /// True if mail may be encrypted to the key: it may encrypt, and the
    /// address's owner has not turned email encryption off, as Proton does
    /// for an address that forwards to another provider.
    pub fn can_encrypt_mail(flags: u32) -> bool {
        can_encrypt(flags) && flags & EMAIL_NO_ENCRYPT == 0
    }

    /// True if signatures made with the key may be trusted: Proton has not
    /// marked it as compromised.
    pub fn can_verify(flags: u32) -> bool {
        flags & NOT_COMPROMISED != 0
    }
}

/// Message flags (bitmask; `u64` because high bits overflow 32-bit).
pub mod message_flag {
    /// Message was received.
    pub const RECEIVED: u64 = 1 << 0;
    /// Message was sent.
    pub const SENT: u64 = 1 << 1;
    /// Message is internal (Proton-to-Proton).
    pub const INTERNAL: u64 = 1 << 2;
    /// Message is end-to-end encrypted.
    pub const E2E: u64 = 1 << 3;
    /// Message has been replied to.
    pub const REPLIED: u64 = 1 << 5;
    /// Message has been replied to all.
    pub const REPLIED_ALL: u64 = 1 << 6;
    /// Message has been forwarded.
    pub const FORWARDED: u64 = 1 << 7;
    /// Message carries a public key.
    pub const PUBLIC_KEY: u64 = 1 << 17;
    /// Message is signed.
    pub const SIGN: u64 = 1 << 18;
    /// Message is a scheduled send.
    pub const SCHEDULED_SEND: u64 = 1 << 20;
    /// Message was produced by auto-forwarding.
    pub const AUTO_FORWARDEE: u64 = 1 << 35;

    /// True if `flags` has `flag` set.
    pub fn has(flags: u64, flag: u64) -> bool {
        flags & flag != 0
    }
}

/// How a draft answers its parent, which Proton records on the parent once
/// the draft is sent ([`message_flag::REPLIED`], [`message_flag::REPLIED_ALL`]
/// or [`message_flag::FORWARDED`]).
pub mod message_action {
    /// A reply to the sender.
    pub const REPLY: u8 = 0;
    /// A reply to everyone the parent reached.
    pub const REPLY_ALL: u8 = 1;
    /// A forward.
    pub const FORWARD: u8 = 2;
}

/// The second factors an account can have, as bits of its 2FA `Enabled`
/// mask.
pub mod two_factor {
    /// Codes from an authenticator app (TOTP).
    pub const TOTP: u32 = 1;
    /// A FIDO2/WebAuthn security key.
    pub const SECURITY_KEY: u32 = 2;
}

/// How an account's passwords are set up, as its `PasswordMode` says.
pub mod password_mode {
    /// One password signs in and opens the mailbox.
    pub const ONE_PASSWORD: u8 = 1;
    /// A separate mailbox password opens the keys.
    pub const TWO_PASSWORDS: u8 = 2;
}

/// Recipient type from the public-key lookup.
pub mod recipient_type {
    /// Internal Proton recipient.
    pub const INTERNAL: u8 = 1;
    /// External recipient.
    pub const EXTERNAL: u8 = 2;
}

/// Label types for `/core/v4/labels`.
pub mod label_type {
    /// Message label.
    pub const MESSAGE_LABEL: i64 = 1;
    /// Contact group.
    pub const CONTACT_GROUP: i64 = 2;
    /// Message folder (user-created).
    pub const MESSAGE_FOLDER: i64 = 3;
    /// System folder.
    pub const SYSTEM_FOLDER: i64 = 4;
}

/// Human verification: where a person solves a challenge, and how.
pub mod human_verification {
    /// Proton's verification page, for a challenge that names no host of its
    /// own.
    pub const PAGE: &str = "https://verify.proton.me";
    /// The domain of Proton's own verification hosts.
    pub const DOMAIN: &str = "proton.me";
    /// The method a browser outside the app can complete, named again with
    /// the token that answers it.
    pub const CAPTCHA: &str = "captcha";
}

/// Server-side Sieve filters.
pub mod filter {
    /// The Sieve version filters are written in.
    pub const SIEVE_VERSION: i64 = 2;
    /// The status of a filter that applies to new mail.
    pub const ENABLED: i64 = 1;
}

/// The `Code` of a Proton response, for the codes the client acts on.
pub(crate) mod response_code {
    /// The request succeeded.
    pub(crate) const SUCCESS: i64 = 1000;
    /// A request about several items succeeded for each of them.
    pub(crate) const MULTI_SUCCESS: i64 = 1001;
    /// The code some endpoints send with a successful HTTP status.
    pub(crate) const NONE: i64 = 0;
    /// Proton does not accept the ID.
    pub(crate) const INVALID_ID: i64 = 2061;
    /// The message does not exist.
    pub(crate) const MESSAGE_DOES_NOT_EXIST: i64 = 2501;
    /// The request needs human verification first.
    pub(crate) const HUMAN_VERIFICATION: i64 = 9001;
}

/// MIME body types used by mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MimeType {
    /// `text/plain`.
    PlainText,
    /// `text/html`.
    Html,
    /// `multipart/mixed`.
    MultipartMixed,
}

impl MimeType {
    /// The MIME type's wire string (e.g. `text/plain`).
    pub fn as_str(self) -> &'static str {
        match self {
            MimeType::PlainText => "text/plain",
            MimeType::Html => "text/html",
            MimeType::MultipartMixed => "multipart/mixed",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_resolution() {
        assert_eq!(resolve_folder("inbox"), "0");
        assert_eq!(resolve_folder("INBOX"), "0");
        assert_eq!(resolve_folder("trash"), "3");
        assert_eq!(resolve_folder("sent"), "7");
        assert_eq!(resolve_folder("starred"), "10");
        assert_eq!(resolve_folder("all"), "5");
        assert_eq!(resolve_folder("almost-all"), "15");
        assert_eq!(resolve_folder("xyz123"), "xyz123");
    }

    #[test]
    fn a_raw_label_id_passes_through_unchanged() {
        // Proton's IDs are case-sensitive base64url, so folding one to lower
        // case names a label that does not exist. Only all-lowercase IDs used
        // to survive, which is why listing a folder or label the account made
        // came back empty.
        let id = "qBIcv1_Wv5X4hLpEo0Tz9A==";
        assert_eq!(resolve_folder(id), id);

        // A name is still recognised however it is typed or spaced.
        assert_eq!(resolve_folder("  Archive  "), label_ids::ARCHIVE);
    }

    #[test]
    fn every_system_label_has_a_name_that_resolves_back_to_it() {
        use label_ids::*;
        for id in [
            INBOX,
            ALL_DRAFTS,
            ALL_SENT,
            TRASH,
            SPAM,
            ALL_MAIL,
            ARCHIVE,
            SENT,
            DRAFTS,
            OUTBOX,
            STARRED,
            SCHEDULED,
            ALMOST_ALL_MAIL,
            SNOOZED,
        ] {
            let name = label_ids::name(id).unwrap_or_else(|| panic!("label {id} has no name"));
            assert_eq!(resolve_folder(name), id, "{name}");
            assert_eq!(resolve_folder(&name.to_uppercase()), id, "{name}");
        }
        assert_eq!(label_ids::name("qBIcv1_Wv5X4hLpEo0Tz9A=="), None);
        // The older spellings still resolve.
        assert_eq!(resolve_folder("allmail"), ALL_MAIL);
        assert_eq!(resolve_folder("almost-all-mail"), ALMOST_ALL_MAIL);
    }

    #[test]
    fn flag_values() {
        assert_eq!(key_flag::NOT_OBSOLETE, 2);
        assert!(key_flag::can_encrypt(
            key_flag::NOT_OBSOLETE | key_flag::NOT_COMPROMISED
        ));
        assert!(!key_flag::can_encrypt(key_flag::NOT_COMPROMISED));
        let usable = key_flag::NOT_OBSOLETE | key_flag::NOT_COMPROMISED;
        assert!(key_flag::can_encrypt_mail(usable | key_flag::EMAIL_NO_SIGN));
        assert!(!key_flag::can_encrypt_mail(
            usable | key_flag::EMAIL_NO_ENCRYPT
        ));
        assert!(!key_flag::can_encrypt_mail(key_flag::NOT_COMPROMISED));
        assert!(key_flag::can_verify(key_flag::NOT_COMPROMISED));
        assert!(!key_flag::can_verify(key_flag::NOT_OBSOLETE));
        assert_eq!(package_type::PM, 1);
        assert_eq!(package_type::CLEAR, 4);
        assert_eq!(message_flag::AUTO_FORWARDEE, 1 << 35);
        assert!(message_flag::has(
            message_flag::AUTO_FORWARDEE,
            message_flag::AUTO_FORWARDEE
        ));
    }
}
