//! The CAPTCHA Proton may ask a sign-in to pass, its human verification: how
//! `login` gets a person to solve it.
//!
//! By default it follows Proton's external-browser flow, the one Proton
//! Bridge's CLI and the desktop use: open `verify.proton.me` for the challenge
//! in an isolated Chrome window, or the default browser without one; that
//! page registers the solved captcha against the challenge itself, and once
//! the user presses ENTER the request is retried with the original challenge
//! token. Nothing is captured from the page.
//!
//! `--captcha-manual` opens the API's captcha page instead, which refuses to
//! be framed by any site but `https://mail.proton.me` (its `frame-ancestors`
//! policy), a rule that does not apply to it as a tab of its own, so it opens
//! as one. Solved, the captcha posts a `pm_captcha` message with its token to
//! `window.parent`, which for a page in its own tab is the page itself. A
//! snippet run on that page, from the console or as a bookmarklet, listens
//! for the message and takes the tab to a listener on this machine, token and
//! all, so nothing has to be copied by hand.
//!
//! The page opens with `#ruston=<port>.<nonce>` at the end of its address; the
//! fragment never reaches Proton. The console snippet carries the port and
//! nonce itself, and the bookmarklet reads them from that address, so one saved
//! bookmarklet serves every sign-in. The listener takes a token only with this
//! sign-in's nonce, so no other page can hand one in.
//!
//! A token given with `--captcha-token` answers the challenge without any of
//! this, and one copied from the page can be given that way to a new
//! `login`. A captcha's token goes to Proton as the captcha made it: only
//! codes sent by email or SMS take a `<challenge>:` prefix.

use crate::terminal;
use futures::FutureExt;
use ruston_core::model::enums::human_verification;
use ruston_core::{Error, HvChallenge, HvResolver, Result};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

const HV_TIMEOUT: Duration = Duration::from_secs(300);
/// Names the port and nonce in the captcha page's address fragment.
const FRAGMENT_KEY: &str = "ruston";
const VERIFIED_PAGE: &str =
    "<h2>✓ Verified</h2><p>You can close this tab and return to your terminal.</p>";
const NOT_THIS_SIGN_IN_PAGE: &str = "<h2>Not from this sign-in</h2><p>This request did not \
    come from the captcha page your terminal opened.</p>";
const NO_TOKEN_PAGE: &str =
    "<h2>No token received</h2><p>Your terminal is still waiting: solve the captcha again.</p>";

/// The human-verification step of a sign-in. A `captcha_token` solved
/// beforehand answers a challenge as it is. Otherwise the person solves the
/// captcha in a browser: with `use_chrome`, on Proton's verification page in
/// an isolated Chrome window; without, on the captcha page of the API at
/// `api_base`, which hands the token back to a listener here.
pub fn resolver(captcha_token: Option<String>, api_base: String, use_chrome: bool) -> HvResolver {
    let verification = Arc::new(Verification {
        token: captcha_token,
        api_base,
        chrome: use_chrome,
    });
    Arc::new(move |challenge: HvChallenge| {
        let verification = Arc::clone(&verification);
        async move { verification.solve(challenge).await }.boxed()
    })
}

/// How this sign-in answers a human-verification challenge.
struct Verification {
    /// A token solved beforehand, sent as it is.
    token: Option<String>,
    /// The API the sign-in talks to; its host serves the captcha page.
    api_base: String,
    /// Whether Proton's verification page opens in an isolated Chrome window.
    chrome: bool,
}

impl Verification {
    /// The token that answers `challenge`, with its type: a captcha's, the
    /// only method answered here.
    async fn solve(&self, challenge: HvChallenge) -> Result<(String, String)> {
        if !challenge.offers_captcha() {
            return Err(Error::Other(format!(
                "human verification requires an unsupported method: {:?} (only 'captcha' is handled)",
                challenge.methods
            )));
        }
        let token = if let Some(token) = &self.token {
            token.clone()
        } else if self.chrome {
            let page = challenge.verification_url();
            off_the_runtime(move || run_external_flow(&page)).await?;
            // That page attached the solved captcha to the challenge itself,
            // so the retry sends the challenge's own token, as Proton Bridge
            // does.
            challenge.token
        } else {
            let page = captcha_page(&self.api_base, &challenge.token);
            off_the_runtime(move || run_manual_flow(&page)).await?
        };
        Ok((token, human_verification::CAPTCHA.to_owned()))
    }
}

/// Runs `wait`, which blocks until the person at the terminal is done, on a
/// thread of its own, so the runtime is not held up meanwhile.
async fn off_the_runtime<T: Send + 'static>(
    wait: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(wait)
        .await
        .map_err(failed("hv task"))?
}

/// Turns a failure of the local verification helper into an error that
/// says what it was doing.
fn failed<E: std::fmt::Display>(doing: &'static str) -> impl FnOnce(E) -> Error {
    move |error| Error::Other(format!("{doing}: {error}"))
}

