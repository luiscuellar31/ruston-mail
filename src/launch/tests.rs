use super::*;

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        Self(std::env::temp_dir().join(format!(
            "ruston-activation-test-{}-{}",
            std::process::id(),
            u64::from_be_bytes(random)
        )))
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn primary(folder: &Path, urls: Vec<String>) -> Instance {
    match launch(folder, urls).unwrap() {
        Start::Primary(instance) => instance,
        _ => panic!("expected primary"),
    }
}

fn launch(folder: &Path, urls: Vec<String>) -> io::Result<Start> {
    Instance::start_at(folder, arguments(urls.into_iter().map(Into::into))?)
}

fn mailto_event(url: &str) -> Event {
    Event::Mailto(Request::parse(url).unwrap())
}

#[test]
fn a_second_launch_forwards_urls_and_activation_then_a_new_primary_can_take_over() {
    let directory = Directory::new();
    let urls = vec![
        "mailto:alice@example.org?body=First".into(),
        "mailto:bob@example.org?subject=Second".into(),
    ];
    let instance = primary(&directory.0, vec!["mailto:cold@example.org".into()]);
    assert_eq!(
        instance.drain(),
        vec![mailto_event("mailto:cold@example.org")]
    );
    assert!(matches!(
        launch(&directory.0, urls.clone()).unwrap(),
        Start::Forwarded
    ));
    assert_eq!(
        instance.drain(),
        urls.iter().map(|url| mailto_event(url)).collect::<Vec<_>>()
    );
    assert!(matches!(
        launch(&directory.0, Vec::new()).unwrap(),
        Start::Forwarded
    ));
    assert_eq!(instance.drain(), vec![Event::Activate]);
    drop(instance);
    assert!(!directory.0.join("endpoint").exists());
    let replacement = primary(&directory.0, Vec::new());
    assert!(replacement.drain().is_empty());
}

/// A crashed primary's record, whose port now belongs to another program.
fn stale_record(folder: &Path) -> TcpListener {
    private_directory(folder).unwrap();
    let foreign = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    foreign.set_nonblocking(true).unwrap();
    EndpointRecord {
        port: foreign.local_addr().unwrap().port(),
        token: [9; 32],
        process: 1,
    }
    .publish(&folder.join("endpoint"))
    .unwrap();
    foreign
}

#[test]
fn a_crashed_primarys_record_cannot_send_links_to_another_program() {
    let directory = Directory::new();
    let foreign = stale_record(&directory.0);
    let instance = primary(&directory.0, Vec::new());
    assert!(matches!(
        launch(&directory.0, vec!["mailto:team@example.org".into()]).unwrap(),
        Start::Forwarded
    ));
    assert_eq!(
        instance.drain(),
        vec![mailto_event("mailto:team@example.org")]
    );
    assert_eq!(
        foreign.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock,
        "a launch connected to the old port"
    );
}

#[test]
fn a_launch_while_the_primary_closes_takes_over_once_its_lock_is_free() {
    let directory = Directory::new();
    private_directory(&directory.0).unwrap();
    // A primary that is closing has removed its record but still holds the
    // lock until its process ends.
    let closing = claim(&directory.0, &directory.0.join("endpoint"))
        .unwrap()
        .unwrap();
    let ends = thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        drop(closing);
    });
    let Start::Primary(instance) =
        launch(&directory.0, vec!["mailto:late@example.org".into()]).unwrap()
    else {
        panic!("the launch should have taken over");
    };
    ends.join().unwrap();
    assert_eq!(
        instance.drain(),
        vec![mailto_event("mailto:late@example.org")]
    );
}

#[test]
fn taking_the_lock_removes_the_previous_record_before_publishing_a_new_one() {
    let directory = Directory::new();
    let _foreign = stale_record(&directory.0);
    let endpoint = directory.0.join("endpoint");
    let lock = claim(&directory.0, &endpoint).unwrap();
    assert!(lock.is_some());
    // Until this process publishes its own record, a later launch finds none
    // and retries, rather than reaching the old port.
    assert!(!endpoint.exists());
    assert!(claim(&directory.0, &endpoint).unwrap().is_none());
}

#[test]
fn the_endpoint_record_keeps_the_layout_other_versions_read() {
    let record = EndpointRecord {
        port: 0x1234,
        token: [7; 32],
        process: 0xA1B2_C3D4,
    };
    let bytes = record.encode();
    assert_eq!(bytes[..2], [0x12, 0x34]);
    assert_eq!(bytes[2..34], [7; 32]);
    assert_eq!(bytes[34..], [0xA1, 0xB2, 0xC3, 0xD4]);
    let decoded = EndpointRecord::decode(bytes);
    assert_eq!(
        (decoded.port, decoded.token, decoded.process),
        (record.port, record.token, record.process)
    );
}

