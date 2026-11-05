//! Proton Mail's own values, as its API sends and takes them: the IDs of
//! the labels every mailbox has, bit flags, package and label types, and
//! response codes. Each is written here once, for both frontends to share.

/// The IDs of the labels every mailbox has. Proton sends them as strings,
/// like the IDs of the labels an account makes.
pub mod label_ids {
    /// The inbox.
    pub const INBOX: &str = "0";
    /// Every draft, whichever folder it is in.
    pub const ALL_DRAFTS: &str = "1";
    /// Every message the account sent, whichever folder it is in.
    pub const ALL_SENT: &str = "2";
    /// Trash.
    pub const TRASH: &str = "3";
    /// Spam.
    pub const SPAM: &str = "4";
    /// All mail, Trash and Spam included.
    pub const ALL_MAIL: &str = "5";
    /// The archive.
    pub const ARCHIVE: &str = "6";
    /// The Sent folder.
    pub const SENT: &str = "7";
    /// The Drafts folder.
    pub const DRAFTS: &str = "8";
    /// The outbox.
    pub const OUTBOX: &str = "9";
    /// Starred mail.
    pub const STARRED: &str = "10";
    /// Mail scheduled to be sent later.
    pub const SCHEDULED: &str = "12";
    /// All mail except what is in Trash or Spam.
    pub const ALMOST_ALL_MAIL: &str = "15";
    /// Snoozed mail.
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

/// The label ID that `name` stands for: a system folder's name, such as
/// `inbox` or `all-mail`, in any case, gives its ID, and anything else is
/// kept, trimmed, as a label ID, so an ID works wherever a name does.
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

/// How the copies of a sent message travel, as the `Type` of the package
/// that carries them: one bit for each way.
pub mod package_type {
    /// To Proton addresses, end-to-end encrypted.
    pub const PM: i64 = 1;
    /// Behind a message password, for recipients without a key.
    pub const EO: i64 = 2;
    /// Without end-to-end encryption.
    pub const CLEAR: i64 = 4;
    /// Encrypted to the recipient's own PGP key, as inline PGP.
    pub const PGP_INLINE: i64 = 8;
    /// Encrypted to the recipient's own PGP key, as PGP/MIME.
    pub const PGP_MIME: i64 = 16;
    /// Without end-to-end encryption, as one MIME body.
    pub const CLEAR_MIME: i64 = 32;
}

/// What an address key may be used for, as bits of its `Flags`.
pub mod key_flag {
    /// Proton has not marked the key compromised, so its signatures count.
    pub const NOT_COMPROMISED: u32 = 1;
    /// The key is not obsolete, so it may encrypt.
    pub const NOT_OBSOLETE: u32 = 2;
    /// Mail may not be encrypted to the key, as when its address forwards
    /// mail to another provider.
    pub const EMAIL_NO_ENCRYPT: u32 = 4;
    /// Mail may not be signed with the key.
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

/// What Proton records about a message, as bits of its `Flags`. Some lie
/// above the 32nd, so they are `u64`.
pub mod message_flag {
    /// It reached the account.
    pub const RECEIVED: u64 = 1 << 0;
    /// The account sent it.
    pub const SENT: u64 = 1 << 1;
    /// It went from one Proton address to another.
    pub const INTERNAL: u64 = 1 << 2;
    /// It was end-to-end encrypted.
    pub const E2E: u64 = 1 << 3;
    /// It has been answered.
    pub const REPLIED: u64 = 1 << 5;
    /// It has been answered to everyone it reached.
    pub const REPLIED_ALL: u64 = 1 << 6;
    /// It has been forwarded.
    pub const FORWARDED: u64 = 1 << 7;
    /// It carries its sender's public key.
    pub const PUBLIC_KEY: u64 = 1 << 17;
    /// It is signed.
    pub const SIGN: u64 = 1 << 18;
    /// It waits to be sent at a later time.
    pub const SCHEDULED_SEND: u64 = 1 << 20;
    /// It reached the account through automatic forwarding.
    pub const AUTO_FORWARDEE: u64 = 1 << 35;

