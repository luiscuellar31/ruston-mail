//! Messages and their attachments as Proton's mail API describes them, in
//! the PascalCase JSON it writes.

use serde::{Deserialize, Serialize};

/// Someone a message is from or to.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Recipient {
    /// The name shown for them, which may be empty.
    #[serde(default)]
    pub name: String,
    /// Their email address.
    pub address: String,
    /// The ID of their saved contact, when they are one.
    #[serde(rename = "ContactID", skip_serializing_if = "Option::is_none")]
    pub contact_id: Option<String>,
    /// `1` when the address is a Proton one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_proton: Option<i64>,
}

impl Recipient {
    /// A recipient known only by `address`, which also serves as their name.
    pub fn new(address: impl Into<String>) -> Self {
        let address = address.into();
        Recipient {
            name: address.clone(),
            address,
            contact_id: None,
            is_proton: None,
        }
    }
}

/// A file attached to a message, with what decrypting it takes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Attachment {
    /// The attachment's ID.
    #[serde(rename = "ID", default)]
    pub id: String,
    /// Its file name.
    #[serde(default)]
    pub name: String,
    /// Its size in bytes.
    #[serde(default)]
    pub size: u64,
    /// Its session key, encrypted and in base64, which opens the file.
    #[serde(rename = "KeyPackets", default)]
    pub key_packets: Option<String>,
    /// Its MIME type.
    #[serde(rename = "MIMEType", default)]
    pub mime_type: Option<String>,
    /// How it is meant to be shown: `inline`, within the body, or as an
    /// `attachment`.
    #[serde(default)]
    pub disposition: Option<String>,
    /// An armored PGP signature of its decrypted bytes, when it is signed.
    /// Proton may send an empty one for a file that is not.
    #[serde(default)]
    pub signature: Option<String>,
}

impl Attachment {
    /// Whether the file belongs within the body, as an embedded image does,
    /// rather than offered for download.
    pub fn is_inline(&self) -> bool {
        self.disposition.as_deref() == Some("inline")
    }
}

/// What a listing or a search tells about a message, without its body.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MessageMetadata {
    /// The message's ID.
    #[serde(rename = "ID")]
    pub id: String,
    /// Where it sorts in a listing.
    #[serde(default)]
    pub order: i64,
    /// The ID of its conversation.
    #[serde(rename = "ConversationID", default)]
    pub conversation_id: String,
    /// Its subject.
    #[serde(default)]
    pub subject: String,
    /// `1` while it is unread, `0` once read.
    #[serde(default)]
    pub unread: i64,
    /// Who sent it.
    #[serde(default)]
    pub sender: Recipient,
    /// Its To recipients.
    #[serde(default)]
    pub to_list: Vec<Recipient>,
    /// Its Cc recipients.
    #[serde(rename = "CCList", default)]
    pub cc_list: Vec<Recipient>,
    /// Its Bcc recipients.
    #[serde(rename = "BCCList", default)]
    pub bcc_list: Vec<Recipient>,
    /// When it was sent or received, in Unix seconds.
    #[serde(default)]
    pub time: i64,
    /// Its whole size in bytes.
    #[serde(default)]
    pub size: u64,
    /// The ID of the account's address it belongs to.
    #[serde(rename = "AddressID", default)]
    pub address_id: String,
    /// The IDs of its folders and labels.
    #[serde(rename = "LabelIDs", default)]
    pub label_ids: Vec<String>,
    /// Its RFC 822 `Message-ID`, when it has one.
    #[serde(rename = "ExternalID", default)]
    pub external_id: Option<String>,
    /// How many attachments it has.
    #[serde(default)]
    pub num_attachments: i64,
    /// Its [`message_flag`](crate::model::enums::message_flag) bits. Some
    /// lie above the 32nd, which is why this is a `u64`.
    #[serde(default)]
    pub flags: u64,
    /// When it expires, in Unix seconds, if it does.
    #[serde(default)]
    pub expiration_time: Option<i64>,
}

