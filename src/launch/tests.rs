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
    assert!(instance.drain() == vec![mailto_event("mailto:cold@example.org")]);
    assert!(matches!(
        launch(&directory.0, urls.clone()).unwrap(),
        Start::Forwarded
    ));
    assert!(instance.drain() == urls.iter().map(|url| mailto_event(url)).collect::<Vec<_>>());
    assert!(matches!(
        launch(&directory.0, Vec::new()).unwrap(),
        Start::Forwarded
    ));
    assert!(instance.drain() == vec![Event::Activate]);
    drop(instance);
    assert!(!directory.0.join("endpoint").exists());
    let replacement = primary(&directory.0, Vec::new());
    assert!(replacement.drain().is_empty());
}

#[test]
fn stale_endpoint_data_cannot_forward_to_an_unrelated_process() {
    let directory = Directory::new();
    private_directory(&directory.0).unwrap();
    private_file(&directory.0.join("endpoint"), true)
        .unwrap()
        .write_all(&[255; 34])
        .unwrap();
    let instance = primary(&directory.0, Vec::new());
    assert!(matches!(
        launch(&directory.0, vec!["mailto:team@example.org".into()]).unwrap(),
        Start::Forwarded
    ));
    assert!(instance.drain() == vec![mailto_event("mailto:team@example.org")]);
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
    let mut invalid = connect(&instance.endpoint).unwrap();
    assert!(forward(&mut invalid, "mailto:a@example.org?subject=%0AInjected").is_err());
    assert!(instance.drain().is_empty());
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
    assert!(events[MAX_PENDING] == Event::Rejected);
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
