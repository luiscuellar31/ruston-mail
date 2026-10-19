use std::any::Any;
use std::fs::OpenOptions;
use std::os::windows::io::IntoRawHandle;
use std::panic::PanicHookInfo;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType};
use windows_sys::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, SetConsoleCtrlHandler,
    SetStdHandle,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW,
};

/// The main thread's last panic, with its location, which the payload lacks.
static MAIN_PANIC: Mutex<Option<String>> = Mutex::new(None);

pub(super) fn attach_parent_console() {
    // SAFETY: querying a standard handle has no preconditions.
    let current = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    if !current.is_null() && current != INVALID_HANDLE_VALUE {
        // SAFETY: `current` is this process's standard error handle.
        let kind = unsafe { GetFileType(current) };
        // A file or pipe was handed down, by a redirect or `cargo run`.
        if kind == FILE_TYPE_DISK || kind == FILE_TYPE_PIPE {
            return;
        }
    }
    // SAFETY: attaching to the parent's console has no preconditions; it fails
    // when the parent has none, as when Ruston starts from the Start menu.
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return;
    }
    // SAFETY: a null routine with TRUE only makes this process ignore Ctrl+C,
    // which is meant for the shell sharing the console, not for this window.
    unsafe { SetConsoleCtrlHandler(None, 1) };
    if let Ok(console) = OpenOptions::new().write(true).open("CONOUT$") {
        // SAFETY: the handle stays open for the rest of the process, as
        // standard error.
        unsafe { SetStdHandle(STD_ERROR_HANDLE, console.into_raw_handle()) };
    }
}

pub(super) fn remember_panic(info: &PanicHookInfo<'_>) {
    if std::thread::current().name() == Some("main")
        && let Ok(mut panic) = MAIN_PANIC.lock()
    {
        *panic = Some(info.to_string());
    }
}

pub(super) fn panic_description(payload: &(dyn Any + Send)) -> String {
    if let Some(description) = MAIN_PANIC.lock().ok().and_then(|mut panic| panic.take()) {
        return description;
    }
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown error".to_owned())
}

pub(super) fn show_error(message: &str) {
    let text = wide(message);
    let caption = wide("Ruston Mail");
    // SAFETY: both strings are live, NUL-terminated UTF-16 for the whole call,
    // and a null owner window is allowed.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
