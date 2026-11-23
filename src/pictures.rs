//! Decodes the pictures a message carries within it, which come from whoever
//! sent it: only the formats mail uses, never one too large to hold, and off
//! the interface's thread.

use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits};

/// The largest file a picture is read from. A photo from a phone fits.
pub const MAX_BYTES: u64 = 12 * 1024 * 1024;
/// The most pixels a picture may have: a 40-megapixel photo. A file can
/// claim far more in a few bytes, and decoding it would take the memory.
const MAX_PIXELS: u64 = 40_000_000;
/// The longest side kept. A wider picture is made smaller: the reader shows
/// none wider than its pane.
const MAX_SIDE: u32 = 1600;

/// A decoded picture: its size in pixels and its RGBA bytes, row by row,
/// each color already multiplied by its alpha, as a texture takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

/// The picture `bytes` hold, if they are a PNG, JPEG, GIF or WebP within the
/// limits; a GIF shows its first frame. What the file is comes from its own
/// bytes, not from the type its sender gave it.
pub fn decode(bytes: &[u8]) -> Option<Picture> {
    decode_within(bytes, MAX_PIXELS, MAX_SIDE)
}

fn decode_within(bytes: &[u8], max_pixels: u64, max_side: u32) -> Option<Picture> {
    let reader = || {
        ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()
    };
    let format = reader()?.format()?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP
    ) {
        return None;
    }
    // The size comes from the header alone, before anything is decoded.
    let (width, height) = reader()?.into_dimensions().ok()?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > max_pixels {
        return None;
    }
    let mut reader = reader()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(width);
    limits.max_image_height = Some(height);
    limits.max_alloc = Some(max_pixels.saturating_mul(8));
    reader.limits(limits);
    let picture = reader.decode().ok()?;
    let picture = if width.max(height) > max_side {
        picture.thumbnail(max_side, max_side)
    } else {
        picture
    };
    let mut rgba = picture.into_rgba8();
    // Here rather than on the interface's thread, where a large picture took
    // a few milliseconds of its first frame.
    for pixel in rgba.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        pixel.0 = eframe::egui::Color32::from_rgba_unmultiplied(r, g, b, a).to_array();
    }
    Some(Picture {
        size: [rgba.width() as usize, rgba.height() as usize],
        rgba: rgba.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture of `width` by `height`, written as `format`.
    fn encoded(width: u32, height: u32, format: ImageFormat) -> Vec<u8> {
        let picture = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x * 40) as u8, (y * 40) as u8, 128])
        });
        let mut bytes = Cursor::new(Vec::new());
        picture.write_to(&mut bytes, format).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn the_formats_mail_uses_are_decoded_whatever_their_file_claims_to_be() {
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::Gif,
            ImageFormat::WebP,
        ] {
            let picture = decode(&encoded(4, 3, format)).unwrap_or_else(|| panic!("{format:?}"));
            assert_eq!(picture.size, [4, 3], "{format:?}");
            assert_eq!(picture.rgba.len(), 4 * 3 * 4, "{format:?}");
        }
    }

    #[test]
    fn a_picture_comes_with_its_colors_multiplied_by_their_alpha() {
        let picture = image::RgbaImage::from_fn(2, 1, |x, _| {
            image::Rgba(if x == 0 {
                [200, 100, 50, 128]
            } else {
                [10, 20, 30, 255]
            })
        });
        let mut bytes = Cursor::new(Vec::new());
        picture.write_to(&mut bytes, ImageFormat::Png).unwrap();

        let decoded = decode(&bytes.into_inner()).unwrap();
        let half = eframe::egui::Color32::from_rgba_unmultiplied(200, 100, 50, 128).to_array();
        assert_eq!(decoded.rgba[..4], half);
        assert_eq!(decoded.rgba[4..], [10, 20, 30, 255]);
    }

    #[test]
    fn anything_else_is_no_picture() {
        assert_eq!(decode(b""), None);
        assert_eq!(decode(b"<svg xmlns='http://www.w3.org/2000/svg'/>"), None);
        assert_eq!(decode(b"%PDF-1.7"), None);
        // A format the decoder is not built for, by its first bytes.
        assert_eq!(decode(b"BM\x00\x00\x00\x00"), None);
        // A picture cut short.
        let whole = encoded(8, 8, ImageFormat::Png);
        assert_eq!(decode(&whole[..whole.len() / 2]), None);
    }

    #[test]
    fn a_picture_too_large_is_refused_before_it_is_decoded_and_a_wide_one_made_smaller() {
        let picture = encoded(8, 4, ImageFormat::Png);
        assert_eq!(decode_within(&picture, 31, 1600), None);
        let smaller = decode_within(&picture, 32, 4).unwrap();
        assert_eq!(smaller.size, [4, 2]);
        assert_eq!(decode_within(&picture, 32, 8).unwrap().size, [8, 4]);
    }
}
