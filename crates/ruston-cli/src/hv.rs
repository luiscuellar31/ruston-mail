//! Human-verification (CAPTCHA) resolver for the CLI.
//!
//! Proton's captcha widget can only be *iframed* by `https://mail.proton.me`
//! (its `frame-ancestors` CSP), so we can't embed it. Instead we open it as a
//! TOP-LEVEL page (CSP `frame-ancestors` doesn't apply to top-level nav) and
//! capture the token with a one-line console snippet the user pastes: when the
//! widget solves, it `postMessage`s `{type:'pm_captcha', token}` to
//! `window.parent` which — for a top-level page — is the page itself, so a
//! `message` listener on `window` receives it and navigates the tab to our
//! local listener with it. No copy-paste.
//!
//! The page opens with `#ruston=<port>.<nonce>` at the end of its address; the
//! fragment never reaches Proton. The console snippet carries the port and
//! nonce itself, and the bookmarklet reads them from that address, so one saved
//! bookmarklet serves every sign-in. The listener takes a token only with this
//! sign-in's nonce, so no other page can hand one in.
//!
//! Resolution order:
//!   1. pre-supplied `--captcha-token` (sent verbatim),
//!   2. open captcha top-level + console snippet → auto-return to local server,
//!   3. user can always copy the token and re-run with `--captcha-token`.
//!
//! The captcha HV token is sent RAW (no `<challenge>:` prefix — that form is
//! only for email/sms codes).
//!
//! `--captcha-chrome` instead follows Proton's external-browser flow (the one
//! Proton Bridge's CLI uses): open `verify.proton.me` for the challenge in an
//! isolated Chrome window; that page registers the solved captcha against the
//! challenge itself, and once the user presses ENTER the request is retried
//! with the original challenge token. Nothing is captured from the page.

use crate::terminal;
use ruston_core::{Error, HvChallenge, HvResolver, Result};
use std::future::Future;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

const HV_TIMEOUT: Duration = Duration::from_secs(300);
/// Preferred local port. The bookmarklet reads the port from the page's
/// address; one saved by an earlier version still calls this port, where it
/// is told to be replaced.
const PREFERRED_PORT: u16 = 47821;
/// Names the port and nonce in the captcha page's address fragment.
const FRAGMENT_KEY: &str = "ruston";
const VERIFIED_PAGE: &str =
    "<h2>✓ Verified</h2><p>You can close this tab and return to your terminal.</p>";
const NOT_THIS_SIGN_IN_PAGE: &str = "<h2>Not from this sign-in</h2><p>This request did not \
    come from the captcha page your terminal opened. If you clicked a bookmarklet saved by an \
    earlier version, save the one your terminal shows now and click it on that page.</p>";
const NO_TOKEN_PAGE: &str =
    "<h2>No token received</h2><p>Your terminal is still waiting: solve the captcha again.</p>";
/// Verification page used when the challenge carries no usable `WebUrl`.
const DEFAULT_VERIFY_ORIGIN: &str = "https://verify.proton.me";

/// Build the HV resolver.
/// - `api_base`: API root (used to derive the captcha widget URL).
/// - `captcha_token`: short-circuits the interactive flow when present.
/// - `use_chrome`: open the captcha in an isolated Chrome window instead of the default browser.
pub fn resolver(captcha_token: Option<String>, api_base: String, use_chrome: bool) -> HvResolver {
    Arc::new(move |chal: HvChallenge| {
        let pre = captcha_token.clone();
        let api = api_base.clone();
        Box::pin(solve(chal, pre, api, use_chrome))
            as Pin<Box<dyn Future<Output = Result<(String, String)>> + Send>>
    })
}

async fn solve(
    chal: HvChallenge,
    pre: Option<String>,
    api_base: String,
    use_chrome: bool,
) -> Result<(String, String)> {
    if !chal.methods.is_empty() && !chal.methods.iter().any(|m| m == "captcha") {
        return Err(Error::Other(format!(
            "human verification requires an unsupported method: {:?} (only 'captcha' is handled)",
            chal.methods
        )));
    }
    if let Some(t) = pre {
        return Ok((t, "captcha".to_string()));
    }

    if use_chrome {
        let url = verify_url(&chal);
        tokio::task::spawn_blocking(move || run_external_flow(&url))
            .await
            .map_err(|e| Error::Other(format!("hv task: {e}")))??;
        // The verify page attached the solved captcha to the challenge, so the
        // retry echoes the challenge token itself (as Proton Bridge does).
        return Ok((chal.token, "captcha".to_string()));
    }

    let (endpoint, _origin) = captcha_endpoint(&api_base);
    let page = format!(
        "{endpoint}?Token={}&ForceWebMessaging=1",
        urlencode(&chal.token)
    );

    tokio::task::spawn_blocking(move || run_manual_flow(&page))
        .await
        .map_err(|e| Error::Other(format!("hv task: {e}")))?
}