/// The default: open the verification page in an isolated Chrome window and
/// wait (bounded) for the user to confirm completion with ENTER.
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

    // A window opened here closes now, and its throwaway profile goes with
    // it. Best effort: a leftover only wastes temp space.
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

/// `--captcha-manual`: opens the captcha page in the default browser, says
/// how to hand the token back, and waits for it. `page` is the captcha page's
/// address, without this sign-in's fragment.
fn run_manual_flow(page: &str) -> Result<String> {
    let (listener, port) = token_listener()?;
    let nonce = new_nonce()?;
    let captcha_url = captcha_page_url(page, port, &nonce);

    print_manual_instructions(
        &mut std::io::stderr().lock(),
        &capture_snippet(Some((port, &nonce))),
        &capture_snippet(None),
        &captcha_url,
    );
    let _ = open_browser(&captcha_url);

    let token = wait_for_token(&listener, &nonce, Instant::now() + HV_TIMEOUT)?;
    let _ = terminal::writeln!(std::io::stderr(), "Token received — continuing login.");
    Ok(token)
}

/// A listener on this machine for the captcha page to hand the token to, on
/// a port the system picks, and that port. It does not block, so the wait
/// for the token keeps to its deadline.
fn token_listener() -> Result<(TcpListener, u16)> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(failed("cannot start HV helper server"))?;
    let port = listener.local_addr().map_err(failed("hv addr"))?.port();
    listener
        .set_nonblocking(true)
        .map_err(failed("hv nonblocking"))?;
    Ok((listener, port))
}

/// Tells the person how to hand this sign-in's token back: through the
/// browser console with `console`, or the saved `bookmarklet`, on the page
/// at `captcha_url`.
fn print_manual_instructions(
    output: &mut impl Write,
    console: &str,
    bookmarklet: &str,
    captcha_url: &str,
) {
    let _ = terminal::writeln!(output, "\n── Human verification (CAPTCHA) ──");
    let _ = terminal::writeln!(
        output,
        "A browser tab is opening at the captcha page. Then EITHER:"
    );
    let _ = terminal::writeln!(
        output,
        "\n  [A] Console: open DevTools → Console (F12) and paste once:"
    );
    let _ = terminal::writeln!(output, "      {console}");
    let _ = terminal::writeln!(
        output,
        "\n  [B] Bookmarklet (save it once): save this as a bookmark, then click it"
    );
    let _ = terminal::writeln!(
        output,
        "      on the captcha page opened from here (no console, no \"allow pasting\"):"
    );
    let _ = terminal::writeln!(output, "      javascript:{bookmarklet}");
    let _ = terminal::writeln!(
        output,
        "\nSolve the CAPTCHA — the token returns here automatically."
    );
    let _ = terminal::writeln!(
        output,
        "(Fallback: copy the token, re-run login with --captcha-token <token>.)"
    );
    let _ = terminal::writeln!(
        output,
        "\nCaptcha URL (open it whole; the bookmarklet reads its end): {captcha_url}"
    );
    let _ = output.flush();
}

/// Waits until this sign-in's page hands in its token on `listener`, which
/// does not block, or `deadline` passes. Requests that carry another nonce,
/// or no token, are answered and the wait goes on.
fn wait_for_token(listener: &TcpListener, nonce: &str, deadline: Instant) -> Result<String> {
    loop {
        if Instant::now() >= deadline {
            break Err(Error::Other("human verification timed out".into()));
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                if let Some(token) = handle_conn(&mut stream, nonce) {
                    break Ok(token);
                }
            }
            Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(error) => break Err(Error::Other(format!("hv accept: {error}"))),
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

/// The snippet that hands the token back. On the `pm_captcha` message it
/// sends the tab itself to the listener here: the page's `connect-src https:
/// wss:` policy forbids a `fetch` to http://localhost, but not a navigation.
/// Until then a banner at the foot of the page says what it is doing.
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
        concat!(
            "(function(){{",
            "var t={target},b=document.createElement('div');",
            "b.style='position:fixed;left:0;right:0;bottom:0;background:#111;color:#0f0;font:13px monospace;z-index:2147483647;padding:8px';",
            "document.body.appendChild(b);",
            "if(!t){{",
            "b.style.background='#300';",
            "b.textContent='ruston-cli: open the captcha from the link in your terminal, then try again';",
            "return}}",
            "var p=t.split('.');",
            "b.textContent='ruston-cli: solve the captcha — token returns automatically';",
            "addEventListener('message',function(e){{",
            "var d=e.data;",
            "if(typeof d==='string'){{try{{d=JSON.parse(d)}}catch(x){{return}}}}",
            "if(d&&d.type==='pm_captcha'&&d.token){{",
            "b.style.background='#030';",
            "b.textContent='ruston-cli: token captured — returning…';",
            "location.href='http://127.0.0.1:'+p[0]+'/token?nonce='+p[1]+'&value='+encodeURIComponent(d.token)",
            "}}}});",
            "console.log('ruston-cli: ready (pm_captcha listener installed)')",
            "}})()",
        ),
        target = target,
    )
}