/// A whole message: its metadata, its encrypted body, and what opening it
/// takes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Message {
    /// What a listing tells about it, read from the same JSON object.
    #[serde(flatten)]
    pub meta: MessageMetadata,
    /// Its body, encrypted and armored.
    #[serde(default)]
    pub body: String,
    /// The MIME type of its body.
    #[serde(rename = "MIMEType", default)]
    pub mime_type: String,
    /// Its raw RFC 822 header, when Proton sends it.
    #[serde(default)]
    pub header: Option<String>,
    /// Its attachments.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// The address replies should go to, when it names one.
    #[serde(default)]
    pub reply_to: Option<Recipient>,
    /// Every address replies should go to.
    #[serde(default)]
    pub reply_tos: Vec<Recipient>,
    /// The password a password-protected message was sent with.
    #[serde(default)]
    pub password: Option<String>,
    /// The hint given for that password.
    #[serde(default)]
    pub password_hint: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::enums::message_flag;

    #[test]
    fn a_message_reads_proton_spellings_and_flags_past_32_bits() {
        let flags = message_flag::AUTO_FORWARDEE | message_flag::INTERNAL;
        let message: Message = serde_json::from_value(serde_json::json!({
            "ID": "msg-7",
            "ConversationID": "conv-7",
            "Subject": "Quarterly numbers",
            "Unread": 1,
            "Sender": {"Name": "Ana", "Address": "ana@proton.me", "ContactID": "contact-ana"},
            "ToList": [{"Name": "Bo", "Address": "bo@example.org"}],
            "Time": 1_700_000_000,
            "Flags": flags,
            "MIMEType": "text/html",
            "Body": "-----BEGIN PGP MESSAGE-----",
            "Attachments": [
                {"ID": "att-1", "Name": "q3.pdf", "KeyPackets": "a2V5", "MIMEType": "application/pdf"}
            ]
        }))
        .unwrap();

        // The metadata is read from the same object as the body.
        assert_eq!(message.meta.id, "msg-7");
        assert_eq!(message.meta.conversation_id, "conv-7");
        assert_eq!(message.meta.flags, (1 << 35) | (1 << 2));
        assert_eq!(
            message.meta.sender.contact_id.as_deref(),
            Some("contact-ana")
        );
        assert_eq!(message.meta.to_list[0].address, "bo@example.org");
        assert_eq!(message.mime_type, "text/html");
        let [attachment] = message.attachments.as_slice() else {
            panic!("one attachment, got {:?}", message.attachments);
        };
        assert_eq!(attachment.id, "att-1");
        assert_eq!(attachment.key_packets.as_deref(), Some("a2V5"));
        assert_eq!(attachment.mime_type.as_deref(), Some("application/pdf"));
    }

    #[test]
    fn metadata_is_written_back_as_proton_spells_it() {
        // The cache keeps metadata as this JSON, and reads it back.
        let meta = MessageMetadata {
            id: "msg-7".into(),
            sender: Recipient::new("ana@proton.me"),
            cc_list: vec![Recipient::new("cy@example.org")],
            label_ids: vec!["0".into()],
            external_id: Some("<q3@example.org>".into()),
            ..MessageMetadata::default()
        };
        let json = serde_json::to_value(&meta).unwrap();
        for key in [
            "ID",
            "ConversationID",
            "CCList",
            "BCCList",
            "AddressID",
            "LabelIDs",
            "ExternalID",
            "NumAttachments",
        ] {
            assert!(json.get(key).is_some(), "{key} is missing from {json}");
        }
        // A recipient with no contact and no Proton mark leaves both out.
        assert_eq!(
            json["Sender"],
            serde_json::json!({"Name": "ana@proton.me", "Address": "ana@proton.me"})
        );
        let back: MessageMetadata = serde_json::from_value(json).unwrap();
        assert_eq!(back.external_id.as_deref(), Some("<q3@example.org>"));
        assert_eq!(back.cc_list[0].address, "cy@example.org");
    }
}
