//! Attachments as files: a name every file system takes, a free one in a
//! folder, writing that never replaces a file, and the mark of a download.

use crate::text::is_invisible_mark;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};

/// The longest name kept, in UTF-8 bytes. NTFS allows 255 UTF-16 units and
/// Linux and macOS file systems 255 bytes; this leaves room for the number
/// [`save_under_free_name`] adds to a copy, " (9999)" at most.
pub(super) const MAX_NAME_BYTES: usize = 240;
/// The longest extension, dot included, that a shortened name keeps; one
/// longer is not much of an extension.
const MAX_KEPT_EXTENSION_BYTES: usize = 16;

/// How many names a saved attachment tries: its own and 9,999 numbered ones.
pub(super) const FREE_NAME_ATTEMPTS: usize = 10_000;

/// Return a portable plain filename for an untrusted attachment name.
/// Invisible direction marks are dropped; path components are removed;
/// characters no file system accepts become `_`; a long name is cut, keeping
/// its extension; empty names become `attachment`, and Windows device names
/// are prefixed with an underscore.
pub fn safe_attachment_name(name: &str) -> String {
    let name: String = name.chars().filter(|&c| !is_invisible_mark(c)).collect();
    let name: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = shorten(trim_name(&name));
    // The cut may end in a space or a dot.
    let name = trim_name(&name);
    let mut components = Path::new(name).components();
    let plain_name =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();

    if name.is_empty() || !plain_name {
        return "attachment".to_owned();
    }

    if is_windows_reserved(name) {
        format!("_{name}")
    } else {
        name.to_owned()
    }
}

/// Leading dots would hide the file on Linux and macOS, and Windows drops
/// trailing dots and spaces, so a name ending in them would be saved under
/// another one than the frontend reports.
fn trim_name(name: &str) -> &str {
    name.trim_start_matches(|c: char| c == '.' || c.is_whitespace())
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
}

/// Cuts a name longer than [`MAX_NAME_BYTES`] at a character boundary,
/// keeping a short extension.
fn shorten(name: &str) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.to_owned();
    }
    let extension = match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= MAX_KEPT_EXTENSION_BYTES => &name[dot..],
        _ => "",
    };
    let stem = &name[..name.len() - extension.len()];
    let mut end = MAX_NAME_BYTES - extension.len();
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{extension}", &stem[..end])
}

fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "CLOCK$"]
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return true;
    }
    // COM and LPT with one digit, the superscripts ¹, ² and ³ included.
    let mut chars = stem.chars();
    let prefix: String = chars.by_ref().take(3).collect();
    let digit = chars.next();
    chars.next().is_none()
        && (prefix.eq_ignore_ascii_case("COM") || prefix.eq_ignore_ascii_case("LPT"))
        && digit.is_some_and(|c| c.is_ascii_digit() || matches!(c, '¹' | '²' | '³'))
}

/// Saves an attachment's `contents` in `folder` under its name made safe,
/// or as `name (1)`, `name (2)` and so on when that is taken, and returns
/// where it landed. Each name is claimed by creating the file exclusively,
/// so a file or symlink that appears meanwhile is never written through.
pub fn save_under_free_name(
    folder: &Path,
    name: &str,
    contents: &[u8],
) -> std::io::Result<PathBuf> {
    let name = safe_attachment_name(name);
    for candidate in free_names(&name).take(FREE_NAME_ATTEMPTS) {
        let path = folder.join(candidate);
        match write_new_file(&path, contents) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        ErrorKind::AlreadyExists,
        "no free name for the attachment",
    ))
}

/// `name`, then `name (1)`, `name (2)` and so on, numbered before the
/// extension.
pub(super) fn free_names(name: &str) -> impl Iterator<Item = String> + '_ {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    std::iter::once(name.to_owned())
        .chain((1..).map(move |number| format!("{stem} ({number}){extension}")))
}

/// Writes `contents` to a new file at `path` and marks it as downloaded.
/// Creating it exclusively fails on anything already there, a dangling
/// symlink included; a file that could not be written in full is removed.
pub fn write_new_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    write_new_file_with(path, |file| file.write_all(contents))
}

/// [`write_new_file`], with `write` filling the file; tests make it fail.
pub(super) fn write_new_file_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Err(error) = write(&mut file) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    // Best effort: a file system without the mark still gets the file.
    let _ = mark_downloaded(path);
    Ok(())
}

/// Marks a saved attachment as downloaded from the internet, as browsers mark
/// their downloads: SmartScreen then checks the file before it runs and
/// Office opens it in Protected View. The mark is an NTFS alternate data
/// stream, so on a file system without them, such as FAT32, the file stays
/// unmarked and this returns the error.
#[cfg(windows)]
pub fn mark_downloaded(path: &Path) -> std::io::Result<()> {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    // Zone 3 is the Internet zone.
    std::fs::write(stream, "[ZoneTransfer]\r\nZoneId=3\r\n")
}

/// Marks a saved attachment as an email attachment from the internet, with
/// the quarantine mark browsers give their downloads: Gatekeeper then checks
/// the file before it runs. The system records this program's name and the
/// time; a volume that cannot hold the mark leaves the file unmarked.
#[cfg(target_os = "macos")]
pub fn mark_downloaded(path: &Path) -> std::io::Result<()> {
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::error::CFError;
    use core_foundation::string::{CFString, CFStringRef};
    use core_foundation::url::{CFURL, CFURLSetResourcePropertyForKey};

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        // core-foundation leaves this key out.
        static kCFURLQuarantinePropertiesKey: CFStringRef;
    }
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        static kLSQuarantineTypeKey: CFStringRef;
        static kLSQuarantineTypeEmailAttachment: CFStringRef;
    }

    let url = CFURL::from_path(path, false)
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "not a file path"))?;
    // SAFETY: both are constant strings that LaunchServices owns; the
    // wrappers retain them for as long as they live.
    let (key, kind) = unsafe {
        (
            CFString::wrap_under_get_rule(kLSQuarantineTypeKey),
            CFString::wrap_under_get_rule(kLSQuarantineTypeEmailAttachment),
        )
    };
    let properties = CFDictionary::from_CFType_pairs(&[(key, kind)]);
    let mut error = std::ptr::null_mut();
    // SAFETY: the URL, the key and the dictionary live through the call, and
    // the error it may return is ours, released by its wrapper below.
    let marked = unsafe {
        CFURLSetResourcePropertyForKey(
            url.as_concrete_TypeRef(),
            kCFURLQuarantinePropertiesKey,
            properties.as_CFTypeRef(),
            &mut error,
        )
    };
    if marked != 0 {
        return Ok(());
    }
    let reason = if error.is_null() {
        "the quarantine mark was refused".to_owned()
    } else {
        // SAFETY: a returned error follows the create rule.
        unsafe { CFError::wrap_under_create_rule(error) }
            .description()
            .to_string()
    };
    Err(std::io::Error::other(reason))
}

/// Marks a saved attachment as downloaded from the internet, as browsers mark
/// their downloads where the system has such a mark; this one has none, so
/// nothing is done.
#[cfg(not(any(windows, target_os = "macos")))]
pub fn mark_downloaded(_path: &Path) -> std::io::Result<()> {
    Ok(())
}
