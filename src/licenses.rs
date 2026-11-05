//! Finds the notices of the third-party software Ruston Mail is built with,
//! which every package ships, so Settings can open them.

use std::path::{Path, PathBuf};

/// The file `packaging/third-party-licenses.py` writes and every package
/// carries.
const THIRD_PARTY_NOTICES: &str = "THIRD_PARTY_LICENSES.txt";

/// The notices that came with the running app. A build run from Cargo's
/// output folder has none.
pub fn third_party_notices() -> Option<PathBuf> {
    notices_near(&std::env::current_exe().ok()?)
}

/// The notices where a package puts them around `executable`: beside it in
/// the Windows ZIP and installation folder, in `Contents/Resources` beside
/// the macOS app's `Contents/MacOS`, and in `usr/share/doc/ruston-mail`
/// beside the AppImage's `usr/bin`. Every system looks in all three, so
/// each layout is tested on any of them.
fn notices_near(executable: &Path) -> Option<PathBuf> {
    let folder = executable.parent()?;
    let above = folder.parent();
    [
        Some(folder.to_path_buf()),
        above.map(|contents| contents.join("Resources")),
        above.map(|usr| usr.join("share").join("doc").join("ruston-mail")),
    ]
    .into_iter()
    .flatten()
    .map(|place| place.join(THIRD_PARTY_NOTICES))
    .find(|notices| notices.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A package laid out in a scratch folder of its own: an empty file at
    /// each of `files`, relative to the folder, which is returned.
    fn package(layout: &str, files: &[&str]) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("ruston-licenses-{layout}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for file in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"").unwrap();
        }
        root
    }

    #[test]
    fn windows_finds_the_notices_beside_the_executable() {
        let root = package(
            "windows",
            &[
                "Ruston Mail/ruston.exe",
                "Ruston Mail/THIRD_PARTY_LICENSES.txt",
            ],
        );
        let found = notices_near(&root.join("Ruston Mail/ruston.exe"));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            found,
            Some(root.join("Ruston Mail/THIRD_PARTY_LICENSES.txt"))
        );
    }

    #[test]
    fn the_macos_app_finds_the_notices_in_its_resources() {
        let root = package(
            "macos",
            &[
                "Ruston Mail.app/Contents/MacOS/ruston",
                "Ruston Mail.app/Contents/Resources/THIRD_PARTY_LICENSES.txt",
            ],
        );
        let found = notices_near(&root.join("Ruston Mail.app/Contents/MacOS/ruston"));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            found,
            Some(root.join("Ruston Mail.app/Contents/Resources/THIRD_PARTY_LICENSES.txt"))
        );
    }

    #[test]
    fn the_appimage_finds_the_notices_in_its_documentation() {
        let root = package(
            "appimage",
            &[
                "AppDir/usr/bin/ruston",
                "AppDir/usr/share/doc/ruston-mail/THIRD_PARTY_LICENSES.txt",
            ],
        );
        let found = notices_near(&root.join("AppDir/usr/bin/ruston"));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            found,
            Some(root.join("AppDir/usr/share/doc/ruston-mail/THIRD_PARTY_LICENSES.txt"))
        );
    }

    #[test]
    fn a_build_run_from_cargo_finds_no_notices() {
        // The repository's own copy is not one a package put there.
        let root = package(
            "cargo",
            &["THIRD_PARTY_LICENSES.txt", "target/debug/ruston"],
        );
        let found = notices_near(&root.join("target/debug/ruston"));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(found, None);
    }
}