/// Proton's standalone verification page for `chal`, restricted to the captcha
/// method (the only one whose external-browser result the retry can redeem).
/// Keeps the host of the server-provided `WebUrl` when it is https and a plain
/// host name, which the browser launch then passes on as written.
fn verify_url(chal: &HvChallenge) -> String {
    let origin = chal
        .web_url
        .strip_prefix("https://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .filter(|host| {
            !host.is_empty()
                && host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        })
        .map_or_else(
            || DEFAULT_VERIFY_ORIGIN.to_string(),
            |host| format!("https://{host}"),
        );
    format!("{origin}/?methods=captcha&token={}", urlencode(&chal.token))
}

/// `--captcha-chrome`: open the verification page in an isolated Chrome window
/// and wait (bounded) for the user to confirm completion with ENTER.
fn run_external_flow(url: &str) -> Result<()> {
    terminal::eprintln!("\n── Human verification (CAPTCHA) ──");
    let mut chrome = match launch_chrome(url) {
        Ok(c) => {
            terminal::eprintln!("Opened the verification page in an isolated Chrome window.");
            Some(c)
        }
        Err(msg) => {
            terminal::eprintln!(
                "Chrome launch failed ({msg}); opening your default browser instead."
            );
            if open_browser(url).is_err() {
                terminal::eprintln!("Could not open a browser. Open this URL manually:\n{url}");
            }
            None
        }
    };
    terminal::eprintln!(
        "Solve the CAPTCHA there. When the page says verification is complete, \
         press ENTER here (waiting up to {} minutes).",
        HV_TIMEOUT.as_secs() / 60
    );

    let outcome = wait_for_confirmation(&spawn_enter_reader(), HV_TIMEOUT, || {
        chrome
            .as_mut()
            .is_some_and(|(c, _)| matches!(c.try_wait(), Ok(Some(_))))
    });

    // Close the isolated Chrome window we opened (if any), then remove its
    // throwaway profile. Best effort: a leftover only wastes temp space.
    if let Some((mut c, profile)) = chrome {
        let _ = c.kill();
        let _ = c.wait();
        let _ = std::fs::remove_dir_all(&profile);
    }
    if outcome.is_ok() {
        terminal::eprintln!("Verification confirmed — retrying.");
    }
    outcome
}

/// What the stdin reader observed.
enum Confirm {
    Enter,
    Closed,
}

/// Read one line from stdin on a helper thread. If we time out first, the
/// thread stays parked on stdin, which is harmless: the login then fails.
fn spawn_enter_reader() -> mpsc::Receiver<Confirm> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let msg = match std::io::stdin().read_line(&mut line) {
            Ok(n) if n > 0 => Confirm::Enter,
            _ => Confirm::Closed,
        };
        let _ = tx.send(msg);
    });
    rx
}

/// Wait until the user confirms, stdin closes, or `timeout` elapses. A closed
/// Chrome window only warns: when another Chrome owns the profile the process
/// we spawned exits immediately while its window stays open.
fn wait_for_confirmation(
    rx: &mpsc::Receiver<Confirm>,
    timeout: Duration,
    mut chrome_exited: impl FnMut() -> bool,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let mut warned = false;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Error::Other(
                "human verification timed out waiting for ENTER".into(),
            ));
        }
        match rx.recv_timeout(left.min(Duration::from_millis(500))) {
            Ok(Confirm::Enter) => return Ok(()),
            Ok(Confirm::Closed) | Err(RecvTimeoutError::Disconnected) => {
                return Err(Error::Other(
                    "human verification aborted: stdin closed before ENTER was pressed".into(),
                ));
            }
            Err(RecvTimeoutError::Timeout) => {
                if !warned && chrome_exited() {
                    warned = true;
                    terminal::eprintln!(
                        "The Chrome window was closed. Press ENTER if verification \
                         completed, or Ctrl+C to abort."
                    );
                }
            }
        }
    }
}

