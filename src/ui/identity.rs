//! Native window identity shared by the mailbox and composer.

use std::sync::{Arc, OnceLock};

use eframe::egui;

/// Matches the existing macOS bundle identifier and the Linux desktop filename.
pub(super) const APP_ID: &str = "com.luiscuellar.ruston-mail";

pub(super) fn viewport() -> egui::ViewportBuilder {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    let icon = ICON.get_or_init(|| {
        Arc::new(
            eframe::icon_data::from_png_bytes(include_bytes!(
                "../../assets/icons/hicolor/256x256/apps/com.luiscuellar.ruston-mail.png"
            ))
            .expect("invalid bundled desktop icon"),
        )
    });

    egui::ViewportBuilder::default()
        .with_app_id(APP_ID)
        .with_icon(Arc::clone(icon))
}

/// Set the process identity before Windows creates any native windows.
#[cfg(target_os = "windows")]
pub(super) fn set_process_app_id() -> eframe::Result {
    let app_id: Vec<u16> = APP_ID.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: app_id is a live, NUL-terminated UTF-16 string; the API copies it.
    let result = unsafe {
        windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(app_id.as_ptr())
    };
    if result < 0 {
        return Err(eframe::Error::AppCreation(Box::new(std::io::Error::other(
            format!("could not set Windows application identity: HRESULT {result:#x}"),
        ))));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_reports_the_explicit_process_identity() {
        set_process_app_id().unwrap();
        let mut app_id = std::ptr::null_mut();
        // SAFETY: the API writes an allocated UTF-16 string to a valid output pointer.
        let result = unsafe {
            windows_sys::Win32::UI::Shell::GetCurrentProcessExplicitAppUserModelID(&mut app_id)
        };
        assert!(result >= 0, "could not read Windows app ID: {result:#x}");
        assert!(!app_id.is_null());
        // SAFETY: successful calls return a NUL-terminated string owned by the
        // caller. Copy it before releasing it with the documented COM allocator.
        let actual = unsafe {
            let mut len = 0;
            while *app_id.add(len) != 0 {
                len += 1;
            }
            let actual = String::from_utf16_lossy(std::slice::from_raw_parts(app_id, len));
            windows_sys::Win32::System::Com::CoTaskMemFree(app_id.cast());
            actual
        };
        assert_eq!(actual, APP_ID);
    }

    #[test]
    fn linux_launcher_and_macos_bundle_match_the_window_identity() {
        let desktop = include_str!("../../packaging/linux/com.luiscuellar.ruston-mail.desktop");
        assert!(desktop.lines().any(|line| line == format!("Icon={APP_ID}")));
        assert!(desktop.lines().any(|line| line == "StartupWMClass=ruston"));
        let macos = include_str!("../../packaging/macos/package.sh");
        assert!(macos.contains(&format!("IDENTIFIER=\"{APP_ID}\"")));
    }

    #[test]
    fn window_icon_is_valid_and_matches_the_windows_ico_frame() {
        let viewport = viewport();
        let icon = viewport.icon.unwrap();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        assert!(
            icon.rgba
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] > 0)
        );
        assert_eq!(icon.rgba[3], 0, "the icon must retain transparency");

        let ico = include_bytes!("../../assets/windows/ruston-mail.ico");
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        let count = u16::from_le_bytes(ico[4..6].try_into().unwrap()) as usize;
        assert_eq!(count, 7);
        let (frames, _) = ico[6..6 + count * 16].as_chunks::<16>();
        for (frame, dimension) in frames.iter().zip([16, 24, 32, 48, 64, 128, 256]) {
            let encoded_dimension = if dimension == 256 { 0 } else { dimension as u8 };
            assert_eq!(frame[0..2], [encoded_dimension; 2]);
            let size = u32::from_le_bytes(frame[8..12].try_into().unwrap()) as usize;
            let offset = u32::from_le_bytes(frame[12..16].try_into().unwrap()) as usize;
            let windows_icon =
                eframe::icon_data::from_png_bytes(&ico[offset..offset + size]).unwrap();
            assert_eq!(
                (windows_icon.width, windows_icon.height),
                (dimension, dimension)
            );
            if dimension == 256 {
                assert_eq!(windows_icon, *icon);
            }
        }
    }
}
