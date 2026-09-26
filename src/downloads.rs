//! Saves attachments without accepting paths or overwriting existing files.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};

/// Tried before giving up on a name, which only happens when a hundred files
/// already share it.
const MAX_ATTEMPTS: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveError {
    /// No downloads folder to write to.
    NoFolder,
    /// The folder exists but the file could not be written.
    Failed,
    /// Nothing was written because no attachment arrived.
    NotFetched,
}

impl SaveError {
    pub fn message(self) -> &'static str {
        match self {
            Self::NoFolder => "Ruston Mail could not find a downloads folder to save into.",
            Self::Failed => "Ruston Mail could not save the file.",
            Self::NotFetched => "Ruston Mail could not download the file from Proton.",
        }
    }
}

/// Saves `contents` in the downloads folder, returning where it landed.
pub fn save(name: &str, contents: &[u8]) -> Result<PathBuf, SaveError> {
    let folder = directories::UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(Path::to_path_buf))
        .ok_or(SaveError::NoFolder)?;

    save_to(&folder, &safe_name(name), contents)
}

/// The bare file name a sender asked for, with anything that could point
/// outside the downloads folder or trigger Windows device names sanitized.
fn safe_name(name: &str) -> String {
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('.')
        .trim_end_matches('.');

    if !is_plain_file_name(name) {
        return "attachment".to_owned();
    }

    if is_windows_reserved(name) {
        format!("_{name}")
    } else {
        name.to_owned()
    }
}

/// Whether this is a bare name, including on Windows drive-qualified paths.
fn is_plain_file_name(name: &str) -> bool {
    let mut components = Path::new(name).components();

    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

/// Whether `name` matches a Windows DOS device reserved name (e.g. `CON`, `PRN`,
/// `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, `CONIN$`, `CONOUT$`, `CLOCK$`),
/// with or without an extension.
///
/// On Windows, attempting to create or open a file with these names fails or
/// targets a legacy device driver instead of the filesystem.
fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);

    if stem.eq_ignore_ascii_case("CON")
        || stem.eq_ignore_ascii_case("PRN")
        || stem.eq_ignore_ascii_case("AUX")
        || stem.eq_ignore_ascii_case("NUL")
        || stem.eq_ignore_ascii_case("CONIN$")
        || stem.eq_ignore_ascii_case("CONOUT$")
        || stem.eq_ignore_ascii_case("CLOCK$")
    {
        return true;
    }

    let bytes = stem.as_bytes();
    if bytes.len() == 4
        && (bytes[..3].eq_ignore_ascii_case(b"COM") || bytes[..3].eq_ignore_ascii_case(b"LPT"))
        && bytes[3].is_ascii_digit()
    {
        return true;
    }

    false
}