#[test]
fn unauthenticated_and_oversized_requests_are_rejected_before_delivery() {
    let directory = Directory::new();
    let instance = primary(&directory.0, Vec::new());
    let mut bad = TcpStream::connect(instance.address).unwrap();
    bad.set_read_timeout(Some(TIMEOUT)).unwrap();
    bad.write_all(MAGIC).unwrap();
    bad.write_all(&[0; 32]).unwrap();
    let mut reply = [0];
    assert!(bad.read_exact(&mut reply).is_err());
    assert!(instance.drain().is_empty());
    let mut oversized = connect(&instance.endpoint).unwrap();
    oversized
        .write_all(&((MAX_URL_BYTES + 1) as u32).to_be_bytes())
        .unwrap();
    assert!(oversized.read_exact(&mut reply).is_err());
    assert!(instance.drain().is_empty());
    drop(oversized);
    // A mailto link this version cannot open is reported, never delivered.
    let mut invalid = connect(&instance.endpoint).unwrap();
    assert!(forward(&mut invalid, "mailto:a@example.org?subject=%0AInjected").is_ok());
    assert_eq!(instance.drain(), vec![Event::Rejected]);
    drop(invalid);
    // Anything else is refused.
    let mut foreign = connect(&instance.endpoint).unwrap();
    assert!(forward(&mut foreign, "https://example.org").is_err());
    assert!(instance.drain().is_empty());
}

#[test]
fn an_email_link_this_version_cannot_open_still_reaches_the_window() {
    let directory = Directory::new();
    // A cold start opens the window, which says the link was not opened.
    let instance = primary(
        &directory.0,
        vec!["mailto:Jane%20Doe%20%3Cjane@example.org%3E".into()],
    );
    assert_eq!(instance.drain(), vec![Event::Rejected]);
    // A later launch hands such links over too, even one over the size limit.
    let oversized = format!("mailto:a@example.org?body={}", "x".repeat(MAX_URL_BYTES));
    assert!(matches!(
        launch(
            &directory.0,
            vec![
                "mailto:a@example.org#fragment".into(),
                oversized,
                "mailto:ok@example.org".into(),
            ],
        )
        .unwrap(),
        Start::Forwarded
    ));
    assert_eq!(
        instance.drain(),
        vec![
            Event::Rejected,
            Event::Rejected,
            mailto_event("mailto:ok@example.org"),
        ]
    );
    // The stand-in for an oversized link is one no version opens.
    assert!(Request::parse(OVERSIZED_LINK).is_none());
}

#[test]
fn queue_overflow_is_reported_and_does_not_replace_accepted_requests() {
    let directory = Directory::new();
    let instance = primary(&directory.0, Vec::new());
    for _ in 0..MAX_PENDING {
        assert!(
            instance
                .sender
                .send(mailto_event("mailto:accepted@example.org"))
        );
    }
    assert!(launch(&directory.0, vec!["mailto:overflow@example.org".into()]).is_err());
    let events = instance.drain();
    assert_eq!(events.len(), MAX_PENDING + 1);
    assert!(
        events[..MAX_PENDING]
            .iter()
            .all(|event| *event == mailto_event("mailto:accepted@example.org"))
    );
    assert_eq!(events[MAX_PENDING], Event::Rejected);
}

#[test]
fn arguments_cannot_be_used_as_command_options_or_arbitrary_urls() {
    assert!(arguments(["--", "mailto:a@example.org"].map(Into::into)).is_ok());
    for arg in [
        "--send",
        "https://example.org",
        "--profile=other",
        "file:///etc/passwd",
    ] {
        assert!(arguments([arg.into()]).is_err());
    }
}

#[test]
fn a_slow_peer_cannot_keep_the_activation_worker_or_shutdown_busy_indefinitely() {
    let directory = Directory::new();
    let instance = primary(&directory.0, Vec::new());
    let mut stream = TcpStream::connect(instance.address).unwrap();
    stream.set_read_timeout(Some(TIMEOUT * 3)).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let slow = thread::spawn(move || {
        for byte in MAGIC.iter().copied().chain([0; 32]) {
            if writer.write_all(&[byte]).is_err() {
                break;
            }
            thread::sleep(TIMEOUT / 16);
        }
    });
    let start = Instant::now();
    assert!(stream.read_exact(&mut [0]).is_err());
    assert!(
        start.elapsed() < TIMEOUT * 2,
        "peer extended the total deadline"
    );
    slow.join().unwrap();
    assert!(instance.drain().is_empty());
}

#[test]
fn idle_connections_from_another_local_user_cannot_block_a_later_launch() {
    let directory = Directory::new();
    let instance = primary(&directory.0, Vec::new());
    // Anyone on the machine can connect to the loopback port. The worker
    // serves one connection at a time, and these never send a header.
    let idle: Vec<_> = (0..4)
        .map(|_| TcpStream::connect(instance.address).unwrap())
        .collect();
    assert!(matches!(
        launch(&directory.0, vec!["mailto:team@example.org".into()]).unwrap(),
        Start::Forwarded
    ));
    assert_eq!(
        instance.drain(),
        vec![mailto_event("mailto:team@example.org")]
    );
    drop(idle);
}

#[test]
fn the_token_comparison_checks_every_byte() {
    let token = [7_u8; 32];
    assert!(same_token(&token, &token));
    for position in [0, 15, 31] {
        let mut guess = token;
        guess[position] ^= 1;
        assert!(!same_token(&guess, &token));
    }
}

#[cfg(unix)]
#[test]
fn activation_files_are_private_and_symlink_targets_are_never_modified() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = Directory::new();
    let instance = primary(&directory.0, Vec::new());
    assert_eq!(
        fs::metadata(&directory.0).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in ["endpoint", "instance.lock"] {
        assert_eq!(
            fs::metadata(directory.0.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let target = directory.0.join("untouched");
    fs::write(&target, b"keep").unwrap();
    let link = directory.0.join("link");
    symlink(&target, &link).unwrap();
    assert!(private_file(&link, true).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"keep");
    drop(instance);
}