    /// Whether `flags` has any bit of `flag` set.
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

/// Whose address a key lookup was for, as its `RecipientType` says.
pub mod recipient_type {
    /// A Proton address.
    pub const INTERNAL: u8 = 1;
    /// Another provider's address.
    pub const EXTERNAL: u8 = 2;
}

/// The kinds of label `/core/v4/labels` keeps, by its `Type`.
pub mod label_type {
    /// A label for messages.
    pub const MESSAGE_LABEL: i64 = 1;
    /// A group of contacts.
    pub const CONTACT_GROUP: i64 = 2;
    /// A folder the account made.
    pub const MESSAGE_FOLDER: i64 = 3;
    /// A folder every mailbox has.
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

/// The kinds of body a message has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MimeType {
    /// Plain text, `text/plain`.
    PlainText,
    /// HTML, `text/html`.
    Html,
    /// Several parts in one MIME body, `multipart/mixed`.
    MultipartMixed,
}

impl MimeType {
    /// The MIME type as Proton writes it, such as `text/plain`.
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
    fn each_folder_name_resolves_to_the_id_proton_gives_it() {
        // The IDs are written out, so a constant that changed would show.
        let ids = [
            ("inbox", "0"),
            ("all-drafts", "1"),
            ("all-sent", "2"),
            ("trash", "3"),
            ("spam", "4"),
            ("all-mail", "5"),
            ("archive", "6"),
            ("sent", "7"),
            ("drafts", "8"),
            ("outbox", "9"),
            ("starred", "10"),
            ("scheduled", "12"),
            ("almost-all", "15"),
            ("snoozed", "16"),
            ("all", "5"),
        ];
        for (name, id) in ids {
            assert_eq!(resolve_folder(name), id, "{name}");
            assert_eq!(resolve_folder(&name.to_ascii_uppercase()), id, "{name}");
        }
        // Anything else is taken for an ID of the account's own.
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
    fn key_and_message_flags_read_as_proton_sets_them() {
        // The values are written out, so a constant that changed would show.
        let key_bits = [
            key_flag::NOT_COMPROMISED,
            key_flag::NOT_OBSOLETE,
            key_flag::EMAIL_NO_ENCRYPT,
            key_flag::EMAIL_NO_SIGN,
        ];
        assert_eq!(key_bits, [1, 2, 4, 8]);
        let package_types = [
            package_type::PM,
            package_type::EO,
            package_type::CLEAR,
            package_type::PGP_INLINE,
            package_type::PGP_MIME,
            package_type::CLEAR_MIME,
        ];
        assert_eq!(package_types, [1, 2, 4, 8, 16, 32]);

        let sound = key_flag::NOT_COMPROMISED | key_flag::NOT_OBSOLETE;
        assert!(key_flag::can_verify(sound) && key_flag::can_encrypt_mail(sound));
        // An obsolete key still vouches for its signatures, but encrypts
        // nothing; a compromised one vouches for nothing.
        assert!(key_flag::can_verify(key_flag::NOT_COMPROMISED));
        assert!(!key_flag::can_encrypt(key_flag::NOT_COMPROMISED));
        assert!(!key_flag::can_encrypt_mail(key_flag::NOT_COMPROMISED));
        assert!(!key_flag::can_verify(key_flag::NOT_OBSOLETE));
        // An address that forwards elsewhere takes no encrypted mail, though
        // its key could encrypt it.
        let forwarding = sound | key_flag::EMAIL_NO_ENCRYPT;
        assert!(key_flag::can_encrypt(forwarding) && !key_flag::can_encrypt_mail(forwarding));
        assert!(key_flag::can_encrypt_mail(sound | key_flag::EMAIL_NO_SIGN));

        // Some message flags lie above the 32nd bit.
        let forwarded = message_flag::AUTO_FORWARDEE | message_flag::RECEIVED;
        assert_eq!(message_flag::AUTO_FORWARDEE, 1 << 35);
        assert!(message_flag::has(forwarded, message_flag::AUTO_FORWARDEE));
        assert!(!message_flag::has(forwarded, message_flag::SENT));
    }
}
