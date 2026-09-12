//! Writing saved attachments to disk.
//!
//! Files land in the platform's downloads folder under the name the sender
//! chose, with two rules: the name is stripped to a bare file name, so a
//! sender cannot steer the write anywhere else, and an existing file is never
//! overwritten.

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
}

impl SaveError {
    pub fn message(self) -> &'static str {
        match self {
            Self::NoFolder => "Ruston Mail could not find a downloads folder to save into.",
            Self::Failed => "Ruston Mail could not save the file.",
        }
    }
}

/// Saves `contents` in the downloads folder, returning where it landed.
pub fn save(name: &str, contents: &[u8]) -> Result<PathBuf, SaveError> {
    let folder = directories::UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(Path::to_path_buf))
        .ok_or(SaveError::NoFolder)?;

    let path = free_path(&folder, &safe_name(name)).ok_or(SaveError::Failed)?;
    std::fs::write(&path, contents).map_err(|_| SaveError::Failed)?;

    Ok(path)
}

/// The bare file name a sender asked for, with anything that could point
/// outside the downloads folder removed.
fn safe_name(name: &str) -> String {
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .trim()
        .trim_start_matches('.');

    if is_plain_file_name(name) {
        name.to_owned()
    } else {
        "attachment".to_owned()
    }
}

/// Whether this is a name and nothing more. Stripping separators is not
/// enough on its own: Windows reads `C:report.pdf` as a place on another
/// drive, and joining it to the downloads folder does not bring it back.
fn is_plain_file_name(name: &str) -> bool {
    let mut components = Path::new(name).components();

    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

/// The first free name: `report.pdf`, then `report (2).pdf`, and so on. An
/// existing file is left alone.
fn free_path(folder: &Path, name: &str) -> Option<PathBuf> {
    let candidate = folder.join(name);
    if !candidate.exists() {
        return Some(candidate);
    }

    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };

    (2..MAX_ATTEMPTS).find_map(|number| {
        let candidate = folder.join(format!("{stem} ({number}){extension}"));
        (!candidate.exists()).then_some(candidate)
    })
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

        // Whatever the sender sends, what comes back is a name and nothing
        // more, so joining it to the downloads folder cannot leave it.
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
        std::fs::create_dir_all(&folder).unwrap();

        let first = free_path(&folder, "report.pdf").unwrap();
        assert_eq!(first.file_name().unwrap(), "report.pdf");
        std::fs::write(&first, b"one").unwrap();

        let second = free_path(&folder, "report.pdf").unwrap();
        assert_eq!(second.file_name().unwrap(), "report (2).pdf");
        std::fs::write(&second, b"two").unwrap();

        // The first file still holds what it held.
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(
            free_path(&folder, "report.pdf")
                .unwrap()
                .file_name()
                .unwrap(),
            "report (3).pdf"
        );

        // A name with no extension still gets a number.
        let plain = free_path(&folder, "notes").unwrap();
        std::fs::write(&plain, b"x").unwrap();
        assert_eq!(
            free_path(&folder, "notes").unwrap().file_name().unwrap(),
            "notes (2)"
        );

        let _ = std::fs::remove_dir_all(&folder);
    }
}
