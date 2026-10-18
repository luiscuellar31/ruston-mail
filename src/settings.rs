//! Persistent preferences with safe defaults and best-effort writes.

use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mail::{BodyFormat, Domains, Folder, MailFolder};

const FILE: &str = "settings.json";
/// The fictional mailbox keeps preferences of its own, so nothing changed
/// while trying it, such as a link prompt turned off, reaches a real account.
const DEMO_FILE: &str = "settings-demo.json";
const TEMP_ATTEMPTS: u32 = 100;

/// Ratios are clamped on load: a damaged file must not produce a layout with
/// no room to read in.
const MIN_RATIO: f32 = 0.1;
const MAX_RATIO: f32 = 0.9;
const MIN_WINDOW: (f32, f32) = (480.0, 420.0);
/// Zoom is a scale on every length in the interface, so it has to stay within
/// what a window can still show something useful at.
pub const MIN_ZOOM: f32 = 0.8;
pub const MAX_ZOOM: f32 = 2.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub window: Window,
    pub panels: Panels,
    /// The last folder that was read. Kept whatever [`Self::start`] says, so
    /// going back to [`StartFolder::LastRead`] still knows where that was.
    pub folder: Folder,
    /// Which folder a run opens in.
    pub start: StartFolder,
    /// Mark a conversation as read as soon as it is opened.
    pub mark_read_on_open: bool,
    /// Ask where a link goes before opening it.
    pub confirm_links: bool,
    /// Open every message in a conversation, not only the newest.
    pub expand_all_messages: bool,
    /// Unfold quoted passages instead of hiding them behind a button.
    pub show_quoted_text: bool,
    /// Whether a message is written out as HTML or as plain text.
    pub compose_format: BodyFormat,
    /// Where new messages, replies and forwards open.
    pub compose_placement: ComposePlacement,
    /// Ask Proton for recipients' keys while writing, to show whether a new
    /// message will be end-to-end encrypted. Off, nothing is asked before
    /// sending. Settings saved before this choice existed keep it on.
    #[serde(default = "enabled")]
    pub check_recipient_protection: bool,
    /// Take recipient domains written in another script, sent in their ASCII
    /// `xn--` form. Off, only ASCII domains are taken, as before this choice.
    #[serde(default)]
    pub accept_international_domains: bool,
    /// Color scheme of the desktop interface.
    #[serde(default)]
    pub appearance: Appearance,
    /// Follow the system's light or dark mode instead. `appearance` stays the
    /// last scheme chosen, used where the system tells none. A field of its
    /// own, not a third `Appearance`: an older version would not read that
    /// and would put every setting back to its default.
    #[serde(default)]
    pub follow_system_appearance: bool,
    /// Opt-in native UI face; bundled fonts remain the default and fallback.
    #[serde(default)]
    pub system_font: bool,
    /// How much to scale the interface by. Every length is a multiple of it,
    /// so the whole window grows together rather than the text alone.
    pub zoom: f32,
    /// Show the unread inbox count on the macOS Dock icon.
    pub show_unread_badge: bool,
    /// Show desktop notification when new mail arrives in inbox.
    pub desktop_notifications: bool,
    /// Whether `load` read these settings, allowing them to be written back.
    #[serde(skip)]
    stored: bool,
    /// Whether these are the demo's settings, kept in a file of their own.
    #[serde(skip)]
    demo: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window: Window::default(),
            panels: Panels::default(),
            folder: Folder::INBOX,
            start: StartFolder::LastRead,
            // Keep the initial reader choices predictable on a fresh install.
            mark_read_on_open: true,
            confirm_links: true,
            expand_all_messages: false,
            show_quoted_text: false,
            compose_format: BodyFormat::PlainText,
            compose_placement: ComposePlacement::ReadingPane,
            check_recipient_protection: true,
            accept_international_domains: false,
            appearance: Appearance::Dark,
            follow_system_appearance: false,
            system_font: false,
            zoom: 1.0,
            show_unread_badge: true,
            desktop_notifications: true,
            stored: false,
            demo: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub width: f32,
    pub height: f32,
    /// Reopen maximized. The size above is the one to come back to.
    #[serde(default)]
    pub maximized: bool,
    /// Where the window's top-left corner was, in the desktop's own
    /// coordinates; `None` leaves the place to the system.
    #[serde(default)]
    pub position: Option<[f32; 2]>,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            width: 1_100.0,
            height: 700.0,
            maximized: false,
            position: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Panels {
    /// Where the sidebar ends, as a fraction of the window.
    pub sidebar: f32,
    /// Where the conversation list ends, as a fraction of the rest.
    pub conversations: f32,
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            sidebar: 0.2,
            conversations: 3.0 / 7.0,
        }
    }
}

