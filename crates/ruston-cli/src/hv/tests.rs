use super::*;

#[test]
fn browsers_never_inherit_sign_in_secrets() {
    let url = "https://verify.proton.me/?methods=captcha";
    for command in [
        browser_command(url).unwrap(),
        chrome_command("chrome", std::path::Path::new("profile"), url),
    ] {
        let removed: Vec<_> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_os_string())
            .collect();
        for secret in crate::cli::SECRET_ENV_VARS {
            assert!(
                removed.iter().any(|name| name == secret),
                "{secret} reaches {command:?}"
            );
        }
    }
}

#[test]
#[cfg(target_os = "windows")]
fn windows_hands_the_whole_address_to_system_rundll32() {
    let url = "https://mail-api.proton.me/core/v4/captcha?Token=a&ForceWebMessaging=1#ruston=1.2";
    let command = browser_command(url).unwrap();
    let program = std::path::Path::new(command.get_program());
    assert!(program.is_absolute(), "{program:?}");
    assert!(program.ends_with("rundll32.exe"), "{program:?}");
    assert!(program.exists(), "{program:?}");
    let args: Vec<_> = command.get_args().collect();
    assert_eq!(args, ["url.dll,FileProtocolHandler", url]);
}

#[test]
#[cfg(target_os = "windows")]
fn windows_looks_for_edge_and_per_user_browsers() {
    let browsers = windows_browsers();
    assert!(
        browsers
            .iter()
            .all(|path| std::path::Path::new(path).is_absolute())
    );
    assert!(browsers.iter().any(|path| path.ends_with(r"\msedge.exe")));
    let local = std::env::var("LOCALAPPDATA").unwrap();
    assert!(browsers.iter().any(|path| path.starts_with(&local)));
}

#[test]
fn every_secret_the_cli_reads_from_the_environment_is_listed() {
    use clap::CommandFactory;

    let cli = crate::cli::Cli::command();
    for arg in cli.get_arguments() {
        if let Some(name) = arg.get_env()
            && arg.is_hide_env_values_set()
        {
            let name = name.to_str().unwrap();
            assert!(
                crate::cli::SECRET_ENV_VARS.contains(&name),
                "{name} is not in SECRET_ENV_VARS"
            );
        }
    }
    // The password is read straight from the environment by `login`.
    assert!(crate::cli::SECRET_ENV_VARS.contains(&crate::cli::PASSWORD_ENV));
}

#[test]
fn protons_captcha_page_is_on_its_api_host() {
    for (api, endpoint) in [
        (
            "https://mail.proton.me/api",
            "https://mail-api.proton.me/core/v4/captcha",
        ),
        (
            "https://account.proton.me",
            "https://account-api.proton.me/core/v4/captcha",
        ),
        // Without a scheme, https is meant.
        (
            "mail.proton.me/api",
            "https://mail-api.proton.me/core/v4/captcha",
        ),
    ] {
        assert_eq!(captcha_endpoint(api), endpoint, "{api}");
    }
    assert_eq!(
        captcha_page("https://mail.proton.me/api", "a b/c"),
        "https://mail-api.proton.me/core/v4/captcha?Token=a%20b%2Fc&ForceWebMessaging=1"
    );
}

#[test]
fn a_local_server_serves_the_captcha_under_its_api_path() {
    for (api, endpoint) in [
        (
            "http://127.0.0.1:8080/api",
            "http://127.0.0.1:8080/api/core/v4/captcha",
        ),
        (
            "http://localhost:8080/api",
            "http://localhost:8080/api/core/v4/captcha",
        ),
        ("http://[::1]:8080", "http://[::1]:8080/api/core/v4/captcha"),
        (
            "http://devbox:8080/api",
            "http://devbox:8080/api/core/v4/captcha",
        ),
    ] {
        assert_eq!(captcha_endpoint(api), endpoint, "{api}");
    }
}

const NONCE: &str = "0123456789abcdef0123456789abcdef";

#[test]
fn tokens_are_percent_encoded_but_for_unreserved_characters() {
    assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
    assert_eq!(urlencode("Az09-_.~"), "Az09-_.~");
    // Every byte of a character beyond ASCII is encoded.
    assert_eq!(urlencode("+=&é"), "%2B%3D%26%C3%A9");
}