/// Open the captcha top-level, print the capture snippets, wait for the token.
/// `page` is the captcha page's address, without this sign-in's fragment.
fn run_manual_flow(page: &str) -> Result<(String, String)> {
    let listener = TcpListener::bind(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| TcpListener::bind("127.0.0.1:0"))
        .map_err(|e| Error::Other(format!("cannot start HV helper server: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| Error::Other(format!("hv addr: {e}")))?
        .port();
    listener
        .set_nonblocking(true)
        .map_err(|e| Error::Other(format!("hv nonblocking: {e}")))?;
    let nonce = new_nonce()?;
    let captcha_url = captcha_page_url(page, port, &nonce);

    let console = capture_snippet(Some((port, &nonce)));
    let bookmarklet = capture_snippet(None);
    let err = std::io::stderr();
    let mut e = err.lock();
    let _ = terminal::writeln!(e, "\n── Human verification (CAPTCHA) ──");
    let _ = terminal::writeln!(
        e,
        "A browser tab is opening at the captcha page. Then EITHER:"
    );
    let _ = terminal::writeln!(
        e,
        "\n  [A] Console: open DevTools → Console (F12) and paste once:"
    );
    let _ = terminal::writeln!(e, "      {console}");
    let _ = terminal::writeln!(
        e,
        "\n  [B] Bookmarklet (save it once): save this as a bookmark, then click it"
    );
    let _ = terminal::writeln!(
        e,
        "      on the captcha page opened from here (no console, no \"allow pasting\")."
    );
    let _ = terminal::writeln!(
        e,
        "      One saved by an earlier version is refused; save this one instead:"
    );
    let _ = terminal::writeln!(e, "      javascript:{bookmarklet}");
    let _ = terminal::writeln!(
        e,
        "\nSolve the CAPTCHA — the token returns here automatically."
    );
    let _ = terminal::writeln!(
        e,
        "(Fallback: copy the token, re-run login with --captcha-token <token>.)"
    );
    let _ = terminal::writeln!(
        e,
        "\nCaptcha URL (open it whole; the bookmarklet reads its end): {captcha_url}"
    );
    let _ = e.flush();

    let _ = open_browser(&captcha_url);

    let deadline = Instant::now() + HV_TIMEOUT;
    loop {
        if Instant::now() >= deadline {
            break Err(Error::Other("human verification timed out".into()));
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                if let Some(token) = handle_conn(&mut stream, &nonce) {
                    let _ = terminal::writeln!(e, "Token received — continuing login.");
                    break Ok((token, "captcha".to_string()));
                }
            }
            Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(err) => break Err(Error::Other(format!("hv accept: {err}"))),
        }
    }
}

/// 128 random bits, in hex, that only this sign-in's captcha page carries.
fn new_nonce() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| Error::Other(format!("hv nonce: {e}")))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// The captcha page's address with this sign-in's port and nonce after `#`.
/// Browsers keep the fragment to themselves, so it never reaches Proton.
fn captcha_page_url(page: &str, port: u16, nonce: &str) -> String {
    format!("{page}#{FRAGMENT_KEY}={port}.{nonce}")
}

/// The capture snippet. On the `pm_captcha` message it NAVIGATES the tab to
/// our local listener (the page's CSP `connect-src https: wss:` blocks `fetch`
/// to http://localhost, but top-level navigation is not governed by
/// connect-src). Shows a small status banner until then.
///
/// With `target`, the snippet carries the port and nonce itself, for pasting
/// into the console. Without, it reads them from the page's address, so one
/// bookmarklet serves every sign-in; on a page the terminal did not open it
/// says so and sends nothing. It holds no `%`, because a `javascript:` URL is
/// percent-decoded before it runs.
fn capture_snippet(target: Option<(u16, &str)>) -> String {
    let target = match target {
        Some((port, nonce)) => format!("'{port}.{nonce}'"),
        None => format!("(location.hash.match(/{FRAGMENT_KEY}=(\\d+\\.[0-9a-f]+)/)||[])[1]"),
    };
    format!(
        "(function(){{var t={target},b=document.createElement('div');b.style='position:fixed;left:0;right:0;bottom:0;background:#111;color:#0f0;font:13px monospace;z-index:2147483647;padding:8px';document.body.appendChild(b);if(!t){{b.style.background='#300';b.textContent='ruston-cli: open the captcha from the link in your terminal, then try again';return}}var p=t.split('.');b.textContent='ruston-cli: solve the captcha — token returns automatically';addEventListener('message',function(e){{var d=e.data;if(typeof d==='string'){{try{{d=JSON.parse(d)}}catch(x){{return}}}}if(d&&d.type==='pm_captcha'&&d.token){{b.style.background='#030';b.textContent='ruston-cli: token captured — returning…';location.href='http://127.0.0.1:'+p[0]+'/token?nonce='+p[1]+'&value='+encodeURIComponent(d.token)}}}});console.log('ruston-cli: ready (pm_captcha listener installed)')}})()"
    )
}

/// Takes the token from `GET /token?nonce=…&value=…`. A request without this
/// sign-in's nonce, or without a token, is answered as such and the wait goes
/// on. CORS hides the answer from the page, but the request still arrives.
fn handle_conn(stream: &mut std::net::TcpStream, nonce: &str) -> Option<String> {
    // On macOS/BSD the accepted socket inherits the listener's non-blocking
    // mode; without this a request that arrives after `accept` is dropped.
    stream.set_nonblocking(false).ok();
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf).ok()?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let mut request_line = req.lines().next()?.split_whitespace();
    let (method, target) = (request_line.next()?, request_line.next()?);
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if method != "GET" || path != "/token" {
        respond(stream, "404 Not Found", "<h2>Not found</h2>");
        return None;
    }

    let (mut sent_nonce, mut token) = (None, None);
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match &*key {
            "nonce" => sent_nonce = Some(value),
            "value" => token = Some(value),
            _ => {}
        }
    }
    if !sent_nonce.is_some_and(|sent| same_nonce(&sent, nonce)) {
        respond(stream, "403 Forbidden", NOT_THIS_SIGN_IN_PAGE);
        return None;
    }
    match token.filter(|token| !token.is_empty()) {
        Some(token) => {
            respond(stream, "200 OK", VERIFIED_PAGE);
            Some(token.into_owned())
        }
        None => {
            respond(stream, "400 Bad Request", NO_TOKEN_PAGE);
            None
        }
    }
}

