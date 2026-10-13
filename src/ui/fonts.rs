//! Bounded, local-only font discovery. Keep the bundled look unless opted in.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use skrifa::{FontRef, MetadataProvider, raw::TableProvider};

const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_FALLBACK_BYTES: usize = 48 * 1024 * 1024;
const APPLIED: &str = "ruston-font-policy";

pub(super) fn apply(context: &egui::Context, system: bool) {
    if context.data(|data| data.get_temp::<bool>(egui::Id::new(APPLIED))) == Some(system) {
        return;
    }
    context.set_fonts(definitions(system));
    context.data_mut(|data| data.insert_temp(egui::Id::new(APPLIED), system));
}

fn definitions(system: bool) -> FontDefinitions {
    static PRIMARY: OnceLock<Option<Arc<FontData>>> = OnceLock::new();
    static FALLBACKS: OnceLock<Vec<Arc<FontData>>> = OnceLock::new();
    let mut fonts = FontDefinitions::default();
    if system
        && let Some(data) = PRIMARY.get_or_init(|| {
            primary_candidates()
                .iter()
                .find_map(|path| load(path))
                .map(retain)
        })
    {
        fonts
            .font_data
            .insert("ruston-system-ui".into(), data.clone());
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "ruston-system-ui".into());
    }
    for (index, data) in FALLBACKS.get_or_init(load_fallbacks).iter().enumerate() {
        let name = format!("ruston-language-{index}");
        fonts.font_data.insert(name.clone(), data.clone());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.get_mut(&family).unwrap().push(name.clone());
        }
    }
    fonts
}

