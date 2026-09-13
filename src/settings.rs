//! What Ruston Mail remembers between runs.
//!
//! Settings are a convenience, never a requirement: a missing, unreadable or
//! damaged file falls back to the defaults, and a failed write is ignored.
//! Losing a window size must never stop someone from reading their mail.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::mail::Folder;

const FILE: &str = "settings.json";

/// Ratios are clamped on load: a damaged file must not produce a layout with
/// no room to read in.
const MIN_RATIO: f32 = 0.1;
const MAX_RATIO: f32 = 0.9;
const MIN_WINDOW: (f32, f32) = (820.0, 480.0);
/// Zoom is a scale on every length in the interface, so it has to stay within
/// what a window can still show something useful at.
pub const MIN_ZOOM: f32 = 0.8;
pub const MAX_ZOOM: f32 = 2.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub window: Window,
    pub panels: Panels,
    /// The folder to open on start, which is the last one that was read.
    pub folder: Folder,
    /// Mark a conversation as read as soon as it is opened.
    pub mark_read_on_open: bool,
    /// Ask where a link goes before opening it.
    pub confirm_links: bool,
    /// Open every message in a conversation, not only the newest.
    pub expand_all_messages: bool,
    /// Unfold quoted passages instead of hiding them behind a button.
    pub show_quoted_text: bool,
    /// How much to scale the interface by. Every length is a multiple of it,
    /// so the whole window grows together rather than the text alone.
    pub zoom: f32,
    /// Whether these settings came from the settings file, and so belong back
    /// in it. Only `load` sets it, which keeps tests and any other in-memory
    /// settings from writing over what someone has on disk.
    #[serde(skip)]
    stored: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window: Window::default(),
            panels: Panels::default(),
            folder: Folder::INBOX,
            // Both default to the behaviour the app had before it could be
            // configured, so an upgrade changes nothing on its own.
            mark_read_on_open: true,
            confirm_links: true,
            expand_all_messages: false,
            show_quoted_text: false,
            zoom: 1.0,
            stored: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Window {
    pub width: f32,
    pub height: f32,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            width: 1_100.0,
            height: 700.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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

/// How the reader opens a conversation, from the settings.
///
/// A small copy of the two choices the reader needs keeps it from depending on
/// the whole of [`Settings`], and keeps the answer to "is this open?" a
/// question about the settings and the reader together rather than a copy that
/// can go stale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reading {
    pub expand_all_messages: bool,
    pub show_quoted_text: bool,
}

impl Settings {
    pub fn reading(&self) -> Reading {
        Reading {
            expand_all_messages: self.expand_all_messages,
            show_quoted_text: self.show_quoted_text,
        }
    }

    /// Reads the settings, falling back to the defaults for anything missing
    /// or out of range.
    pub fn load() -> Self {
        let mut settings = path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<Self>(&text).ok())
            .unwrap_or_default()
            .sanitized();
        // Even defaults are worth storing once the app is running: it is the
        // first run that has nothing on disk yet.
        settings.stored = true;

        settings
    }

    /// Writes the settings. Failures are ignored: this is a convenience, and
    /// a read-only disk is not a reason to interrupt someone's mail.
    pub fn save(&self) {
        if !self.stored {
            return;
        }
        let Some(path) = path() else {
            return;
        };
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, text);
    }

    /// Settings for one run that open a given folder. Tests build them this
    /// way because `stored` stays private: nothing outside `load` may mark
    /// settings as belonging in someone's file.
    #[cfg(test)]
    pub fn opening(folder: Folder) -> Self {
        Self {
            folder,
            ..Self::default()
        }
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

        self
    }
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

fn path() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "Ruston Mail")?;

    Some(dirs.config_dir().join(FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip() {
        let settings = Settings {
            folder: Folder::System(crate::mail::MailFolder::Archive),
            mark_read_on_open: false,
            ..Settings::default()
        };

        let text = serde_json::to_string(&settings).unwrap();

        assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn a_new_choice_starts_at_the_behaviour_the_app_already_had() {
        // An upgrade must not change how anyone's mail opens on its own.
        let defaults = Settings::default();

        assert!(!defaults.expand_all_messages);
        assert!(!defaults.show_quoted_text);
        assert_eq!(defaults.reading(), Reading::default());
    }

    #[test]
    fn missing_fields_fall_back_to_the_defaults() {
        let settings: Settings = serde_json::from_str("{}").unwrap();

        assert_eq!(settings, Settings::default());
        // A file written by an older version keeps whatever it does carry.
        let partial: Settings = serde_json::from_str(r#"{"confirm_links": false}"#).unwrap();
        assert!(!partial.confirm_links);
        assert_eq!(partial.folder, Folder::INBOX);
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
    }
}