/// Compares without stopping at the first difference, so the time taken does
/// not tell how much of a guess was right.
fn same_nonce(sent: &str, expected: &str) -> bool {
    sent.len() == expected.len()
        && sent
            .bytes()
            .zip(expected.bytes())
            .fold(0, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn respond(stream: &mut std::net::TcpStream, status: &str, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><body style=\"font:16px sans-serif;padding:40px\">{message}"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Derive the captcha widget endpoint + origin from the API base.
/// `https://mail.proton.me/api` -> (`https://mail-api.proton.me/core/v4/captcha`, origin).
fn captcha_endpoint(api_base: &str) -> (String, String) {
    let (scheme, host) = split_scheme_host(api_base);
    let is_ip_or_local = host.starts_with("127.")
        || host.starts_with("localhost")
        || host
            .split(':')
            .next()
            .unwrap_or("")
            .parse::<std::net::IpAddr>()
            .is_ok();
    if is_ip_or_local || !host.contains('.') {
        let origin = format!("{scheme}://{host}");
        (format!("{origin}/api/core/v4/captcha"), origin)
    } else {
        let (first, rest) = host.split_once('.').unwrap();
        let api_host = format!("{first}-api.{rest}");
        let origin = format!("{scheme}://{api_host}");
        (format!("{origin}/core/v4/captcha"), origin)
    }
}

fn split_scheme_host(url: &str) -> (String, String) {
    if let Some(i) = url.find("://") {
        let scheme = url[..i].to_string();
        let rest = &url[i + 3..];
        let host = rest.split('/').next().unwrap_or(rest).to_string();
        (scheme, host)
    } else {
        (
            "https".to_string(),
            url.split('/').next().unwrap_or(url).to_string(),
        )
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn open_browser(url: &str) -> std::io::Result<()> {
    browser_command(url)?.spawn().map(|_| ())
}

/// The system's way to open `url` in the default browser. No shell reads the
/// address: `cmd /C start` would cut it at the first `&`.
fn browser_command(url: &str) -> std::io::Result<std::process::Command> {
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "linux")]
    let mut cmd = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new(system_rundll32()?);
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    cmd.arg(url);
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    without_secrets(&mut cmd);
    Ok(cmd)
}

/// rundll32 from System32. Windows looks for a program named without a
/// folder beside this executable first, where a download could be waiting.
#[cfg(target_os = "windows")]
fn system_rundll32() -> std::io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    let mut buffer = [0_u16; 260];
    // SAFETY: the length passed is the buffer's own; the API writes at most
    // that many UTF-16 units and returns how many it wrote, or 0 on failure.
    let length = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(std::io::Error::last_os_error());
    }
    Ok(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length])).join("rundll32.exe"))
}

