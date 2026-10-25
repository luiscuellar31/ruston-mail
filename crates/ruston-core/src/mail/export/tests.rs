use super::*;
use crate::model::message::{MessageMetadata, Recipient};
use mail_parser::{MessageParser, MimeHeaders};

fn message() -> FullMessage {
    FullMessage {
        meta: MessageMetadata {
            subject: "Café résumé".into(),
            sender: Recipient {
                name: "Alícia".into(),
                address: "alice@example.test".into(),
                ..Default::default()
            },
            to_list: vec![Recipient::new("bob@example.test")],
            time: 1_700_000_000,
            ..Default::default()
        },
        body: "first\nsecond".into(),
        mime_type: "text/plain".into(),
        verdict: crate::crypto::Verdict::Verified,
        attachments: vec![],
        reply_tos: vec![],
    }
}

#[test]
fn distinct_message_ids_have_distinct_filenames() {
    assert_ne!(export_filename("a+b"), export_filename("a/b"));
    assert!(!export_filename("a/b").contains('/'));
}

#[test]
fn subject_cannot_inject_a_second_header() {
    let mut full = message();
    full.meta.subject = "Hello\r\nBcc: attacker@example.test".into();
    full.meta.sender.name = "Alice\r\nX-Injected: yes".into();
    let eml = EmlWriter::new(Vec::new(), &full).unwrap().finish().unwrap();
    assert!(!eml.windows(6).any(|window| window == b"\r\nBcc:"));
    assert!(!eml.windows(13).any(|window| window == b"\r\nX-Injected:"));
    let parsed = MessageParser::default().parse(&eml).unwrap();
    assert_eq!(parsed.subject(), Some("Hello  Bcc: attacker@example.test"));
    assert_eq!(
        parsed.from().unwrap().first().unwrap().name.as_deref(),
        Some("Alice  X-Injected: yes")
    );
}

#[test]
fn eml_round_trips_unicode_headers_body_and_attachment() {
    let mut full = message();
    let attachment = Attachment {
        name: "résumé très long 2026.pdf".into(),
        mime_type: Some("application/pdf".into()),
        ..Default::default()
    };
    full.attachments.push(attachment.clone());
    let bytes: Vec<u8> = (0..200).map(|value| value as u8).collect();
    let mut writer = EmlWriter::new(Vec::new(), &full).unwrap();
    writer.attachment(&attachment, &bytes).unwrap();
    let eml = writer.finish().unwrap();

    let parsed = MessageParser::default().parse(&eml).unwrap();
    assert_eq!(parsed.subject(), Some("Café résumé"));
    assert_eq!(
        parsed.from().unwrap().first().unwrap().name.as_deref(),
        Some("Alícia")
    );
    assert_eq!(parsed.body_text(0).as_deref(), Some("first\r\nsecond"));
    assert_eq!(parsed.attachment_count(), 1);
    let attached = parsed.attachment(0).unwrap();
    assert_eq!(
        attached.attachment_name(),
        Some("résumé très long 2026.pdf")
    );
    assert_eq!(attached.contents(), bytes);
}

#[test]
fn base64_lines_hold_76_characters_and_text_ends_lines_with_crlf() {
    let mut bytes = Vec::new();
    write_base64_bytes(&mut bytes, &[0u8; 200]).unwrap();
    let lines: Vec<usize> = String::from_utf8(bytes)
        .unwrap()
        .split_terminator("\r\n")
        .map(str::len)
        .collect();
    // 200 bytes: three lines of 57, then 29.
    assert_eq!(lines, [76, 76, 76, 40]);

    let mut text = Vec::new();
    write_base64_text(&mut text, "a\rb\nc\r\nd").unwrap();
    let encoded = String::from_utf8(text).unwrap();
    let decoded = STANDARD.decode(encoded.trim_end()).unwrap();
    assert_eq!(decoded, b"a\r\nb\r\nc\r\nd");
}

#[test]
fn eml_without_attachments_has_a_single_text_part() {
    let eml = EmlWriter::new(Vec::new(), &message())
        .unwrap()
        .finish()
        .unwrap();
    let parsed = MessageParser::default().parse(&eml).unwrap();
    assert_eq!(parsed.body_text(0).as_deref(), Some("first\r\nsecond"));
    assert_eq!(parsed.attachment_count(), 0);
}