fn load(path: &Path) -> Option<FontData> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return None;
    }
    let count = skrifa::raw::CollectionRef::new(&bytes)
        .map(|collection| collection.len())
        .unwrap_or(1)
        .min(32);
    // Collections often start with a bold/light face. Choose a normal face
    // nearest regular weight, rather than assuming collection index zero.
    let index = (0..count)
        .filter_map(|index| {
            let font = FontRef::from_index(&bytes, index).ok()?;
            let attributes = font.attributes();
            if attributes.style != skrifa::attribute::Style::Normal
                || font.head().is_err()
                || font.hhea().is_err()
                || font.hmtx().is_err()
                || font.charmap().map(' ').is_none()
            {
                return None;
            }
            Some((index, (attributes.weight.value() - 400.0).abs()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))?
        .0;
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    Some(data)
}

fn retain(mut data: FontData) -> Arc<FontData> {
    // This process-lifetime cache is bounded before insertion. Borrowing its
    // bytes avoids egui 0.36 cloning an entire owned font per context/scale.
    data.font = std::borrow::Cow::Borrowed(Box::leak(data.font.into_owned().into_boxed_slice()));
    Arc::new(data)
}

fn load_fallbacks() -> Vec<Arc<FontData>> {
    let mut fonts = Vec::new();
    let mut bytes = 0;
    for group in fallback_candidates() {
        if let Some(font) = group.iter().find_map(|path| load(path))
            && bytes + font.font.len() <= MAX_FALLBACK_BYTES
        {
            bytes += font.font.len();
            fonts.push(retain(font));
        }
    }
    fonts
}

#[cfg(target_os = "macos")]
fn primary_candidates() -> Vec<PathBuf> {
    [
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/HelveticaNeue.ttc",
    ]
    .map(PathBuf::from)
    .to_vec()
}

#[cfg(target_os = "macos")]
fn fallback_candidates() -> Vec<Vec<PathBuf>> {
    // Arial Unicode covers these scripts in a single installed file. If it is
    // unavailable, use a bounded CJK collection instead. No Apple fonts are shipped.
    vec![
        [
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
        ]
        .map(PathBuf::from)
        .to_vec(),
    ]
}

#[cfg(target_os = "windows")]
fn windows_fonts(names: &[&str]) -> Vec<PathBuf> {
    std::env::var_os("WINDIR")
        .map(|root| {
            let directory = PathBuf::from(root).join("Fonts");
            names.iter().map(|name| directory.join(name)).collect()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn primary_candidates() -> Vec<PathBuf> {
    windows_fonts(&["segoeui.ttf", "arial.ttf"])
}

#[cfg(target_os = "windows")]
fn fallback_candidates() -> Vec<Vec<PathBuf>> {
    vec![
        windows_fonts(&["msyh.ttc", "meiryo.ttc", "msgothic.ttc"]),
        windows_fonts(&["malgun.ttf"]),
        windows_fonts(&["arial.ttf", "tahoma.ttf"]),
    ]
}

#[cfg(target_os = "linux")]
fn primary_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    // Fontconfig follows the user's configured sans-serif family. No shell is
    // involved; a missing or stalled helper cannot prevent startup.
    if let Some(path) = fontconfig_sans() {
        paths.push(path);
    }
    paths.extend(
        [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
        ]
        .map(PathBuf::from),
    );
    paths
}

#[cfg(target_os = "linux")]
fn fontconfig_sans() -> Option<PathBuf> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut child = Command::new("fc-match")
        .args(["-f", "%{file}", "sans-serif"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let mut path = String::new();
                child
                    .stdout
                    .take()?
                    .take(4096)
                    .read_to_string(&mut path)
                    .ok()?;
                return Some(PathBuf::from(path));
            }
            Ok(Some(_)) => return None,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn fallback_candidates() -> Vec<Vec<PathBuf>> {
    vec![
        [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        ]
        .map(PathBuf::from)
        .to_vec(),
        ["/usr/share/fonts/truetype/noto/NotoSansDevanagari-Regular.ttf"]
            .map(PathBuf::from)
            .to_vec(),
        ["/usr/share/fonts/truetype/noto/NotoSansThai-Regular.ttf"]
            .map(PathBuf::from)
            .to_vec(),
        ["/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"]
            .map(PathBuf::from)
            .to_vec(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_modes_keep_bundled_faces_and_the_monospace_primary() {
        let defaults = FontDefinitions::default();
        for system in [false, true] {
            let fonts = definitions(system);
            for name in defaults.font_data.keys() {
                assert!(fonts.font_data.contains_key(name));
            }
            assert!(
                fonts.families[&FontFamily::Monospace]
                    .starts_with(&defaults.families[&FontFamily::Monospace])
            );
            if !system {
                assert!(
                    fonts.families[&FontFamily::Proportional]
                        .starts_with(&defaults.families[&FontFamily::Proportional])
                );
            }
        }
    }

    #[test]
    fn installed_fallback_glyphs_are_available_in_both_families() {
        let definitions = definitions(false);
        let context = egui::Context::default();
        context.set_fonts(definitions.clone());
        context
            .run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        for glyph in ['王', 'あ', '김', 'क', 'ส', 'Ж', 'א', 'م'] {
            let available = definitions.font_data.values().any(|data| {
                FontRef::from_index(data.font.as_ref(), data.index)
                    .is_ok_and(|font| font.charmap().map(glyph).is_some())
            });
            if available {
                for family in [FontFamily::Proportional, FontFamily::Monospace] {
                    // has_glyph can return false when a real glyph shares the
                    // replacement's face; compare actual atlas allocations.
                    let rendered = |character: char| {
                        context.fonts_mut(|fonts| {
                            let galley = fonts.layout_no_wrap(
                                character.to_string(),
                                egui::FontId::new(14.0, family.clone()),
                                egui::Color32::WHITE,
                            );
                            galley.rows[0].row.glyphs[0].uv_rect
                        })
                    };
                    assert_ne!(
                        rendered(glyph),
                        rendered('◻'),
                        "missing installed glyph {glyph}"
                    );
                }
            }
        }
    }

    #[test]
    fn repeated_apply_does_not_reset_the_font_layout_cache() {
        let context = egui::Context::default();
        apply(&context, false);
        context
            .run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let layout = || {
            context.fonts_mut(|fonts| {
                fonts.layout_no_wrap(
                    "Artículo faltante en pedido".into(),
                    egui::FontId::proportional(14.0),
                    egui::Color32::WHITE,
                )
            })
        };
        let first = layout();
        apply(&context, false);
        context
            .run_ui(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        assert!(Arc::ptr_eq(&first, &layout()));
    }

    #[test]
    fn missing_invalid_and_oversized_files_are_skipped() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ruston-font-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        assert!(load(&path).is_none());
        std::fs::write(&path, b"not a font").unwrap();
        assert!(load(&path).is_none());
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        let rejected = load(&path).is_none();
        std::fs::remove_file(path).unwrap();
        assert!(rejected);
    }
}
