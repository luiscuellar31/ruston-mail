//! Per-user desktop activation. Only bounded, authenticated mailto requests
//! cross the loopback connection; message contents never reach the filesystem.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(test)]
mod tests;

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use eframe::egui;

use crate::mailto::{MAX_PENDING, MAX_URL_BYTES, Request};

const TIMEOUT: Duration = Duration::from_secs(1);
/// How long a connection may take to authenticate. Any local user can connect
/// to the loopback port and the worker serves one connection at a time, so
/// idle connections must not use up a later launch's retries.
const HEADER_TIMEOUT: Duration = Duration::from_millis(200);
const MAGIC: &[u8; 4] = b"RM01";
/// Forwarded in place of a mailto link over the size limit, which the
/// receiving process would drop without an answer. Every version refuses to
/// open a lone `%`, so the window still says the link was not opened.
const OVERSIZED_LINK: &str = "mailto:%";

pub enum Start {
    Forwarded,
    Primary(Instance),
}

/// Startup arguments retain their original URI only for forwarding to another
/// process. Within this process, the parsed request is carried directly; a
/// mailto link this version cannot open has none.
pub struct Argument {
    url: String,
    request: Option<Request>,
}

#[derive(Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub enum Event {
    Activate,
    Mailto(Request),
    Rejected,
}

#[derive(Clone)]
struct Sender {
    events: mpsc::SyncSender<Event>,
    context: Arc<Mutex<Option<egui::Context>>>,
    overflow: Arc<AtomicBool>,
}

impl Sender {
    fn send(&self, event: Event) -> bool {
        let accepted = self.events.try_send(event).is_ok();
        if !accepted {
            self.overflow.store(true, Ordering::Relaxed);
        }
        if let Some(context) = self.context.lock().ok().and_then(|context| context.clone()) {
            context.request_repaint();
        }
        accepted
    }
}

pub struct Instance {
    // The OS releases this lock after a crash. Never unlink the lock file:
    // another process could otherwise lock a different inode at the same path.
    _lock: File,
    endpoint: PathBuf,
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    receiver: mpsc::Receiver<Event>,
    sender: Sender,
    #[cfg(target_os = "macos")]
    _urls: Option<macos::UrlEvents>,
}

impl Instance {
    pub fn start(demo: bool, urls: Vec<Argument>) -> io::Result<Start> {
        let directories =
            directories::ProjectDirs::from("", "", "Ruston Mail").ok_or_else(|| {
                io::Error::other("could not find the desktop configuration directory")
            })?;
        // Windows roams the configuration directory with the user's profile,
        // sometimes to a network share, where another computer would find the
        // lock held. The lock and endpoint belong to this computer.
        #[cfg(target_os = "windows")]
        let base = directories.data_local_dir();
        #[cfg(not(target_os = "windows"))]
        let base = directories.config_dir();
        // A demo process must never forward an email link to a real account.
        let folder = base.join(if demo {
            "activation-demo"
        } else {
            "activation"
        });
        let start = Self::start_at(&folder, urls)?;
        #[cfg(target_os = "macos")]
        let start = match start {
            Start::Primary(mut instance) => {
                instance._urls = Some(macos::UrlEvents::install(instance.sender.clone())?);
                Start::Primary(instance)
            }
            other => other,
        };
        Ok(start)
    }

    fn start_at(folder: &Path, urls: Vec<Argument>) -> io::Result<Start> {
        if urls.len() > MAX_PENDING {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid email link",
            ));
        }
        private_directory(folder)?;
        let endpoint = folder.join("endpoint");
        let lock = match find_primary(folder, &endpoint)? {
            Found::Lock(lock) => lock,
            Found::Primary(stream) => {
                forward_to_primary(stream, urls)?;
                return Ok(Start::Forwarded);
            }
        };
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let address = listener.local_addr()?;
        let mut token = [0_u8; 32];
        getrandom::fill(&mut token).map_err(|error| io::Error::other(error.to_string()))?;
        let (events, receiver) = mpsc::sync_channel(MAX_PENDING);
        let sender = Sender {
            events,
            context: Arc::new(Mutex::new(None)),
            overflow: Arc::new(AtomicBool::new(false)),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_sender = sender.clone();
        let worker = thread::Builder::new()
            .name("ruston-activation".into())
            .spawn(move || {
                for connection in listener.incoming() {
                    if worker_stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if let Ok(mut stream) = connection {
                        let _ = receive(&mut stream, &token, &worker_sender);
                    }
                }
            })?;
        let instance = Self {
            _lock: lock,
            endpoint,
            address,
            stop,
            worker: Some(worker),
            receiver,
            sender,
            #[cfg(target_os = "macos")]
            _urls: None,
        };
        // Seed startup links before making the endpoint available to secondaries.
        for argument in urls {
            instance
                .sender
                .send(argument.request.map_or(Event::Rejected, Event::Mailto));
        }
        let record = EndpointRecord {
            port: address.port(),
            token,
            process: std::process::id(),
        };
        // The window opens without a record too; later launches just cannot
        // hand it their links. Desktop logging is off by default, so say so on
        // standard error.
        if let Err(error) = record.publish(&instance.endpoint) {
            eprintln!("Ruston Mail: later launches cannot pass links to this window: {error}");
        }
        Ok(Start::Primary(instance))
    }

    pub fn attach(&self, context: &egui::Context) {
        if let Ok(mut current) = self.sender.context.lock() {
            *current = Some(context.clone());
        }
    }

    pub fn drain(&self) -> Vec<Event> {
        let mut events: Vec<_> = self.receiver.try_iter().take(MAX_PENDING).collect();
        if self.sender.overflow.swap(false, Ordering::Relaxed) {
            events.push(Event::Rejected);
        }
        events
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake blocking accept without leaving a polling thread behind.
        let _ = TcpStream::connect_timeout(&self.address, TIMEOUT);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = fs::remove_file(&self.endpoint);
    }
}