#[test]
fn each_sign_in_has_its_own_nonce_in_the_page_address() {
    let (first, second) = (new_nonce().unwrap(), new_nonce().unwrap());
    assert_ne!(first, second);
    for nonce in [&first, &second] {
        assert_eq!(nonce.len(), 32);
        assert!(
            nonce
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    let url = captcha_page_url(
        "https://mail-api.proton.me/core/v4/captcha?Token=t&ForceWebMessaging=1",
        47821,
        &first,
    );
    // What the bookmarklet's pattern picks out of the address.
    let (page, fragment) = url.split_once('#').unwrap();
    assert!(page.ends_with("&ForceWebMessaging=1"));
    assert_eq!(fragment, format!("ruston=47821.{first}"));
}

#[test]
fn the_console_snippet_carries_the_nonce_and_the_bookmarklet_reads_it() {
    let console = capture_snippet(Some((54321, NONCE)));
    assert!(console.contains(&format!("var t='54321.{NONCE}'")));
    assert!(console.contains("/token?nonce='+p[1]+'&value='"));
    assert!(console.contains("pm_captcha"));

    let bookmarklet = capture_snippet(None);
    assert!(bookmarklet.contains("location.hash.match(/ruston=(\\d+\\.[0-9a-f]+)/)"));
    // A javascript: URL is percent-decoded before it runs.
    assert!(!bookmarklet.contains('%'));
}

/// Sends `request` to a helper expecting [`NONCE`], and returns the token
/// it took, if any, and the status line it answered with.
fn exchange(request: &str) -> (Option<String>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let request = request.to_owned();
    let client = std::thread::spawn(move || {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        response
    });
    let (mut stream, _) = listener.accept().unwrap();
    let token = handle_conn(&mut stream, NONCE);
    drop(stream); // EOF for the client's read_to_string
    let response = client.join().unwrap();
    (
        token,
        response.lines().next().unwrap_or_default().to_owned(),
    )
}

fn get(target: &str) -> String {
    format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n")
}

#[test]
fn only_this_sign_ins_page_can_hand_in_a_token() {
    let (token, status) = exchange(&get(&format!("/token?nonce={NONCE}&value=raw%3Atoken+abc")));
    assert_eq!(token.as_deref(), Some("raw:token abc"));
    assert_eq!(status, "HTTP/1.1 200 OK");

    // A request without this sign-in's code, or with another.
    let forged = format!("/token?nonce={}&value=forged", "f".repeat(32));
    for target in ["/token?value=forged", forged.as_str()] {
        assert_eq!(
            exchange(&get(target)),
            (None, "HTTP/1.1 403 Forbidden".to_owned()),
            "{target}"
        );
    }
    assert_eq!(
        exchange(&get(&format!("/token?nonce={NONCE}&value="))),
        (None, "HTTP/1.1 400 Bad Request".to_owned())
    );
    assert_eq!(
        exchange(&get(&format!("/tokens?nonce={NONCE}&value=x"))),
        (None, "HTTP/1.1 404 Not Found".to_owned())
    );
}

#[test]
fn broken_escapes_in_a_request_do_not_end_the_sign_in() {
    // A hand-written decoder used to slice these through a character.
    let (token, status) = exchange(&get(&format!("/token?nonce={NONCE}&value=%€x%aé")));
    assert_eq!(token.as_deref(), Some("%€x%aé"));
    assert_eq!(status, "HTTP/1.1 200 OK");
}

/// A listener on a free port of this machine that does not block, as the
/// wait for the token takes one, and its port.
fn waiting_listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

#[test]
fn the_wait_ends_with_the_token_the_page_hands_in() {
    let (listener, port) = waiting_listener();
    let waiting = std::thread::spawn(move || {
        wait_for_token(&listener, NONCE, Instant::now() + Duration::from_secs(5))
    });

    // Bound already, the listener queues the connection until the wait
    // takes it.
    let mut browser = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let request = get(&format!("/token?nonce={NONCE}&value=solved%2Btoken"));
    browser.write_all(request.as_bytes()).unwrap();
    let mut answer = String::new();
    let _ = browser.read_to_string(&mut answer);

    let token = waiting.join().unwrap().ok();
    assert_eq!(token.as_deref(), Some("solved+token"));
    assert!(answer.starts_with("HTTP/1.1 200 OK"), "{answer}");
}

#[test]
fn a_wait_for_the_token_ends_at_its_deadline() {
    let (listener, _) = waiting_listener();

    let waited = wait_for_token(&listener, NONCE, Instant::now());

    assert!(matches!(
        waited,
        Err(Error::Other(ref message)) if message == "human verification timed out"
    ));
}

#[test]
fn the_manual_instructions_offer_the_console_the_bookmarklet_and_the_page() {
    let mut output = Vec::new();
    print_manual_instructions(&mut output, "CONSOLE", "BOOKMARKLET", "https://page/#n");
    let text = String::from_utf8(output).unwrap();

    assert!(
        text.starts_with("\n── Human verification (CAPTCHA) ──\n"),
        "{text}"
    );
    assert!(text.contains("\n      CONSOLE\n"), "{text}");
    assert!(text.contains("\n      javascript:BOOKMARKLET\n"), "{text}");
    assert!(
        text.ends_with("the bookmarklet reads its end): https://page/#n\n"),
        "{text}"
    );
}

#[test]
fn token_request_arriving_after_accept_is_not_dropped() {
    // On macOS an accepted socket inherits the listener's non-blocking mode,
    // so a request line that arrives after `accept` must still be read.
    let (listener, port) = waiting_listener();

    let client = std::thread::spawn(move || {
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let _ = c.write_all(get(&format!("/token?nonce={NONCE}&value=synthetic")).as_bytes());
        let mut resp = String::new();
        let _ = c.read_to_string(&mut resp);
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "the client never connected");
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("accept: {e}"),
        }
    };
    assert_eq!(
        handle_conn(&mut stream, NONCE).as_deref(),
        Some("synthetic")
    );
    drop(stream); // EOF for the client's read_to_string
    client.join().unwrap();
}

#[test]
fn a_chrome_profile_is_always_a_new_private_directory() {
    let parent = std::env::temp_dir().join(format!("ruston-hv-profiles-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(parent.join("taken")).unwrap();
    std::fs::write(parent.join("taken").join("Preferences"), b"someone else's").unwrap();

    // A name already held, with a profile in it, is passed over.
    let mut names = ["taken", "fresh"].into_iter();
    let dir = create_private_dir(&parent, || Ok(names.next().unwrap().to_owned())).unwrap();
    assert_eq!(dir, parent.join("fresh"));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    // With every name held, none is borrowed.
    let held = create_private_dir(&parent, || Ok("taken".to_owned())).unwrap_err();
    assert_eq!(held.kind(), std::io::ErrorKind::AlreadyExists);
    std::fs::remove_dir_all(&parent).unwrap();

    // A real profile has a name of its own each time.
    let first = new_chrome_profile_dir().unwrap();
    let second = new_chrome_profile_dir().unwrap();
    assert_ne!(first, second);
    assert_eq!(first.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir(first).unwrap();
    std::fs::remove_dir(second).unwrap();
}

#[test]
fn confirmation_succeeds_on_enter() {
    let (tx, rx) = mpsc::channel();
    tx.send(Confirm::Enter).unwrap();
    assert!(wait_for_confirmation(&rx, Duration::from_secs(5), || false).is_ok());
}

#[test]
fn confirmation_aborts_when_stdin_closes() {
    let (tx, rx) = mpsc::channel();
    tx.send(Confirm::Closed).unwrap();
    let err = wait_for_confirmation(&rx, Duration::from_secs(5), || false).unwrap_err();
    assert!(err.to_string().contains("stdin closed"));

    let (tx, rx) = mpsc::channel::<Confirm>();
    drop(tx);
    assert!(wait_for_confirmation(&rx, Duration::from_secs(5), || false).is_err());
}

#[test]
fn confirmation_times_out_and_closed_chrome_only_warns() {
    let (_tx, rx) = mpsc::channel::<Confirm>();
    let mut polls = 0;
    let started = Instant::now();
    let err = wait_for_confirmation(&rx, Duration::from_millis(50), || {
        polls += 1;
        true
    })
    .unwrap_err();
    assert!(err.to_string().contains("timed out"));
    assert_eq!(polls, 1, "Chrome exit is checked, and warned about, once");
    assert!(started.elapsed() < Duration::from_secs(2));
}
