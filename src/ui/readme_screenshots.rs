//! The README's screenshots, taken by the app itself: the demo's inbox with the
//! booking newsletter open, dark and then light, as macOS draws the window, with
//! its rounded corners, colored buttons and shadow on a transparent background.
//! Only the `readme-screenshots` feature builds it, and
//! `packaging/readme-screenshots.sh` runs it.

use std::io;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use eframe::egui;

use crate::app::{App, Message};
use crate::settings::Appearance;

/// The folder the screenshots are saved in; the script sets it.
const FOLDER_VARIABLE: &str = "RUSTON_README_SCREENSHOTS";
/// The size of the window's content in the README, in points.
const WINDOW_SIZE: egui::Vec2 = egui::vec2(1280.0, 800.0);
/// The conversation the README shows open.
const SUBJECT: &str = "Your reading room booking";
/// Seconds a change gets to show before its screenshot.
const SETTLE: f64 = 1.5;
/// Seconds to wait for the demo's inbox before giving up.
const PATIENCE: f64 = 20.0;
/// How many times the window may be found without focus, or before the demo's
/// folder and labels arrive, before giving up.
const RETRIES: u32 = 5;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
/// The PNG chunks showing the picture needs: the critical ones, transparency
/// and color. The others, such as EXIF, XMP and Apple's `iDOT`, are metadata.
const KEPT_CHUNKS: [&[u8]; 10] = [
    b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tRNS", b"iCCP", b"cICP", b"sRGB", b"gAMA", b"cHRM",
];

/// How far the screenshots have come.
pub(super) struct Session {
    folder: PathBuf,
    step: Step,
    /// When the step began, on egui's clock.
    began: Option<f64>,
    retries: u32,
    /// The screenshot being saved, off the interface thread.
    saving: Option<JoinHandle<io::Result<()>>>,
}

#[derive(Clone, Copy)]
enum Step {
    /// Waits for the demo's inbox to list the conversation.
    Open,
    /// Lets the window settle in this appearance, then saves it.
    Shoot(Appearance),
    Finished,
}

impl Session {
    /// A session when the demo starts with [`FOLDER_VARIABLE`] set.
    pub(super) fn from_env(demo: bool) -> Option<Self> {
        let folder = std::env::var_os(FOLDER_VARIABLE)?;
        demo.then(|| Self {
            folder: folder.into(),
            step: Step::Open,
            began: None,
            retries: 0,
            saving: None,
        })
    }

    /// Takes the screenshots one frame further: the messages the app takes.
    pub(super) fn frame(
        &mut self,
        context: &egui::Context,
        frame: &eframe::Frame,
        app: &App,
    ) -> Vec<Message> {
        // The steps wait on egui's clock, which moves only while frames are drawn.
        context.request_repaint();
        let now = context.input(|input| input.time);
        self.began.get_or_insert(now);
        match self.step {
            Step::Open => self.open(context, app, now),
            Step::Shoot(appearance) => self.shoot(context, frame, app, appearance, now),
            Step::Finished => Vec::new(),
        }
    }