fn private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(io::Error::other(
            "activation directory is not a regular directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn private_file(path: &Path, truncate: bool) -> io::Result<File> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && !metadata.is_file()
    {
        return Err(io::Error::other("activation file is not a regular file"));
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .truncate(truncate);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

/// What a later launch reads to reach the running instance. Launches of other
/// versions read it too, so its layout stays fixed: the port, the token and
/// the process ID, big-endian.
struct EndpointRecord {
    port: u16,
    token: [u8; 32],
    /// Lets a later launch hand this process the foreground on Windows.
    process: u32,
}

impl EndpointRecord {
    const LEN: usize = 2 + 32 + 4;

    fn encode(&self) -> [u8; Self::LEN] {
        let mut bytes = [0_u8; Self::LEN];
        let (port, rest) = bytes.split_at_mut(size_of::<u16>());
        let (token, process) = rest.split_at_mut(self.token.len());
        port.copy_from_slice(&self.port.to_be_bytes());
        token.copy_from_slice(&self.token);
        process.copy_from_slice(&self.process.to_be_bytes());
        bytes
    }

    fn decode(bytes: [u8; Self::LEN]) -> Self {
        let [port_high, port_low, token @ .., p0, p1, p2, p3] = bytes;
        Self {
            port: u16::from_be_bytes([port_high, port_low]),
            token,
            process: u32::from_be_bytes([p0, p1, p2, p3]),
        }
    }

    /// Writes the record later launches read to reach this process. It lives
    /// only as long as the process and other processes read it from the page
    /// cache, so it is not flushed to disk: on macOS that would be a full
    /// device flush.
    fn publish(&self, endpoint: &Path) -> io::Result<()> {
        private_file(endpoint, true)?.write_all(&self.encode())
    }

    fn read(endpoint: &Path) -> io::Result<Self> {
        if !fs::symlink_metadata(endpoint)?.is_file() {
            return Err(io::Error::other(
                "activation endpoint is not a regular file",
            ));
        }
        let mut bytes = [0_u8; Self::LEN];
        File::open(endpoint)?.read_exact(&mut bytes)?;
        Ok(Self::decode(bytes))
    }
}

/// Takes the per-user instance lock, or returns `None` while another process
/// holds it.
fn claim(folder: &Path, endpoint: &Path) -> io::Result<Option<File>> {
    let lock = private_file(&folder.join("instance.lock"), false)?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(None),
        Err(TryLockError::Error(error)) => return Err(error),
    }
    // A stale record must not route a URL to a port reused after a crash.
    match fs::remove_file(endpoint) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(Some(lock))
}

/// What a launch found: the instance lock, now its own, or the running
/// instance to hand its links to.
enum Found {
    Lock(File),
    Primary(TcpStream),
}

/// Takes the instance lock, or connects to the instance that holds it. That
/// instance may still be publishing its endpoint, or be closing, when its
/// lock comes free and this launch takes over. Only discovery and connect are
/// retried, never a request after bytes have been sent.
fn find_primary(folder: &Path, endpoint: &Path) -> io::Result<Found> {
    let deadline = Instant::now() + TIMEOUT * 3;
    loop {
        if let Some(lock) = claim(folder, endpoint)? {
            return Ok(Found::Lock(lock));
        }
        match connect(endpoint) {
            Ok(stream) => return Ok(Found::Primary(stream)),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => thread::sleep(Duration::from_millis(25)),
        }
    }
}

/// Hands this launch's links, or a plain activation, to the running instance.
fn forward_to_primary(mut stream: TcpStream, urls: Vec<Argument>) -> io::Result<()> {
    if urls.is_empty() {
        return forward(&mut stream, "");
    }
    for argument in urls {
        let url = if argument.url.len() > MAX_URL_BYTES {
            OVERSIZED_LINK
        } else {
            &argument.url
        };
        forward(&mut stream, url)?;
    }
    Ok(())
}

fn connect(endpoint: &Path) -> io::Result<TcpStream> {
    let record = EndpointRecord::read(endpoint)?;
    let mut stream = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, record.port)),
        TIMEOUT,
    )?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.write_all(MAGIC)?;
    stream.write_all(&record.token)?;
    let mut acknowledgement = [0];
    stream.read_exact(&mut acknowledgement)?;
    if acknowledgement != [1] {
        return Err(io::Error::other("activation handshake failed"));
    }
    #[cfg(target_os = "windows")]
    {
        // SAFETY: this API accepts a process ID by value. The launching process
        // can grant its foreground rights to the authenticated existing instance;
        // Windows may still decline if it has no foreground rights itself.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(record.process);
        }
    }
    Ok(stream)
}