/// Which folder opens. Only stable system folders can be pinned.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartFolder {
    /// Wherever the mailbox was left.
    #[default]
    LastRead,
    /// The same folder every time, whatever was read last.
    Always(MailFolder),
}

/// Where the composer appears while the mailbox stays open.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComposePlacement {
    /// Replaces the reader in the right-hand pane.
    #[default]
    ReadingPane,
    /// Opens in a separate native window.
    Window,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Appearance {
    #[default]
    Dark,
    Light,
}

/// The subset of settings that controls initial reader expansion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reading {
    pub expand_all_messages: bool,
    pub show_quoted_text: bool,
}

impl Settings {
    /// The folder to open, which is the one pinned here or the last one read.
    pub fn start_folder(&self) -> Folder {
        match self.start {
            StartFolder::LastRead => self.folder.clone(),
            StartFolder::Always(folder) => Folder::System(folder),
        }
    }

    pub fn reading(&self) -> Reading {
        Reading {
            expand_all_messages: self.expand_all_messages,
            show_quoted_text: self.show_quoted_text,
        }
    }

    /// Which recipient domains the composer takes.
    pub fn recipient_domains(&self) -> Domains {
        if self.accept_international_domains {
            Domains::International
        } else {
            Domains::Ascii
        }
    }

    /// Reads the settings, using defaults if the file is absent or invalid.
    /// The demo reads, and later writes, a file of its own.
    pub fn load(demo: bool) -> Self {
        let mut settings = path(demo).map_or_else(Self::default, |path| read(&path));
        settings.demo = demo;

        settings
    }

    /// Writes the settings. Failures are ignored: this is a convenience, and
    /// a read-only disk is not a reason to interrupt someone's mail.
    pub fn save(&self) {
        if !self.stored {
            return;
        }
        let Some(path) = path(self.demo) else {
            return;
        };
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        let _ = write_atomically(&path, text.as_bytes());
    }

    /// In-memory settings that open a given folder without becoming writable.
    #[cfg(test)]
    pub fn opening(folder: Folder) -> Self {
        Self {
            folder,
            ..Self::default()
        }
    }

    /// Pins a system folder while keeping `stored` private.
    #[cfg(test)]
    pub fn with_start(mut self, start: StartFolder) -> Self {
        self.start = start;
        self
    }

    /// Values a damaged or hand-edited file could otherwise break the layout
    /// with: ratios outside the panel range, and impossible window sizes.
    fn sanitized(mut self) -> Self {
        let defaults = Self::default();

        self.zoom = if self.zoom.is_finite() {
            self.zoom.clamp(MIN_ZOOM, MAX_ZOOM)
        } else {
            defaults.zoom
        };
        self.panels.sidebar = clamp_ratio(self.panels.sidebar, defaults.panels.sidebar);
        self.panels.conversations =
            clamp_ratio(self.panels.conversations, defaults.panels.conversations);
        self.window.width = clamp_size(self.window.width, MIN_WINDOW.0, defaults.window.width);
        self.window.height = clamp_size(self.window.height, MIN_WINDOW.1, defaults.window.height);
        self.window.position = self
            .window
            .position
            .filter(|position| position.iter().all(|coordinate| coordinate.is_finite()));

        self
    }
}

fn enabled() -> bool {
    true
}

fn clamp_ratio(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_RATIO, MAX_RATIO)
    } else {
        fallback
    }
}

fn clamp_size(value: f32, minimum: f32, fallback: f32) -> f32 {
    if value.is_finite() && value >= minimum {
        value
    } else {
        fallback
    }
}

fn path(demo: bool) -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "Ruston Mail")?;

    Some(dirs.config_dir().join(if demo { DEMO_FILE } else { FILE }))
}

/// Reads the settings kept at `path`, to be written back there. A file that
/// is there but cannot be used is first moved aside, to [`unreadable`], so
/// the defaults written instead destroy nothing; one that cannot be moved is
/// never written over.
fn read(path: &Path) -> Settings {
    let found = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Settings>(&bytes).ok(),
        // Even defaults are worth storing once the app is running: it is
        // the first run that has nothing on disk yet.
        Err(error) if error.kind() == ErrorKind::NotFound => Some(Settings::default()),
        Err(_) => None,
    };
    let usable = found.is_some();
    let mut settings = found.unwrap_or_default().sanitized();
    settings.stored = usable || fs::rename(path, unreadable(path)).is_ok();

    settings
}

