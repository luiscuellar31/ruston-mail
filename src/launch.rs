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
const MAGIC: &[u8; 4] = b"RM01";

pub enum Start {
    Forwarded,
    Primary(Instance),
}

/// Startup arguments retain their original URI only for forwarding to another
/// process. Within this process, the parsed request is carried directly.
pub struct Argument {
    url: String,
    request: Request,
}

#[derive(Clone, PartialEq, Eq)]
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
        // A demo process must never forward an email link to a real account.
        let folder = directories.config_dir().join(if demo {
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
        let lock = private_file(&folder.join("instance.lock"), false)?;
        let endpoint = folder.join("endpoint");
        match lock.try_lock() {
            Err(TryLockError::WouldBlock) => {
                // The primary may still be publishing its endpoint. Retry only
                // discovery/connect, never a request after bytes have been sent.
                let deadline = Instant::now() + TIMEOUT * 3;
                let mut stream = loop {
                    match connect(&endpoint) {
                        Ok(stream) => break stream,
                        Err(error) if Instant::now() >= deadline => return Err(error),
                        Err(_) => thread::sleep(Duration::from_millis(25)),
                    }
                };
                if urls.is_empty() {
                    forward(&mut stream, "")?;
                } else {
                    for argument in urls {
                        forward(&mut stream, &argument.url)?;
                    }
                }
                return Ok(Start::Forwarded);
            }
            Err(TryLockError::Error(error)) => return Err(error),
            Ok(()) => {}
        }
        // A stale record must not route a URL to a port reused after a crash.
        match fs::remove_file(&endpoint) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
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
            instance.sender.send(Event::Mailto(argument.request));
        }
        let mut file = private_file(&instance.endpoint, true)?;
        file.write_all(&address.port().to_be_bytes())?;
        file.write_all(&token)?;
        file.write_all(&std::process::id().to_be_bytes())?;
        file.sync_all()?;
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

fn connect(endpoint: &Path) -> io::Result<TcpStream> {
    if !fs::symlink_metadata(endpoint)?.is_file() {
        return Err(io::Error::other(
            "activation endpoint is not a regular file",
        ));
    }
    let mut record = [0_u8; 38];
    File::open(endpoint)?.read_exact(&mut record)?;
    let port = u16::from_be_bytes([record[0], record[1]]);
    let mut stream =
        TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), TIMEOUT)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.write_all(MAGIC)?;
    stream.write_all(&record[2..34])?;
    let mut acknowledgement = [0];
    stream.read_exact(&mut acknowledgement)?;
    if acknowledgement != [1] {
        return Err(io::Error::other("activation handshake failed"));
    }
    #[cfg(target_os = "windows")]
    {
        let process = u32::from_be_bytes(record[34..38].try_into().expect("fixed endpoint record"));
        // SAFETY: this API accepts a process ID by value. The launching process
        // can grant its foreground rights to the authenticated existing instance;
        // Windows may still decline if it has no foreground rights itself.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(process);
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
    let mut header = [0_u8; 36];
    read_until(stream, &mut header, deadline)?;
    if &header[..4] != MAGIC || &header[4..] != token {
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
            Request::parse(&url).map(Event::Mailto)
        };
        let accepted = event.is_some_and(|event| sender.send(event));
        stream.write_all(&[u8::from(accepted)])?;
        if !accepted {
            break;
        }
    }
    Ok(())
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
        let request = Request::parse(&url).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "expected a valid mailto link")
        })?;
        urls.push(Argument { url, request });
    }
    Ok(urls)
}