fn forward(stream: &mut TcpStream, url: &str) -> io::Result<()> {
    stream.write_all(&(url.len() as u32).to_be_bytes())?;
    stream.write_all(url.as_bytes())?;
    let mut response = [0];
    stream.read_exact(&mut response)?;
    if response == [1] {
        Ok(())
    } else {
        Err(io::Error::other(
            "email link was not accepted; please try again later",
        ))
    }
}

fn receive(stream: &mut TcpStream, token: &[u8; 32], sender: &Sender) -> io::Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let header_deadline = Instant::now() + HEADER_TIMEOUT;
    let mut magic = [0_u8; 4];
    let mut received = [0_u8; 32];
    read_until(stream, &mut magic, header_deadline)?;
    read_until(stream, &mut received, header_deadline)?;
    if &magic != MAGIC || !same_token(&received, token) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid activation token",
        ));
    }
    stream.write_all(&[1])?;
    // One connection forwards up to the bounded command-line request count.
    for _ in 0..MAX_PENDING {
        let mut length = [0_u8; 4];
        read_until(stream, &mut length, deadline)?;
        let length = u32::from_be_bytes(length) as usize;
        if length > MAX_URL_BYTES {
            return Err(io::Error::other("email link exceeds the size limit"));
        }
        let mut bytes = vec![0_u8; length];
        read_until(stream, &mut bytes, deadline)?;
        let url = String::from_utf8(bytes).map_err(|_| io::Error::other("invalid email link"))?;
        // The wire is an external boundary: authenticate and validate here,
        // then move the parsed request through the rest of this process.
        let event = if url.is_empty() {
            Some(Event::Activate)
        } else {
            match Request::parse(&url) {
                Some(request) => Some(Event::Mailto(request)),
                // The window still says a link arrived and was not opened.
                None if Request::is_mailto(&url) => Some(Event::Rejected),
                None => None,
            }
        };
        let accepted = event.is_some_and(|event| sender.send(event));
        stream.write_all(&[u8::from(accepted)])?;
        if !accepted {
            break;
        }
    }
    Ok(())
}

/// Compares every byte whatever the first difference, so the time a guess
/// takes to be rejected does not say how much of it was right.
fn same_token(received: &[u8; 32], token: &[u8; 32]) -> bool {
    let mut difference = 0_u8;
    for (a, b) in received.iter().zip(token) {
        difference |= std::hint::black_box(a ^ b);
    }
    difference == 0
}

/// A total deadline prevents a slow peer from extending read_exact indefinitely
/// by sending one byte before each socket timeout, including during shutdown.
fn read_until(stream: &mut TcpStream, mut buffer: &mut [u8], deadline: Instant) -> io::Result<()> {
    while !buffer.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "activation timed out"))?;
        stream.set_read_timeout(Some(remaining))?;
        match stream.read(buffer) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "incomplete activation request",
                ));
            }
            Ok(count) => buffer = &mut buffer[count..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Desktop URI arguments are data, never shell commands or frontend switches.
/// Anything but a mailto link stops the launch. A mailto link this version
/// cannot open still reaches the window, which says it was not opened.
pub fn arguments(args: impl IntoIterator<Item = std::ffi::OsString>) -> io::Result<Vec<Argument>> {
    let mut urls = Vec::new();
    for arg in args {
        if arg == "--" {
            continue;
        }
        // Finder may still provide a process serial number on older macOS.
        #[cfg(target_os = "macos")]
        if arg.to_string_lossy().starts_with("-psn_") {
            continue;
        }
        let url = arg
            .into_string()
            .map_err(|_| io::Error::other("invalid desktop argument"))?;
        if urls.len() == MAX_PENDING {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a valid mailto link",
            ));
        }
        let request = Request::parse(&url);
        if request.is_none() && !Request::is_mailto(&url) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a valid mailto link",
            ));
        }
        urls.push(Argument { url, request });
    }
    Ok(urls)
}
