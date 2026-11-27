//! Files only their owner can open, for decrypted mail written to disk.

use std::fs::File;
use std::io;
use std::path::Path;

/// Creates a new file at `path` for writing that only its owner can open,
/// for decrypted mail: mode `0600` on Unix, and on Windows an access list
/// for the owner, the system and administrators, as a user's profile
/// folders have, that takes nothing from the folder it is in. Otherwise a
/// file in a shared folder would let every user who can read there read it
/// too. Fails if `path` exists.
pub fn create_private(path: &Path) -> io::Result<File> {
    create_owner_only(path)
}

/// Creates the file with mode `0600`.
#[cfg(unix)]
fn create_owner_only(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Creates the file with the system's own permissions, where there is no
/// owner-only mode to ask for.
#[cfg(not(any(unix, windows)))]
fn create_owner_only(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Full access for the owner, the system and administrators, in a protected
/// list that inherits nothing.
#[cfg(windows)]
const PRIVATE_FILE_ACCESS: &str = "D:P(A;;FA;;;OW)(A;;FA;;;SY)(A;;FA;;;BA)";

/// Creates the file with its access list already in place, so no other user
/// can open it, and keep it open, before it is restricted.
#[cfg(windows)]
fn create_owner_only(path: &Path) -> io::Result<File> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    let name = verbatim(path)?;
    let access: Vec<u16> = PRIVATE_FILE_ACCESS.encode_utf16().chain([0]).collect();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: `access` is NUL-terminated, and on success the descriptor is
    // this function's to free with LocalFree.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            access.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: `name` is NUL-terminated and `attributes` points at the live
    // descriptor. Sharing is std's, so the file can be renamed while open.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    let created = if handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: the handle was just opened, and nothing else owns it.
        Ok(unsafe { File::from_raw_handle(handle) })
    };
    // SAFETY: the descriptor came from the conversion above, and the file
    // holds its own copy.
    unsafe { LocalFree(descriptor) };
    created
}

/// `path` as CreateFileW takes it at any length: absolute and verbatim, the
/// form std's own file functions use for long names.
#[cfg(windows)]
fn verbatim(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let wide = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    let absolute: Vec<u16> = std::path::absolute(path)?
        .as_os_str()
        .encode_wide()
        .collect();
    let mut name = if absolute.starts_with(&wide(r"\\?\")) || absolute.starts_with(&wide(r"\\.\")) {
        absolute
    } else if let Some(share) = absolute.strip_prefix(wide(r"\\").as_slice()) {
        [wide(r"\\?\UNC\").as_slice(), share].concat()
    } else {
        [wide(r"\\?\"), absolute].concat()
    };
    name.push(0);
    Ok(name)
}

// Only Windows has an access list to check; elsewhere the mode is std's.
#[cfg(all(test, windows))]
mod tests {
    use super::*;

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
}