    fn open(&mut self, context: &egui::Context, app: &App, now: f64) -> Vec<Message> {
        let Some(messages) = opening(app) else {
            if self.waited(now) > PATIENCE {
                return self.fail(context, "the demo's inbox did not list the booking");
            }
            return Vec::new();
        };
        context.send_viewport_cmd(egui::ViewportCommand::InnerSize(WINDOW_SIZE));
        // eframe skips drawing a covered window, which would stall the session.
        context.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::AlwaysOnTop,
        ));
        // macOS colors the window's buttons only while the window has focus.
        context.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.next(Step::Shoot(Appearance::Dark), now);
        messages
    }

    fn shoot(
        &mut self,
        context: &egui::Context,
        frame: &eframe::Frame,
        app: &App,
        appearance: Appearance,
        now: f64,
    ) -> Vec<Message> {
        if let Some(saving) = self.saving.take_if(|saving| saving.is_finished()) {
            return match saving.join() {
                Ok(Ok(())) => self.saved(context, appearance, now),
                Ok(Err(error)) => {
                    let name = file_name(appearance);
                    self.fail(context, &format!("saving {name}: {error}"))
                }
                Err(_) => self.fail(context, "saving a screenshot panicked"),
            };
        }
        if self.saving.is_some() || self.waited(now) < SETTLE {
            return Vec::new();
        }
        // A window without focus shows gray buttons, and the sidebar lists no
        // folder or labels until the demo's arrive: either waits again.
        let focused = context.input(|input| input.viewport().focused) == Some(true);
        if !focused || app.folders().is_empty() {
            if self.retries == RETRIES {
                return self.fail(
                    context,
                    "the window lost focus or shows no folders; leave it in front until it closes",
                );
            }
            self.retries += 1;
            self.began = Some(now);
            if !focused {
                context.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            return Vec::new();
        }
        match window_number(frame) {
            Ok(window) => {
                let path = self.folder.join(file_name(appearance));
                self.saving = Some(std::thread::spawn(move || save_window(window, &path)));
                Vec::new()
            }
            Err(error) => self.fail(context, &format!("finding the window: {error}")),
        }
    }

    /// After a screenshot is saved: the light one next, or the end.
    fn saved(&mut self, context: &egui::Context, appearance: Appearance, now: f64) -> Vec<Message> {
        match appearance {
            Appearance::Dark => {
                self.next(Step::Shoot(Appearance::Light), now);
                vec![Message::SetAppearance(Appearance::Light)]
            }
            Appearance::Light => {
                self.step = Step::Finished;
                context.send_viewport_cmd(egui::ViewportCommand::Close);
                Vec::new()
            }
        }
    }

    /// Closes the window without the screenshots; the script reports them
    /// missing.
    fn fail(&mut self, context: &egui::Context, reason: &str) -> Vec<Message> {
        eprintln!("README screenshots: {reason}");
        self.step = Step::Finished;
        context.send_viewport_cmd(egui::ViewportCommand::Close);
        Vec::new()
    }

    fn next(&mut self, step: Step, now: f64) {
        self.step = step;
        self.began = Some(now);
        self.retries = 0;
    }

    fn waited(&self, now: f64) -> f64 {
        self.began.map_or(0.0, |began| now - began)
    }
}

/// Keeps the mouse and keys out of the screenshots: a pointer over the window
/// would light up what it rests on.
pub(super) fn ignore_input(raw_input: &mut egui::RawInput) {
    raw_input
        .events
        .retain(|event| matches!(event, egui::Event::WindowFocused(_)));
    raw_input.events.push(egui::Event::PointerGone);
}

/// What sets up the README's view once the demo's inbox lists the booking:
/// the booking open, and the dark appearance.
fn opening(app: &App) -> Option<Vec<Message>> {
    let booking = app
        .mailbox()?
        .visible_conversations()
        .find(|row| row.subject.as_deref() == Some(SUBJECT))?
        .id
        .clone();
    Some(vec![
        Message::SelectConversation(booking),
        Message::SetAppearance(Appearance::Dark),
    ])
}

fn file_name(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Dark => "main-window-dark.png",
        Appearance::Light => "main-window-light.png",
    }
}

/// The number macOS's window server knows the window by.
#[cfg(target_os = "macos")]
fn window_number(frame: &eframe::Frame) -> io::Result<isize> {
    use objc2_app_kit::NSView;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = frame.window_handle().map_err(io::Error::other)?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err(io::Error::other("the window is not an AppKit window"));
    };
    // SAFETY: winit hands over the window's live content view, and the
    // interface runs on the main thread, where AppKit may be used.
    let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    let window = view
        .window()
        .ok_or_else(|| io::Error::other("the view has no window"))?;
    Ok(window.windowNumber())
}

#[cfg(not(target_os = "macos"))]
fn window_number(_frame: &eframe::Frame) -> io::Result<isize> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "the README shows the app on macOS",
    ))
}