/// Takes the token from `GET /token?nonce=…&value=…`. A request without this
/// sign-in's nonce, or without a token, is answered as such and the wait goes
/// on. CORS hides the answer from the page, but the request still arrives.
fn handle_conn(stream: &mut std::net::TcpStream, nonce: &str) -> Option<String> {
    let line = request_line(stream)?;
    let mut words = line.split_whitespace();
    let (method, target) = (words.next()?, words.next()?);
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

/// How long a browser that connected has to send its request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// The first line of the request on `stream`, from a single read: a browser
/// sends a request this short in one piece.
fn request_line(stream: &mut std::net::TcpStream) -> Option<String> {
    // On macOS/BSD the accepted socket inherits the listener's non-blocking
    // mode; without this a request that arrives after `accept` is dropped.
    stream.set_nonblocking(false).ok();
    stream.set_read_timeout(Some(REQUEST_TIMEOUT)).ok();
    let mut received = [0_u8; 8192];
    let length = stream.read(&mut received).ok()?;
    String::from_utf8_lossy(&received[..length])
        .lines()
        .next()
        .map(str::to_owned)
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

/// The captcha page for the challenge `token` names, served by the API at
/// `api_base`, asked to post the solved token as a message, which the
/// capture snippet listens for.
fn captcha_page(api_base: &str, token: &str) -> String {
    format!(
        "{}?Token={}&ForceWebMessaging=1",
        captcha_endpoint(api_base),
        urlencode(token)
    )
}

/// Where the API at `api_base` serves its captcha page. Proton serves it
/// from the API's own host: `mail-api.proton.me` for
/// `https://mail.proton.me/api`. A server named by an address, by
/// `localhost` or by a name without a dot is a local one, which serves it
/// under its API path. Without a scheme, https is meant.
fn captcha_endpoint(api_base: &str) -> String {
    let (scheme, rest) = api_base.split_once("://").unwrap_or(("https", api_base));
    // The host and any port, without the path.
    let host = rest.split('/').next().unwrap_or(rest);
    match host.split_once('.') {
        Some((first, domain)) if !is_local(host) => {
            format!("{scheme}://{first}-api.{domain}/core/v4/captcha")
        }
        _ => format!("{scheme}://{host}/api/core/v4/captcha"),
    }
}

/// Whether `host`, perhaps with a port, names this machine or an address
/// rather than a domain.
fn is_local(host: &str) -> bool {
    let name = host.split(':').next().unwrap_or(host);
    host.starts_with("127.") || host.starts_with("localhost") || name.parse::<IpAddr>().is_ok()
}

/// Percent-encodes every byte but those of the characters RFC 3986 leaves
/// unreserved, so a token passes through a query unchanged.
fn urlencode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn open_browser(url: &str) -> std::io::Result<()> {
    browser_command(url)?.spawn().map(|_| ())
}

/// The system's way to open `url` in the default browser, from core; the
/// terminal interface opens links with it too.
pub(crate) fn browser_command(url: &str) -> std::io::Result<std::process::Command> {
    let mut cmd = ruston_core::browser::opener()?;
    cmd.arg(url);
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    without_secrets(&mut cmd);
    Ok(cmd)
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

/// The browser at `chrome` showing `url` alone, in an app window, on the
/// throwaway `profile`, without asking the questions of a first run.
fn chrome_command(chrome: &str, profile: &std::path::Path, url: &str) -> std::process::Command {
    let mut command = std::process::Command::new(chrome);
    command
        .arg(format!("--user-data-dir={}", profile.display()))
        .args(["--no-first-run", "--no-default-browser-check"])
        .arg(format!("--app={url}"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    without_secrets(&mut command);
    command
}

/// The first Chromium-based browser installed in Applications: Chrome, then
/// Chromium, Brave or Edge.
#[cfg(target_os = "macos")]
fn find_chrome() -> Option<String> {
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ]
    .into_iter()
    .find(|app| std::path::Path::new(app).exists())
    .map(str::to_owned)
}

/// The first Chromium-based browser on `PATH`: Chrome, then Chromium or
/// Brave.
#[cfg(target_os = "linux")]
fn find_chrome() -> Option<String> {
    [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "brave-browser",
    ]
    .into_iter()
    .find_map(which)
}

/// The first of [`windows_browsers`] that is installed.
#[cfg(target_os = "windows")]
fn find_chrome() -> Option<String> {
    windows_browsers()
        .into_iter()
        .find(|path| std::path::Path::new(path).exists())
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

/// The first place on `PATH` that holds `program`.
#[cfg(target_os = "linux")]
fn which(program: &str) -> Option<String> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|folder| folder.join(program))
        .find(|candidate| candidate.exists())
        .map(|found| found.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests;