/// Where a settings file that could not be used is kept: beside it, with
/// `.unreadable` after its name. A later one replaces it.
fn unreadable(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or(FILE.as_ref()).to_os_string();
    name.push(".unreadable");
    path.with_file_name(name)
}

/// Replaces complete settings in one rename, leaving the previous file intact
/// until the new contents have reached disk.
fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "settings path has no parent"))?;
    fs::create_dir_all(parent)?;

    for attempt in 0..TEMP_ATTEMPTS {
        let name = path.file_name().unwrap_or(FILE.as_ref()).to_string_lossy();
        let temporary = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), attempt));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };

        let written = file.write_all(contents).and_then(|()| file.sync_all());
        drop(file);
        if let Err(error) = written {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        return Ok(());
    }

    Err(io::Error::new(
        ErrorKind::AlreadyExists,
        "no temporary settings name is available",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip() {
        let settings = Settings {
            folder: Folder::System(crate::mail::MailFolder::Archive),
            mark_read_on_open: false,
            check_recipient_protection: false,
            appearance: Appearance::Light,
            system_font: true,
            ..Settings::default()
        };

        let text = serde_json::to_string(&settings).unwrap();

        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn older_settings_keep_the_dark_appearance() {
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        old.as_object_mut().unwrap().remove("appearance");
        old.as_object_mut()
            .unwrap()
            .remove("follow_system_appearance");
        old.as_object_mut().unwrap().remove("system_font");
        let restored: Settings = serde_json::from_value(old).unwrap();
        assert_eq!(restored.appearance, Appearance::Dark);
        assert!(!restored.follow_system_appearance);
        assert!(!restored.system_font);
    }

    #[test]
    fn following_the_system_leaves_older_versions_a_scheme_they_read() {
        let following = Settings {
            appearance: Appearance::Light,
            follow_system_appearance: true,
            ..Settings::default()
        };
        let mut written = serde_json::to_value(&following).unwrap();
        // A version before this choice finds a scheme it knows...
        assert_eq!(written["appearance"], "Light");
        // ...and skips the field it does not, as this one skips a later one's.
        written
            .as_object_mut()
            .unwrap()
            .insert("from_a_later_version".into(), true.into());
        let read: Settings = serde_json::from_value(written).unwrap();
        assert_eq!(read, following);
    }

    #[test]
    fn older_settings_keep_checking_recipient_protection() {
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        old.as_object_mut()
            .unwrap()
            .remove("check_recipient_protection");
        let restored: Settings = serde_json::from_value(old).unwrap();
        assert!(restored.check_recipient_protection);
    }

    #[test]
    fn only_ascii_domains_are_taken_until_asked_otherwise() {
        assert!(!Settings::default().accept_international_domains);
        assert_eq!(Settings::default().recipient_domains(), Domains::Ascii);

        // Settings saved before this choice existed keep refusing them.
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        old.as_object_mut()
            .unwrap()
            .remove("accept_international_domains");
        let restored: Settings = serde_json::from_value(old).unwrap();
        assert!(!restored.accept_international_domains);

        let chosen = Settings {
            accept_international_domains: true,
            ..Settings::default()
        };
        let text = serde_json::to_string(&chosen).unwrap();
        let restored: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(restored.recipient_domains(), Domains::International);
    }

    #[test]
    fn mail_is_written_as_plain_text_until_asked_otherwise() {
        assert_eq!(Settings::default().compose_format, BodyFormat::PlainText);
        // It survives the file like every other choice.
        let chosen = Settings {
            compose_format: BodyFormat::Html,
            ..Settings::default()
        };
        let text = serde_json::to_string(&chosen).unwrap();
        assert_eq!(
            serde_json::from_str::<Settings>(&text)
                .unwrap()
                .compose_format,
            BodyFormat::Html
        );
    }

    #[test]
    fn writing_opens_in_the_reading_pane_until_a_window_is_chosen() {
        assert_eq!(
            Settings::default().compose_placement,
            ComposePlacement::ReadingPane
        );

        let chosen = Settings {
            compose_placement: ComposePlacement::Window,
            ..Settings::default()
        };
        let text = serde_json::to_string(&chosen).unwrap();

        assert_eq!(
            serde_json::from_str::<Settings>(&text)
                .unwrap()
                .compose_placement,
            ComposePlacement::Window
        );
    }

    #[test]
    fn reader_defaults_are_consistent() {
        let defaults = Settings::default();

        assert!(!defaults.expand_all_messages);
        assert!(!defaults.show_quoted_text);
        assert!(defaults.show_unread_badge);
        assert_eq!(defaults.reading(), Reading::default());
    }

    #[test]
    fn incomplete_settings_are_invalid() {
        assert!(serde_json::from_str::<Settings>(r#"{"confirm_links": false}"#).is_err());

        let mut incomplete = serde_json::to_value(Settings::default()).unwrap();
        incomplete["window"]
            .as_object_mut()
            .unwrap()
            .remove("width");
        assert!(serde_json::from_value::<Settings>(incomplete).is_err());
    }

    #[test]
    fn the_demo_keeps_settings_of_its_own() {
        let file = |demo| path(demo).unwrap().file_name().unwrap().to_owned();
        assert_eq!(file(false), FILE);
        assert_eq!(file(true), DEMO_FILE);
        assert_eq!(path(true).unwrap().parent(), path(false).unwrap().parent());

        // Loading only reads; what was loaded is written back to its own file.
        let demo = Settings::load(true);
        assert!(demo.stored && demo.demo);
        assert_eq!(file(demo.demo), DEMO_FILE);
        assert!(!Settings::load(false).demo);
        // Neither bookkeeping flag reaches the file.
        let text = serde_json::to_string(&demo).unwrap();
        assert!(!text.contains("demo") && !text.contains("stored"));
    }

    #[test]
    fn only_loaded_settings_are_written_back() {
        // Settings built in memory, as every test does, must never reach the
        // config file someone actually uses.
        assert!(!Settings::default().stored);

        let loaded = Settings {
            stored: true,
            ..Settings::default()
        };
        assert!(loaded.stored);
        // The flag is bookkeeping, not content, so it never reaches the file.
        let text = serde_json::to_string(&loaded).unwrap();
        assert!(!text.contains("stored"));
    }

    #[test]
    fn settings_replace_a_complete_file_and_leave_no_temporary_file() {
        let folder = std::env::temp_dir().join(format!("ruston-settings-{}", std::process::id()));
        let path = folder.join(FILE);
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        fs::write(&path, b"old settings").unwrap();

        write_atomically(&path, b"new settings").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"new settings");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        let _ = fs::remove_dir_all(folder);
    }

    /// What 0.7.1, the latest release, writes once every choice is changed.
    const RELEASED: &str = r#"{
  "window": {
    "width": 1280.0,
    "height": 800.0,
    "maximized": true,
    "position": [
      40.0,
      60.0
    ]
  },
  "panels": {
    "sidebar": 0.25,
    "conversations": 0.5
  },
  "folder": {
    "id": "kZ9",
    "name": "Invoices",
    "kind": "Folder"
  },
  "start": {
    "Always": "Archive"
  },
  "mark_read_on_open": false,
  "confirm_links": false,
  "expand_all_messages": true,
  "show_quoted_text": true,
  "compose_format": "Html",
  "compose_placement": "Window",
  "check_recipient_protection": false,
  "accept_international_domains": true,
  "appearance": "Light",
  "system_font": true,
  "zoom": 1.25,
  "show_unread_badge": false,
  "desktop_notifications": false
}"#;

    fn empty_folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("ruston-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&folder);
        fs::create_dir_all(&folder).unwrap();
        folder
    }

    #[test]
    fn settings_written_by_the_latest_release_still_load() {
        let folder = empty_folder("released");
        let path = folder.join(FILE);
        fs::write(&path, RELEASED).unwrap();

        let expected = Settings {
            window: Window {
                width: 1_280.0,
                height: 800.0,
                maximized: true,
                position: Some([40.0, 60.0]),
            },
            panels: Panels {
                sidebar: 0.25,
                conversations: 0.5,
            },
            folder: Folder::custom("kZ9", "Invoices"),
            start: StartFolder::Always(MailFolder::Archive),
            mark_read_on_open: false,
            confirm_links: false,
            expand_all_messages: true,
            show_quoted_text: true,
            compose_format: BodyFormat::Html,
            compose_placement: ComposePlacement::Window,
            check_recipient_protection: false,
            accept_international_domains: true,
            appearance: Appearance::Light,
            system_font: true,
            zoom: 1.25,
            show_unread_badge: false,
            desktop_notifications: false,
            stored: true,
            // A field added since keeps its default.
            ..Settings::default()
        };
        assert_eq!(read(&path), expected);
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn a_damaged_file_is_kept_aside_before_defaults_replace_it() {
        let folder = empty_folder("damaged");
        let path = folder.join(FILE);
        let writable_defaults = Settings {
            stored: true,
            ..Settings::default()
        };
        // A first run has nothing to keep.
        assert_eq!(read(&path), writable_defaults);
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);

        // A hand edit that lost a comma.
        let damaged = RELEASED.replacen(',', "", 1);
        fs::write(&path, &damaged).unwrap();

        assert_eq!(read(&path), writable_defaults);
        assert!(!path.exists());
        let kept = folder.join("settings.json.unreadable");
        assert_eq!(fs::read_to_string(kept).unwrap(), damaged);
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn a_damaged_file_that_cannot_be_kept_aside_is_not_written_over() {
        let folder = empty_folder("unmovable");
        let path = folder.join(FILE);
        fs::write(&path, b"{").unwrap();
        // Something already holds the name it would be kept under.
        fs::create_dir_all(folder.join("settings.json.unreadable").join("taken")).unwrap();

        let settings = read(&path);

        assert!(!settings.stored);
        assert_eq!(settings, Settings::default());
        assert_eq!(fs::read(&path).unwrap(), b"{");
        let _ = fs::remove_dir_all(folder);
    }

    #[test]
    fn a_pinned_folder_wins_over_the_one_last_read() {
        let last_read = Folder::custom("kZ9", "Invoices");
        let settings = Settings {
            folder: last_read.clone(),
            ..Settings::default()
        };

        // Left alone, a run returns to where it was.
        assert_eq!(settings.start, StartFolder::LastRead);
        assert_eq!(settings.start_folder(), last_read);

        let pinned = Settings {
            start: StartFolder::Always(MailFolder::Archive),
            ..settings
        };
        assert_eq!(pinned.start_folder(), Folder::System(MailFolder::Archive),);
        // The last folder read is still kept, so going back to LastRead knows
        // where that was.
        assert_eq!(pinned.folder, last_read);
    }

    #[test]
    fn zoom_stays_within_what_a_window_can_show() {
        let at = |zoom| {
            Settings {
                zoom,
                ..Settings::default()
            }
            .sanitized()
            .zoom
        };

        assert_eq!(Settings::default().zoom, 1.0);
        assert_eq!(at(5.0), MAX_ZOOM);
        assert_eq!(at(0.1), MIN_ZOOM);
        assert_eq!(at(0.0), MIN_ZOOM);
        assert_eq!(at(f32::NAN), 1.0);
        assert_eq!(at(1.25), 1.25);
    }

    #[test]
    fn a_damaged_file_cannot_break_the_layout() {
        let damaged = Settings {
            panels: Panels {
                sidebar: 0.0,
                conversations: f32::NAN,
            },
            window: Window {
                width: -5.0,
                height: 10.0,
                position: Some([f32::NAN, 40.0]),
                ..Window::default()
            },
            ..Settings::default()
        }
        .sanitized();

        assert_eq!(damaged.panels.sidebar, MIN_RATIO);
        assert_eq!(
            damaged.panels.conversations,
            Settings::default().panels.conversations
        );
        assert_eq!(damaged.window.width, Settings::default().window.width);
        assert_eq!(damaged.window.height, Settings::default().window.height);
        assert_eq!(damaged.window.position, None);
    }

    #[test]
    fn older_settings_reopen_a_normal_window_where_the_system_puts_it() {
        let mut old = serde_json::to_value(Settings::default()).unwrap();
        let window = old["window"].as_object_mut().unwrap();
        window.remove("maximized");
        window.remove("position");
        let restored: Settings = serde_json::from_value(old).unwrap();
        assert!(!restored.window.maximized);
        assert_eq!(restored.window.position, None);
    }

    #[test]
    fn a_narrow_window_is_kept_down_to_the_composers_minimum() {
        let narrow = Settings {
            window: Window {
                width: 520.0,
                height: 430.0,
                ..Window::default()
            },
            ..Settings::default()
        }
        .sanitized();
        assert_eq!((narrow.window.width, narrow.window.height), (520.0, 430.0));
    }
}
