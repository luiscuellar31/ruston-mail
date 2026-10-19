//! Saves attachments using safe sender names or user-selected destinations,
//! without overwriting existing files.

use ruston_core::mail::attachments::{save_under_free_name, write_new_file};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveError {
    /// No downloads folder to write to.
    NoFolder,
    /// The folder exists but the file could not be written.
    Failed,
    /// Nothing was written because no attachment arrived.
    NotFetched,
    /// A present signature could not be verified, so core withheld the bytes.
    VerificationFailed,
    /// The explicitly selected destination already exists.
    AlreadyExists,
    /// Proton ended the session before the file arrived.
    SessionExpired,
    /// The saved session could not be read or saved.
    SessionUnavailable,
}

impl SaveError {
    pub fn message(self) -> &'static str {
        match self {
            Self::NoFolder => "Ruston Mail could not find a downloads folder to save into.",
            Self::Failed => "Ruston Mail could not save the file.",
            Self::NotFetched => "Ruston Mail could not download the file from Proton.",
            Self::AlreadyExists => {
                "A file already exists at this location. Choose a different name."
            }
            Self::VerificationFailed => {
                "Ruston Mail could not verify this attachment's signature. The file was not saved."
            }
            Self::SessionExpired => {
                "Your Proton session has expired. Sign in again; the file was not saved."
            }
            Self::SessionUnavailable => {
                "Ruston Mail could not access the saved Proton session. Unlock your password \
                 storage and try again; the file was not saved."
            }
        }
    }
}

/// Saves `contents` in the downloads folder under the attachment's name made
/// safe, or a numbered copy of it when taken, returning where it landed.
pub fn save(name: &str, contents: &[u8]) -> Result<PathBuf, SaveError> {
    let folder = directories::UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(Path::to_path_buf))
        .ok_or(SaveError::NoFolder)?;

    save_under_free_name(&folder, name, contents).map_err(|_| SaveError::Failed)
}

/// Uses the exact path chosen by the user while retaining exclusive creation.
/// A file created after the save dialog closes must not be overwritten either.
pub fn save_selected(path: &Path, contents: &[u8]) -> Result<PathBuf, SaveError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(SaveError::Failed);
    }
    match write_new_file(path, contents) {
        Ok(()) => Ok(path.to_path_buf()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(SaveError::AlreadyExists),
        Err(_) => Err(SaveError::Failed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty folder of its own for one test.
    fn scratch_folder(label: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("ruston-save-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn a_file_that_cannot_be_created_is_a_failure_not_a_taken_name() {
        let folder = scratch_folder("missing");
        let missing = folder.join("missing");

        let selected = save_selected(&missing.join("report.pdf"), b"x");
        let created = missing.exists();
        std::fs::remove_dir_all(&folder).unwrap();

        assert_eq!(selected, Err(SaveError::Failed));
        assert!(!created);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn a_selected_destination_is_marked_as_downloaded_from_the_internet() {
        let folder = scratch_folder("zone");
        let mut stream = save_selected(&folder.join("elegido.pdf"), b"%PDF")
            .unwrap()
            .into_os_string();
        stream.push(":Zone.Identifier");
        assert!(
            std::fs::read_to_string(stream)
                .unwrap()
                .contains("ZoneId=3")
        );
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[test]
    fn selected_destination_preserves_existing_files_and_exact_names() {
        let folder = scratch_folder("selected");
        let path = folder.join("informe español 100% # ' final.pdf");
        assert_eq!(save_selected(&path, b"original").unwrap(), path);
        assert_eq!(
            save_selected(&path, b"replacement"),
            Err(SaveError::AlreadyExists)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"original");
        assert_eq!(
            save_selected(Path::new("relative.pdf"), b"x"),
            Err(SaveError::Failed)
        );
        std::fs::remove_dir_all(folder).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn selected_destination_rejects_dangling_symlinks() {
        let folder =
            std::env::temp_dir().join(format!("ruston-save-selected-link-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let outside = folder.join("outside.pdf");
        let path = folder.join("chosen.pdf");
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert_eq!(save_selected(&path, b"x"), Err(SaveError::AlreadyExists));
        assert!(!outside.exists());
        std::fs::remove_dir_all(folder).unwrap();
    }
}
