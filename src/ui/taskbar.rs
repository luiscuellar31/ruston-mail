//! The unread count on the Windows taskbar button, as an overlay icon: what
//! the Dock badge is on macOS.

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::HICON;
use windows::core::HSTRING;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLIP_DEFAULT_PRECIS, CreateBitmap,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject,
    DrawTextW, FW_BOLD, GdiFlush, HBITMAP, HDC, OUT_DEFAULT_PRECIS, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, ICONINFO};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Windows shows the overlay at the small icon size, 16 pixels at 100 %;
/// drawing twice that keeps it sharp at 200 %.
const SIZE: usize = 32;

/// Windows' own red for alerts, as red, green, blue.
const RED: [u8; 3] = [0xC4, 0x2B, 0x1C];

/// Shows `count` on the window's taskbar button, or takes the badge away.
/// Returns false only while the window has no taskbar button to badge yet.
pub(super) fn show_unread(window: &winit::window::Window, count: Option<u32>) -> bool {
    if window.is_visible() == Some(false) {
        return false;
    }
    let Some(hwnd) = hwnd(window) else {
        return true;
    };
    // SAFETY: winit initialized COM for OLE on this UI thread, which owns the
    // window; the interface lives only for these calls.
    unsafe {
        let Ok(taskbar) =
            CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER)
        else {
            return true;
        };
        if taskbar.HrInit().is_err() {
            return true;
        }
        match count {
            None => {
                let _ = taskbar.SetOverlayIcon(hwnd, HICON::default(), &HSTRING::new());
            }
            Some(count) => {
                let Some(icon) = icon(&badge_pixels(&text_mask(&label(count)))) else {
                    return true;
                };
                let description = HSTRING::from(format!("{count} unread"));
                // The taskbar keeps its own copy of the icon.
                let _ = taskbar.SetOverlayIcon(hwnd, HICON(icon), &description);
                DestroyIcon(icon);
            }
        }
    }
    true
}

fn hwnd(window: &winit::window::Window) -> Option<HWND> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// What the badge says: the count, up to 99.
fn label(count: u32) -> String {
    if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    }
}

/// The label drawn in white on black with Segoe UI, as one coverage byte per
/// pixel, row by row. Grayscale smoothing keeps the coverage usable as alpha.
fn text_mask(text: &str) -> Vec<u8> {
    let mut mask = vec![0_u8; SIZE * SIZE];
    // DrawTextW reads the string even when told it is empty.
    if text.is_empty() {
        return mask;
    }
    let height = match text.chars().count() {
        1 => 22,
        2 => 19,
        _ => 14,
    };
    let text: Vec<u16> = text.encode_utf16().collect();
    let face: Vec<u16> = "Segoe UI".encode_utf16().chain(Some(0)).collect();
    // SAFETY: every GDI object created here is selected out and deleted
    // before returning; the pixels are read after GdiFlush, within the
    // bitmap's 32-bit, SIZE by SIZE buffer.
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return mask;
        }
        let mut bits = std::ptr::null_mut();
        let bitmap = bitmap(dc, &mut bits);
        if !bitmap.is_null() && !bits.is_null() {
            std::ptr::write_bytes(bits.cast::<u8>(), 0, SIZE * SIZE * 4);
            let old_bitmap = SelectObject(dc, bitmap);
            let font = CreateFontW(
                -height,
                0,
                0,
                0,
                FW_BOLD as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET.into(),
                OUT_DEFAULT_PRECIS.into(),
                CLIP_DEFAULT_PRECIS.into(),
                ANTIALIASED_QUALITY.into(),
                DEFAULT_PITCH.into(),
                face.as_ptr(),
            );
            let old_font = SelectObject(dc, font);
            SetTextColor(dc, 0x00FF_FFFF);
            SetBkMode(dc, TRANSPARENT as i32);
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: SIZE as i32,
                bottom: SIZE as i32,
            };
            DrawTextW(
                dc,
                text.as_ptr(),
                text.len() as i32,
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            GdiFlush();
            let pixels = std::slice::from_raw_parts(bits.cast::<u8>(), SIZE * SIZE * 4);
            for (coverage, pixel) in mask.iter_mut().zip(pixels.as_chunks::<4>().0) {
                *coverage = pixel[0].max(pixel[1]).max(pixel[2]);
            }
            SelectObject(dc, old_font);
            DeleteObject(font);
            SelectObject(dc, old_bitmap);
        }
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
        DeleteDC(dc);
    }
    mask
}