/// A browser started now may outlive the CLI, so it gets none of the
/// sign-in secrets the CLI's environment may hold.
fn without_secrets(command: &mut std::process::Command) {
    for name in crate::cli::SECRET_ENV_VARS {
        command.env_remove(name);
    }
}

/// Names a new Chrome profile tries; each has 64 random bits, so only
/// directories made on purpose under every one of them would use them all.
const PROFILE_NAME_ATTEMPTS: u32 = 8;

/// A new, empty Chrome profile for this verification only (never a user's).
/// It is created here, exclusively and on Unix for this user alone, under a
/// name with a random part: a name another user of a shared temporary
/// directory could guess, and prepare with a profile of theirs, is never used.
fn new_chrome_profile_dir() -> std::io::Result<PathBuf> {
    create_private_dir(&std::env::temp_dir(), || {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).map_err(|e| std::io::Error::other(e.to_string()))?;
        let random: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(format!("ruston-cli-hv-{}-{random}", std::process::id()))
    })
}

/// Creates a directory in `parent` under the first name from `next_name` that
/// nothing holds yet, so it is always a new one.
fn create_private_dir(
    parent: &std::path::Path,
    mut next_name: impl FnMut() -> std::io::Result<String>,
) -> std::io::Result<PathBuf> {
    for _ in 0..PROFILE_NAME_ATTEMPTS {
        let dir = parent.join(next_name()?);
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "no free name for a Chrome profile",
    ))
}

/// Open `url` in an isolated Chrome window with a new throwaway profile.
/// Returns the child process and its profile dir so both can be cleaned up
/// once verification completes.
fn launch_chrome(url: &str) -> std::result::Result<(std::process::Child, PathBuf), String> {
    let chrome = find_chrome().ok_or_else(|| "no Chrome/Chromium binary found".to_string())?;
    let profile = new_chrome_profile_dir().map_err(|e| format!("Chrome profile: {e}"))?;
    match chrome_command(&chrome, &profile, url).spawn() {
        Ok(child) => Ok((child, profile)),
        Err(e) => {
            let _ = std::fs::remove_dir(&profile);
            Err(format!("spawn {chrome}: {e}"))
        }
    }
}

fn chrome_command(chrome: &str, profile: &std::path::Path, url: &str) -> std::process::Command {
    let mut command = std::process::Command::new(chrome);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(format!("--app={url}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    without_secrets(&mut command);
    command
}

fn find_chrome() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        windows_browsers()
            .into_iter()
            .find(|path| std::path::Path::new(path).exists())
    }
    #[cfg(unix)]
    {
        #[cfg(target_os = "macos")]
        let candidates = [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ];
        #[cfg(target_os = "linux")]
        let candidates = [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "brave-browser",
        ];
        for c in candidates {
            if c.contains('/') {
                if std::path::Path::new(c).exists() {
                    return Some(c.to_string());
                }
            } else if let Some(p) = which(c) {
                return Some(p);
            }
        }
        None
    }
}