/// Exclusively creates `report.pdf`, then `report (2).pdf`, and so on. Opening
/// and claiming the name in one operation prevents races and symlink writes.
fn save_to(folder: &Path, name: &str, contents: &[u8]) -> Result<PathBuf, SaveError> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };

    for attempt in 0..MAX_ATTEMPTS {
        let path = if attempt == 0 {
            folder.join(name)
        } else {
            folder.join(format!("{stem} ({}){extension}", attempt + 1))
        };
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(SaveError::Failed),
        };
        if file.write_all(contents).is_err() {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(SaveError::Failed);
        }
        return Ok(path);
    }

    Err(SaveError::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sender_cannot_steer_the_write_out_of_the_folder() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name(r"..\..\windows\system32"), "system32");
        assert_eq!(safe_name("/etc/hosts"), "hosts");
        // A name that is nothing but dots or spaces still needs a file.
        assert_eq!(safe_name("   "), "attachment");
        assert_eq!(safe_name("..."), "attachment");
        assert_eq!(safe_name(""), "attachment");
        // An ordinary name is left exactly as the sender wrote it.
        assert_eq!(safe_name("Q3 report.pdf"), "Q3 report.pdf");

        // The sanitized name cannot escape the downloads folder.
        for name in [
            "../../etc/passwd",
            "C:report.pdf",
            r"C:\Windows\evil.exe",
            r"\\server\share\file",
            ".",
            "..",
            "/",
        ] {
            assert!(
                is_plain_file_name(&safe_name(name)),
                "{name} did not reduce to a plain file name"
            );
        }
    }

    #[test]
    fn a_drive_qualified_name_does_not_pass_for_a_file_name() {
        // On Windows this is a path on drive C, not a file called `C:report`.
        // Everywhere else it is an ordinary, if odd, name.
        assert_eq!(is_plain_file_name("C:report.pdf"), !cfg!(windows));
        assert!(is_plain_file_name("report.pdf"));
        assert!(!is_plain_file_name(""));
        assert!(!is_plain_file_name("."));
    }

    #[test]
    fn an_existing_file_is_never_overwritten() {
        let folder = std::env::temp_dir().join(format!("ruston-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();

        let first = save_to(&folder, "report.pdf", b"one").unwrap();
        assert_eq!(first.file_name().unwrap(), "report.pdf");

        let second = save_to(&folder, "report.pdf", b"two").unwrap();
        assert_eq!(second.file_name().unwrap(), "report (2).pdf");

        // The first file still holds what it held.
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(
            save_to(&folder, "report.pdf", b"three")
                .unwrap()
                .file_name()
                .unwrap(),
            "report (3).pdf"
        );

        // A name with no extension still gets a number.
        save_to(&folder, "notes", b"x").unwrap();
        assert_eq!(
            save_to(&folder, "notes", b"y")
                .unwrap()
                .file_name()
                .unwrap(),
            "notes (2)"
        );

        let _ = std::fs::remove_dir_all(&folder);
    }

    #[cfg(unix)]
    #[test]
    fn a_dangling_symlink_cannot_steer_the_write_out_of_the_folder() {
        use std::os::unix::fs::symlink;

        let folder =
            std::env::temp_dir().join(format!("ruston-save-symlink-{}", std::process::id()));
        let outside = folder.with_extension("outside");
        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::remove_file(&outside);
        std::fs::create_dir_all(&folder).unwrap();
        symlink(&outside, folder.join("report.pdf")).unwrap();

        let selected = save_to(&folder, "report.pdf", b"mail").unwrap();
        let escaped = outside.exists();
        let selected_name = selected.file_name().unwrap().to_owned();

        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::remove_file(&outside);
        assert!(!escaped, "the write followed a dangling symlink");
        assert_eq!(selected_name, "report (2).pdf");
    }

    #[test]
    fn concurrent_saves_claim_different_names() {
        use std::collections::HashSet;
        use std::sync::{Arc, Barrier};

        let folder =
            std::env::temp_dir().join(format!("ruston-save-concurrent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let folder = Arc::new(folder);
        let barrier = Arc::new(Barrier::new(8));

        let threads: Vec<_> = (0_u8..8)
            .map(|contents| {
                let folder = Arc::clone(&folder);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let path = save_to(&folder, "report.pdf", &[contents]).unwrap();
                    (path, contents)
                })
            })
            .collect();
        let saves: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();

        let mut paths = HashSet::new();
        for (path, contents) in saves {
            assert!(paths.insert(path.clone()));
            assert_eq!(std::fs::read(path).unwrap(), [contents]);
        }

        let _ = std::fs::remove_dir_all(folder.as_ref());
    }

    #[test]
    fn windows_reserved_device_names_are_prefixed_safely() {
        // Direct reserved devices
        assert_eq!(safe_name("CON.txt"), "_CON.txt");
        assert_eq!(safe_name("con.pdf"), "_con.pdf");
        assert_eq!(safe_name("PRN.dat"), "_PRN.dat");
        assert_eq!(safe_name("prn"), "_prn");
        assert_eq!(safe_name("AUX.h"), "_AUX.h");
        assert_eq!(safe_name("aux.tar.gz"), "_aux.tar.gz");
        assert_eq!(safe_name("NUL.zip"), "_NUL.zip");
        assert_eq!(safe_name("nul"), "_nul");
        assert_eq!(safe_name("COM1.txt"), "_COM1.txt");
        assert_eq!(safe_name("com9.bin"), "_com9.bin");
        assert_eq!(safe_name("LPT1.doc"), "_LPT1.doc");
        assert_eq!(safe_name("lpt9.pdf"), "_lpt9.pdf");
        assert_eq!(safe_name("CONIN$.log"), "_CONIN$.log");
        assert_eq!(safe_name("CONOUT$"), "_CONOUT$");
        assert_eq!(safe_name("CLOCK$"), "_CLOCK$");

        // Paths containing reserved device names
        assert_eq!(safe_name(r"C:\temp\CON.pdf"), "_CON.pdf");
        assert_eq!(safe_name("../../aux.png"), "_aux.png");
        assert_eq!(safe_name("con."), "_con");
    }

    #[test]
    fn non_reserved_similar_names_are_unmodified() {
        assert_eq!(safe_name("contact.pdf"), "contact.pdf");
        assert_eq!(safe_name("conversation.txt"), "conversation.txt");
        assert_eq!(safe_name("auxiliary.c"), "auxiliary.c");
        assert_eq!(safe_name("null.json"), "null.json");
        assert_eq!(safe_name("COM10.txt"), "COM10.txt");
        assert_eq!(safe_name("printer.png"), "printer.png");
    }

    #[test]
    fn saving_reserved_name_creates_prefixed_files_and_deduplicates() {
        let folder =
            std::env::temp_dir().join(format!("ruston-save-reserved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();

        let first = save_to(&folder, &safe_name("CON.pdf"), b"one").unwrap();
        assert_eq!(first.file_name().unwrap(), "_CON.pdf");

        let second = save_to(&folder, &safe_name("CON.pdf"), b"two").unwrap();
        assert_eq!(second.file_name().unwrap(), "_CON (2).pdf");

        let third = save_to(&folder, &safe_name("CON.pdf"), b"three").unwrap();
        assert_eq!(third.file_name().unwrap(), "_CON (3).pdf");

        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        assert_eq!(std::fs::read(&third).unwrap(), b"three");

        let _ = std::fs::remove_dir_all(&folder);
    }
}
