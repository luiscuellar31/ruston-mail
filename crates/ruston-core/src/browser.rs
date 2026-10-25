//! The system's way to open a web address in the default browser, which both
//! apps use. No shell reads the address: `cmd /C start` would cut it at the
//! first `&`.

use std::process::Command;

/// The program that opens a web address, given as the next argument, in the
/// default browser.
#[cfg(target_os = "macos")]
pub fn opener() -> std::io::Result<Command> {
    Ok(Command::new("open"))
}

/// The program that opens a web address, given as the next argument, in the
/// default browser.
#[cfg(target_os = "windows")]
pub fn opener() -> std::io::Result<Command> {
    let mut command = Command::new(system_rundll32()?);
    command.arg("url.dll,FileProtocolHandler");
    Ok(command)
}

/// The program that opens a web address, given as the next argument, in the
/// default browser.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn opener() -> std::io::Result<Command> {
    Ok(Command::new("xdg-open"))
}

/// rundll32 from System32. Windows looks for a program named without a
/// folder beside the executable first, where a download could be waiting.
#[cfg(target_os = "windows")]
fn system_rundll32() -> std::io::Result<std::path::PathBuf> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;

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
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])).join("rundll32.exe"))
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(target_os = "windows")]
    fn windows_opens_addresses_with_system32s_rundll32() {
        let command = super::opener().unwrap();
        let program = std::path::Path::new(command.get_program());
        assert!(program.is_absolute(), "{program:?}");
        assert!(program.ends_with("rundll32.exe"), "{program:?}");
        assert!(program.exists(), "{program:?}");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["url.dll,FileProtocolHandler"]);
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn linux_opens_addresses_with_xdg_open() {
        let command = super::opener().unwrap();
        assert_eq!(command.get_program(), "xdg-open");
        assert_eq!(command.get_args().count(), 0);
    }
}
