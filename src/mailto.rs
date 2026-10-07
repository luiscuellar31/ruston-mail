//! The deliberately small subset of RFC 6068 that the composer supports.

pub const MAX_URL_BYTES: usize = 16 * 1024;
pub const MAX_PENDING: usize = 16;

#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    to: String,
    subject: String,
    body: String,
}

impl Request {
    pub fn into_fields(self) -> (String, String, String) {
        (self.to, self.subject, self.body)
    }

    pub fn parse(input: &str) -> Option<Self> {
        // Check before any URL normalization can strip newlines or controls.
        if input.len() > MAX_URL_BYTES || input.chars().any(char::is_control) {
            return None;
        }
        let (scheme, rest) = input.trim().split_once(':')?;
        if !scheme.eq_ignore_ascii_case("mailto") || rest.starts_with("//") || rest.contains('#') {
            return None;
        }
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let mut to = decode(path)?;
        let mut query_to = None;
        let mut subject = None;
        let mut body = None;
        for field in query.split('&').filter(|field| !field.is_empty()) {
            let (key, value) = field.split_once('=')?;
            let key = decode(key)?.to_ascii_lowercase();
            let value = decode(value)?;
            match key.as_str() {
                "to" => set_once(&mut query_to, value)?,
                "subject" => set_once(&mut subject, value)?,
                "body" => set_once(&mut body, value)?,
                // Never accept attachments, arbitrary headers, or commands.
                _ => {}
            }
        }
        if let Some(extra) = query_to.filter(|value| !value.is_empty()) {
            if !to.is_empty() {
                to.push(',');
            }
            to.push_str(&extra);
        }
        let subject = subject.unwrap_or_default();
        let body = body
            .unwrap_or_default()
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        if to.chars().any(char::is_control)
            || subject.chars().any(char::is_control)
            || body
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\t'))
        {
            return None;
        }
        let recipients = crate::mail::recipients(&to);
        if !recipients.rejected.is_empty() {
            return None;
        }
        Some(Self {
            to: recipients.accepted.join(", "),
            subject,
            body,
        })
    }
}

fn set_once(slot: &mut Option<String>, value: String) -> Option<()> {
    if slot.is_some() {
        return None;
    }
    *slot = Some(value);
    Some(())
}

fn decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%'
            && !bytes
                .get(index + 1..index + 3)?
                .iter()
                .all(u8::is_ascii_hexdigit)
        {
            return None;
        }
    }
    // Unlike form decoding, a literal '+' stays a '+', including subaddresses.
    Some(urlencoding::decode(value).ok()?.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipients_utf8_and_body_are_decoded_once_without_form_plus_rules() {
        let request = Request::parse("MAILTO:alice+work@example.org?TO=bob%40example.org&Subject=Reuni%C3%B3n+%26+notas&body=Hola%0D%0A%2520%20%3Cb%3E").unwrap();
        assert_eq!(request.to, "alice+work@example.org, bob@example.org");
        assert_eq!(request.subject, "Reunión+&+notas");
        assert_eq!(request.body, "Hola\n%20 <b>");
        assert!(
            Request::parse("mailto:?subject=Hello")
                .unwrap()
                .to
                .is_empty()
        );
    }

    #[test]
    fn hostile_or_ambiguous_links_are_rejected() {
        for input in [
            "https://example.org",
            "mailto://alice@example.org",
            "mailto:a@example.org#fragment",
            "mailto:a@example.org?subject=%0ABcc:b@example.org",
            "mailto:a%0D@example.org",
            "mailto:a@example.org?body=%00",
            "mailto:a@example.org?body=%FF",
            "mailto:a@example.org?body=%0",
            "mailto:a@example.org?body=%GG",
            "mailto:a@example.org?subject=one&SUBJECT=two",
            "mailto:not-an-address",
            "mail\nto:a@example.org",
            "mailto:a@example.org?subject=a\tb",
        ] {
            assert!(Request::parse(input).is_none(), "accepted {input:?}");
        }
        assert!(Request::parse(&format!("mailto:?body={}", "x".repeat(MAX_URL_BYTES))).is_none());
    }

    #[test]
    fn unsupported_fields_cannot_inject_sender_attachments_or_hidden_recipients() {
        let request = Request::parse("mailto:a@example.org?from=other@example.org&bcc=hidden@example.org&attach=/etc/passwd&subject=Hello").unwrap();
        assert_eq!(request.to, "a@example.org");
        assert_eq!(request.subject, "Hello");
        assert!(request.body.is_empty());
    }
}
