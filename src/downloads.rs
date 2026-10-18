//! Saves attachments using safe sender names or user-selected destinations,
//! without overwriting existing files.

use ruston_core::mail::attachments::{mark_downloaded, safe_attachment_name};
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

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

/// Saves `contents` in the downloads folder, returning where it landed.
pub fn save(name: &str, contents: &[u8]) -> Result<PathBuf, SaveError> {
    let folder = directories::UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(Path::to_path_buf))
        .ok_or(SaveError::NoFolder)?;

    save_to(&folder, &safe_attachment_name(name), contents)
}

/// Uses the exact path chosen by the user while retaining exclusive creation.
/// A file created after the save dialog closes must not be overwritten either.
pub fn save_selected(path: &Path, contents: &[u8]) -> Result<PathBuf, SaveError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(SaveError::Failed);
    }
    match create_new_file(path, contents) {
        Ok(()) => Ok(path.to_path_buf()),
        Err(NewFileError::NameTaken) => Err(SaveError::AlreadyExists),
        Err(NewFileError::Failed) => Err(SaveError::Failed),
    }
}

/// Exclusively creates `report.pdf`, then `report (2).pdf`, and so on.
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
        match create_new_file(&path, contents) {
            Ok(()) => return Ok(path),
            Err(NewFileError::NameTaken) => continue,
            Err(NewFileError::Failed) => return Err(SaveError::Failed),
        }
    }

    Err(SaveError::Failed)
}

/// Why [`create_new_file`] left nothing behind.
enum NewFileError {
    /// Something already has the name.
    NameTaken,
    /// The file could not be created, or not written in full.
    Failed,
}

/// Creates a file at `path` only if nothing has that name yet, writes
/// `contents` and marks it as downloaded. Opening and claiming the name in one
/// operation prevents races and symlink writes. A file that could not be
/// written in full is removed.
fn create_new_file(path: &Path, contents: &[u8]) -> Result<(), NewFileError> {
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            return Err(NewFileError::NameTaken);
        }
        Err(_) => return Err(NewFileError::Failed),
    };
    if file.write_all(contents).is_err() {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(NewFileError::Failed);
    }
    // Best effort: a file system without the mark still gets the file.
    let _ = mark_downloaded(path);
    Ok(())
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

        let saved = save_to(&missing, "report.pdf", b"x");
        let selected = save_selected(&missing.join("report.pdf"), b"x");
        let created = missing.exists();
        std::fs::remove_dir_all(&folder).unwrap();

        assert_eq!(saved, Err(SaveError::Failed));
        assert_eq!(selected, Err(SaveError::Failed));
        assert!(!created);
    }

    #[test]
    fn a_name_a_hundred_files_already_have_is_not_saved() {
        let folder = scratch_folder("full");
        std::fs::write(folder.join("report.pdf"), b"first").unwrap();
        for number in 2..=MAX_ATTEMPTS {
            std::fs::write(folder.join(format!("report ({number}).pdf")), b"older").unwrap();
        }

        let saved = save_to(&folder, "report.pdf", b"newest");
        let first = std::fs::read(folder.join("report.pdf")).unwrap();
        let files = std::fs::read_dir(&folder).unwrap().count();
        std::fs::remove_dir_all(&folder).unwrap();

        assert_eq!(saved, Err(SaveError::Failed));
        assert_eq!(first, b"first");
        assert_eq!(files, 100);
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn saved_files_are_marked_as_downloaded_from_the_internet() {
        let folder = scratch_folder("zone");
        let saved = save_to(&folder, "factura.pdf", b"%PDF").unwrap();
        let selected = save_selected(&folder.join("elegido.pdf"), b"%PDF").unwrap();
        for path in [saved, selected] {
            let mut stream = path.into_os_string();
            stream.push(":Zone.Identifier");
            assert!(
                std::fs::read_to_string(stream)
                    .unwrap()
                    .contains("ZoneId=3")
            );
        }
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

    #[test]
    fn an_existing_file_is_never_overwritten() {
        let folder = scratch_folder("existing");

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

        let folder = Arc::new(scratch_folder("concurrent"));
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
    fn saving_reserved_name_creates_prefixed_files_and_deduplicates() {
        let folder = scratch_folder("reserved");

        let first = save_to(&folder, &safe_attachment_name("CON.pdf"), b"one").unwrap();
        assert_eq!(first.file_name().unwrap(), "_CON.pdf");

        let second = save_to(&folder, &safe_attachment_name("CON.pdf"), b"two").unwrap();
        assert_eq!(second.file_name().unwrap(), "_CON (2).pdf");

        let third = save_to(&folder, &safe_attachment_name("CON.pdf"), b"three").unwrap();
        assert_eq!(third.file_name().unwrap(), "_CON (3).pdf");

        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        assert_eq!(std::fs::read(&third).unwrap(), b"three");

        let _ = std::fs::remove_dir_all(&folder);
    }
}