/// Chromium-based browsers in the per-machine and per-user install folders.
/// Microsoft Edge comes with Windows, so one is nearly always found.
#[cfg(target_os = "windows")]
fn windows_browsers() -> Vec<String> {
    const BROWSERS: [&str; 3] = [
        r"Google\Chrome\Application\chrome.exe",
        r"BraveSoftware\Brave-Browser\Application\brave.exe",
        r"Microsoft\Edge\Application\msedge.exe",
    ];
    let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    BROWSERS
        .iter()
        .flat_map(|browser| roots.iter().map(move |root| root.join(browser)))
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

#[cfg(unix)]
fn which(bin: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(bin);
        if p.exists() {
            return Some(p.to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
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
    fn the_verification_page_keeps_only_a_plain_https_host() {
        let page = |web_url: &str| {
            verify_url(&HvChallenge {
                token: "t".into(),
                methods: vec!["captcha".into()],
                web_url: web_url.into(),
            })
        };
        assert_eq!(
            page("https://verify-api.proton.me/?methods=captcha"),
            "https://verify-api.proton.me/?methods=captcha&token=t"
        );
        for hostile in [
            "https://x&calc&/",
            "https://evil host/",
            "https://a\"b/",
            "http://verify.proton.me/",
            "https://",
        ] {
            assert_eq!(
                page(hostile),
                format!("{DEFAULT_VERIFY_ORIGIN}/?methods=captcha&token=t"),
                "{hostile}"
            );
        }
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_hands_the_whole_address_to_system_rundll32() {
        let url =
            "https://mail-api.proton.me/core/v4/captcha?Token=a&ForceWebMessaging=1#ruston=1.2";
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
        assert!(crate::cli::SECRET_ENV_VARS.contains(&"PROTON_PASSWORD"));
    }

    #[test]
    fn captcha_endpoint_uses_api_subdomain() {
        let (url, origin) = captcha_endpoint("https://mail.proton.me/api");
        assert_eq!(url, "https://mail-api.proton.me/core/v4/captcha");
        assert_eq!(origin, "https://mail-api.proton.me");
    }

    #[test]
    fn captcha_endpoint_localhost_path_form() {
        let (url, _o) = captcha_endpoint("http://127.0.0.1:8080/api");
        assert_eq!(url, "http://127.0.0.1:8080/api/core/v4/captcha");
    }

    const NONCE: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn url_codec() {
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
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
        assert!(!bookmarklet.contains(&PREFERRED_PORT.to_string()));
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
        let (token, status) =
            exchange(&get(&format!("/token?nonce={NONCE}&value=raw%3Atoken+abc")));
        assert_eq!(token.as_deref(), Some("raw:token abc"));
        assert_eq!(status, "HTTP/1.1 200 OK");

        // Any other page, and a bookmarklet saved by an earlier version.
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

    #[test]
    fn local_server_captures_token() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();

        let handle = std::thread::spawn(move || -> Option<String> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if Instant::now() >= deadline {
                    return None;
                }
                match listener.accept() {
                    Ok((mut s, _)) => {
                        if let Some(t) = handle_conn(&mut s, NONCE) {
                            return Some(t);
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => return None,
                }
            }
        });

        std::thread::sleep(Duration::from_millis(100));
        let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.write_all(get(&format!("/token?nonce={NONCE}&value=raw%3Atoken+abc")).as_bytes())
            .unwrap();
        let mut resp = String::new();
        let _ = c.read_to_string(&mut resp);

        let got = handle.join().unwrap();
        assert_eq!(got.as_deref(), Some("raw:token abc"));
    }

    #[test]
    fn token_request_arriving_after_accept_is_not_dropped() {
        // On macOS an accepted socket inherits the listener's non-blocking mode,
        // so a request line that arrives after `accept` must still be read.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();

        let client = std::thread::spawn(move || {
            let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            let _ = c.write_all(get(&format!("/token?nonce={NONCE}&value=synthetic")).as_bytes());
            let mut resp = String::new();
            let _ = c.read_to_string(&mut resp);
        });

        let mut stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
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
        let parent =
            std::env::temp_dir().join(format!("ruston-hv-profiles-{}", std::process::id()));
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

    fn chal(token: &str, web_url: &str) -> HvChallenge {
        HvChallenge {
            token: token.into(),
            methods: vec!["captcha".into(), "email".into()],
            web_url: web_url.into(),
        }
    }

    #[test]
    fn verify_url_keeps_web_url_host_and_forces_captcha() {
        let c = chal(
            "chal/1 x",
            "https://verify.proton.me/?methods=captcha,email&token=other",
        );
        assert_eq!(
            verify_url(&c),
            "https://verify.proton.me/?methods=captcha&token=chal%2F1%20x"
        );
    }

    #[test]
    fn verify_url_falls_back_for_missing_or_insecure_web_url() {
        let expected = "https://verify.proton.me/?methods=captcha&token=t";
        assert_eq!(verify_url(&chal("t", "")), expected);
        assert_eq!(
            verify_url(&chal("t", "http://attacker.example/?token=t")),
            expected
        );
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
}