#[test]
fn existing_export_is_not_overwritten() {
    let dir = std::env::temp_dir().join(format!(
        "ruston-export-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let path = dir.join(export_filename("a+b"));
    std::fs::write(&path, b"existing").unwrap();
    assert!(matches!(
        create_export_file(&path),
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"existing");
    std::fs::remove_dir_all(dir).unwrap();
}

/// The access list of `path` in SDDL, as icacls would read it.
#[cfg(windows)]
fn access_list(path: &Path) -> String {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
        SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

    let name = verbatim(path).unwrap();
    let mut descriptor = std::ptr::null_mut();
    let mut text = std::ptr::null_mut();
    let mut length = 0;
    // SAFETY: `name` is NUL-terminated; both outputs are freed below.
    unsafe {
        assert_eq!(
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor,
            ),
            0
        );
        assert_ne!(
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                &mut length,
            ),
            0
        );
        let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, length as usize));
        LocalFree(text.cast());
        LocalFree(descriptor);
        sddl.trim_end_matches('\0').to_owned()
    }
}

#[cfg(windows)]
#[test]
fn a_private_file_on_windows_takes_nothing_from_its_folder() {
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

    // A folder that lets everyone read what it holds, as a shared one can.
    let dir = std::env::temp_dir().join(format!(
        "ruston-private-file-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let shared: Vec<u16> = "D:(A;OICI;FA;;;WD)".encode_utf16().chain([0]).collect();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: both strings are NUL-terminated; the descriptor lives until
    // the folder is created and is left to the test process to reclaim.
    unsafe {
        assert_ne!(
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                shared.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            ),
            0
        );
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        assert_ne!(
            CreateDirectoryW(verbatim(&dir).unwrap().as_ptr(), &attributes),
            0
        );
    }

    let ordinary = dir.join("ordinary.eml");
    std::fs::write(&ordinary, b"").unwrap();
    assert!(access_list(&ordinary).contains(";WD)"));

    let private = dir.join("private.eml");
    drop(create_private(&private).unwrap());
    assert_eq!(access_list(&private), PRIVATE_FILE_ACCESS);
    assert_eq!(
        create_private(&private).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );

    // Past MAX_PATH, as std allows.
    let deep = dir.join("a".repeat(120)).join("b".repeat(120));
    std::fs::create_dir_all(&deep).unwrap();
    let long = deep.join("private.eml");
    assert!(long.as_os_str().len() > 260);
    drop(create_private(&long).unwrap());
    assert_eq!(access_list(&long), PRIVATE_FILE_ACCESS);

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn placing_an_export_never_replaces_what_is_there() {
    let dir = std::env::temp_dir().join(format!(
        "ruston-export-place-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&dir).unwrap();
    let temp = dir.join(".message.eml.tmp");
    let path = dir.join("message.eml");

    std::fs::write(&temp, b"first").unwrap();
    let first = place_export(&temp, &path).unwrap();
    std::fs::remove_file(&temp).unwrap();
    std::fs::write(&temp, b"second").unwrap();
    let second = place_export(&temp, &path).unwrap();
    // A dangling symbolic link is a name already taken, not a target.
    #[cfg(unix)]
    let through_link = {
        let link = dir.join("link.eml");
        std::os::unix::fs::symlink(dir.join("missing.eml"), &link).unwrap();
        place_export(&temp, &link).unwrap()
    };
    let kept = std::fs::read(&path).unwrap();
    let missing_created = dir.join("missing.eml").exists();
    std::fs::remove_dir_all(dir).unwrap();

    assert!(first);
    assert!(!second);
    assert_eq!(kept, b"first");
    #[cfg(unix)]
    assert!(!through_link);
    assert!(!missing_created);
}

#[test]
fn invalid_address_and_mime_type_cannot_inject_headers() {
    let mut full = message();
    full.meta.sender.address = "alice@example.test\r\nBcc: attacker@example.test".into();
    assert!(EmlWriter::new(Vec::new(), &full).is_err());

    let mut full = message();
    let attachment = Attachment {
        name: "report.pdf".into(),
        mime_type: Some("application/pdf\r\nX-Injected: yes".into()),
        ..Default::default()
    };
    full.attachments.push(attachment.clone());
    let mut writer = EmlWriter::new(Vec::new(), &full).unwrap();
    writer.attachment(&attachment, b"file").unwrap();
    let eml = writer.finish().unwrap();
    assert!(!eml.windows(13).any(|window| window == b"\r\nX-Injected:"));
    assert!(
        eml.windows(b"Content-Type: application/octet-stream".len())
            .any(|window| window == b"Content-Type: application/octet-stream")
    );
}