/// Saves the window numbered `window` at `path` as macOS draws it, keeping
/// only the picture and its color among the PNG's chunks.
fn save_window(window: isize, path: &Path) -> io::Result<()> {
    let status = std::process::Command::new("/usr/sbin/screencapture")
        // No shutter sound; that one window, with its shadow.
        .arg("-x")
        .arg(format!("-l{window}"))
        .arg(path)
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!(
            "screencapture ended with {status}"
        )));
    }
    let png = std::fs::read(path)?;
    let png = without_metadata(&png)
        .ok_or_else(|| io::Error::other("screencapture saved no whole PNG"))?;
    std::fs::write(path, png)
}

/// `png` with only its [`KEPT_CHUNKS`], each copied whole, so the pixels stay
/// as they were. `None` when `png` is not a whole PNG.
fn without_metadata(png: &[u8]) -> Option<Vec<u8>> {
    let mut rest = png.strip_prefix(PNG_SIGNATURE)?;
    let mut kept = PNG_SIGNATURE.to_vec();
    loop {
        let length = u32::from_be_bytes(rest.get(..4)?.try_into().ok()?);
        // The length, the kind, the data and the checksum.
        let chunk = rest.get(..usize::try_from(length).ok()?.checked_add(12)?)?;
        let kind = &chunk[4..8];
        if KEPT_CHUNKS.contains(&kind) {
            kept.extend_from_slice(chunk);
        }
        rest = &rest[chunk.len()..];
        if kind == b"IEND" {
            return Some(kept);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::Folder;
    use crate::settings::Settings;

    /// A chunk of `kind` holding `data`, with a checksum that is copied as it is.
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let length = u32::try_from(data.len()).unwrap();
        [&length.to_be_bytes()[..], kind, data, &[0; 4]].concat()
    }

    #[test]
    fn a_screenshot_keeps_its_picture_and_colors_without_its_metadata() {
        let picture = [
            chunk(b"IHDR", &[1; 13]),
            chunk(b"iCCP", b"Display P3"),
            chunk(b"cICP", &[12, 13, 0, 1]),
        ]
        .concat();
        let metadata = [
            chunk(b"eXIf", b"Screenshot"),
            chunk(b"pHYs", &[0; 9]),
            chunk(b"iTXt", b"XML:com.adobe.xmp"),
            chunk(b"iDOT", &[0; 28]),
        ]
        .concat();
        let pixels = [
            chunk(b"IDAT", b"top"),
            chunk(b"IDAT", b"bottom"),
            chunk(b"IEND", b""),
        ]
        .concat();
        let png = [PNG_SIGNATURE, &picture, &metadata, &pixels].concat();

        assert_eq!(
            without_metadata(&png),
            Some([PNG_SIGNATURE, &picture, &pixels].concat())
        );
    }

    #[test]
    fn only_a_whole_png_is_kept() {
        let png = [
            PNG_SIGNATURE,
            &chunk(b"IHDR", &[1; 13]),
            &chunk(b"IEND", b""),
        ]
        .concat();

        assert_eq!(without_metadata(&png).as_deref(), Some(&png[..]));
        assert_eq!(without_metadata(&png[1..]), None, "no signature");
        assert_eq!(
            without_metadata(&png[..png.len() - 1]),
            None,
            "a chunk cut short"
        );
        assert_eq!(without_metadata(&png[..png.len() - 12]), None, "no end");
    }

    #[test]
    fn the_readme_view_opens_the_demos_booking_in_the_dark_appearance() {
        let mut settings = Settings::default();
        settings.appearance = Appearance::Light;
        let (mut app, _) = App::boot(true, settings);
        let page = crate::mail::demo::DemoMailbox::new().list_conversations(
            &Folder::INBOX,
            0,
            crate::mail::demo::PAGE_SIZE,
            crate::mail::demo::now(),
        );
        let _ = app.update(Message::ConversationsLoaded(1, page));

        for message in opening(&app).expect("the demo's inbox lists the booking") {
            let _ = app.update(message);
        }

        let open = app.mailbox().unwrap().selected_summary();
        assert_eq!(open.and_then(|row| row.subject.as_deref()), Some(SUBJECT));
        assert_eq!(app.settings().appearance, Appearance::Dark);
    }
}