/// A red disc with smoothed edges and the label in white, as blue, green,
/// red, alpha bytes, row by row, with straight alpha as icons take it.
fn badge_pixels(text: &[u8]) -> Vec<u8> {
    let radius = SIZE as f32 / 2.0;
    let mut pixels = Vec::with_capacity(SIZE * SIZE * 4);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let distance = (x as f32 + 0.5 - radius).hypot(y as f32 + 0.5 - radius);
            let disc = (radius - distance + 0.5).clamp(0.0, 1.0);
            let ink = f32::from(text[y * SIZE + x]) / 255.0;
            let [red, green, blue] = RED.map(|channel| {
                (f32::from(channel) + (255.0 - f32::from(channel)) * ink).round() as u8
            });
            pixels.extend([blue, green, red, (disc * 255.0).round() as u8]);
        }
    }
    pixels
}

/// An icon from `badge_pixels`, or none if Windows cannot make one.
fn icon(pixels: &[u8]) -> Option<windows_sys::Win32::UI::WindowsAndMessaging::HICON> {
    // Every row of a monochrome bitmap is a whole number of 16-bit words;
    // with 32-bit alpha in the colour, an all-clear mask lets alpha decide.
    let mask_bits = [0_u8; SIZE * SIZE / 8];
    // SAFETY: the colour bitmap's buffer is SIZE by SIZE 32-bit pixels, as
    // `pixels` is; both bitmaps are deleted once the icon has copied them.
    unsafe {
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let mut bits = std::ptr::null_mut();
        let colour = bitmap(dc, &mut bits);
        DeleteDC(dc);
        if colour.is_null() || bits.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), SIZE * SIZE * 4);
        let mask = CreateBitmap(SIZE as i32, SIZE as i32, 1, 1, mask_bits.as_ptr().cast());
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: colour,
        };
        let icon = CreateIconIndirect(&info);
        DeleteObject(colour);
        if !mask.is_null() {
            DeleteObject(mask);
        }
        (!icon.is_null()).then_some(icon)
    }
}

/// A top-down, 32-bit SIZE by SIZE bitmap whose pixels `bits` points to.
///
/// # Safety
/// `dc` must be a valid device context.
unsafe fn bitmap(dc: HDC, bits: &mut *mut core::ffi::c_void) -> HBITMAP {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIZE as i32,
            // Negative: the first row is the top one.
            biHeight: -(SIZE as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [Default::default()],
    };
    // SAFETY: the caller passes a valid device context; `info` describes the
    // bitmap and outlives the call.
    unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, bits, std::ptr::null_mut(), 0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badge_counts_up_to_ninety_nine() {
        assert_eq!(label(1), "1");
        assert_eq!(label(99), "99");
        assert_eq!(label(100), "99+");
    }

    #[test]
    fn the_badge_is_a_red_disc_with_white_ink() {
        let mut ink = vec![0_u8; SIZE * SIZE];
        let centre = SIZE / 2 * SIZE + SIZE / 2;
        ink[centre] = 255;
        let pixels = badge_pixels(&ink);
        let pixel = |index: usize| &pixels[index * 4..index * 4 + 4];

        // Corners stay clear; the disc is opaque.
        assert_eq!(pixel(0)[3], 0);
        assert_eq!(pixel(SIZE * SIZE - 1)[3], 0);
        assert_eq!(pixel(centre + 2), [RED[2], RED[1], RED[0], 255]);
        // Ink turns the disc white.
        assert_eq!(pixel(centre), [255, 255, 255, 255]);
    }

    #[test]
    fn windows_draws_the_label_for_the_badge() {
        let blank = text_mask("");
        assert!(blank.iter().all(|&coverage| coverage == 0));
        for text in ["7", "42", "99+"] {
            let mask = text_mask(text);
            let inked = mask.iter().filter(|&&coverage| coverage > 128).count();
            assert!(inked > 10, "{text} left {inked} pixels inked");
            // The label stays inside the disc, away from the corners.
            assert_eq!(mask[0], 0, "{text}");
        }
    }
}
