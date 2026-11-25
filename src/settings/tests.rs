use super::*;

#[test]
fn settings_survive_a_round_trip() {
    let mut settings = Settings {
        mark_read_on_open: false,
        check_recipient_protection: false,
        appearance: Appearance::Light,
        system_font: true,
        text_scale: 1.3,
        ..Settings::default()
    };
    settings
        .accounts
        .set_folder(Folder::System(crate::mail::MailFolder::Archive));

    let text = serde_json::to_string(&settings).unwrap();

    assert_eq!(serde_json::from_str::<Settings>(&text).unwrap(), settings);
}

#[test]
fn text_is_only_ever_drawn_at_a_size_settings_offers() {
    assert_eq!(Settings::default().text_scale, 1.0);

    // Settings saved before this choice existed draw text as they did.
    let mut old = serde_json::to_value(Settings {
        text_scale: 1.3,
        ..Settings::default()
    })
    .unwrap();
    old.as_object_mut().unwrap().remove("text_scale");
    let restored: Settings = serde_json::from_value(old).unwrap();
    assert_eq!(restored.text_scale, 1.0);

    // A hand-edited file gets the nearest size offered.
    let at = |text_scale| {
        Settings {
            text_scale,
            ..Settings::default()
        }
        .sanitized()
        .text_scale
    };
    for offered in TEXT_SCALES {
        assert_eq!(at(offered), offered);
    }
    assert_eq!(at(1.2), 1.15);
    assert_eq!(at(1.25), 1.3);
    assert_eq!(at(0.0), 0.9);
    assert_eq!(at(-3.0), 0.9);
    assert_eq!(at(9.0), 1.5);
    assert_eq!(at(f32::NAN), 1.0);
    assert_eq!(at(f32::INFINITY), 1.0);
    assert_eq!(TEXT_SCALES, [0.9, 1.0, 1.15, 1.3, 1.5]);
}

#[test]
fn older_settings_have_the_one_account_they_signed_in_to() {
    let mut old = serde_json::to_value(Settings::default()).unwrap();
    old.as_object_mut().unwrap().remove("accounts");
    let restored: Settings = serde_json::from_value(old).unwrap();
    assert_eq!(restored.accounts, Accounts::default());
    assert_eq!(restored.accounts.active_profile(), "ruston");
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
fn updates_are_checked_only_once_asked_to() {
    assert!(!Settings::default().check_for_updates);

    // Settings saved before this choice existed do not ask either.
    let mut old = serde_json::to_value(Settings {
        check_for_updates: true,
        ..Settings::default()
    })
    .unwrap();
    old.as_object_mut().unwrap().remove("check_for_updates");
    let restored: Settings = serde_json::from_value(old).unwrap();
    assert!(!restored.check_for_updates);
}

#[test]
fn notices_stay_five_seconds_until_asked_otherwise() {
    assert_eq!(Settings::default().notice_seconds, 5);

    // Settings saved before this choice existed have the same.
    let mut old = serde_json::to_value(Settings {
        notice_seconds: 9,
        ..Settings::default()
    })
    .unwrap();
    old.as_object_mut().unwrap().remove("notice_seconds");
    let restored: Settings = serde_json::from_value(old).unwrap();
    assert_eq!(restored.notice_seconds, 5);

    // A hand-edited file can neither flash them by nor keep them up.
    let at = |notice_seconds| {
        Settings {
            notice_seconds,
            ..Settings::default()
        }
        .sanitized()
        .notice_seconds
    };
    assert_eq!(at(0), MIN_NOTICE_SECONDS);
    assert_eq!(at(1), MIN_NOTICE_SECONDS);
    assert_eq!(at(7), 7);
    assert_eq!(at(60), MAX_NOTICE_SECONDS);
    assert_eq!(at(u64::MAX), MAX_NOTICE_SECONDS);
    assert_eq!((MIN_NOTICE_SECONDS, MAX_NOTICE_SECONDS), (3, 10));
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
fn this_device_is_searched_only_once_asked() {
    assert!(!Settings::default().search_device_offline);
    // Settings saved before this choice existed leave it off.
    let mut old = serde_json::to_value(Settings::default()).unwrap();
    old.as_object_mut().unwrap().remove("search_device_offline");
    let restored: Settings = serde_json::from_value(old).unwrap();
    assert!(!restored.search_device_offline);

    let chosen = Settings {
        search_device_offline: true,
        ..Settings::default()
    };
    let text = serde_json::to_string(&chosen).unwrap();
    let restored: Settings = serde_json::from_str(&text).unwrap();
    assert!(restored.search_device_offline);
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
    let name = |file| path(file).unwrap().file_name().unwrap().to_owned();
    assert_eq!(name(SettingsFile::Main), FILE);
    assert_eq!(name(SettingsFile::Demo), DEMO_FILE);
    assert_eq!(
        path(SettingsFile::Demo).unwrap().parent(),
        path(SettingsFile::Main).unwrap().parent()
    );

    // What is read remembers its file, to be written back there. Read
    // from a folder of the test's own: the person's settings stay as
    // they are, whatever state their file is in.
    let folder = empty_folder("demo");
    let demo = read(&folder.join(DEMO_FILE), SettingsFile::Demo);
    assert_eq!(demo.file, Some(SettingsFile::Demo));
    let main = read(&folder.join(FILE), SettingsFile::Main);
    assert_eq!(main.file, Some(SettingsFile::Main));
    // Which file they came from is bookkeeping, and never reaches it.
    let text = serde_json::to_string(&demo).unwrap();
    assert!(!text.contains("demo") && !text.contains("\"file\""));
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn only_loaded_settings_are_written_back() {
    // Settings built in memory, as every test does, must never reach the
    // config file someone actually uses.
    assert_eq!(Settings::default().file, None);

    let loaded = Settings {
        file: Some(SettingsFile::Main),
        ..Settings::default()
    };
    // Where they came from is bookkeeping, not content, so it never
    // reaches the file.
    let text = serde_json::to_string(&loaded).unwrap();
    assert!(!text.contains("\"file\""));
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

/// What a version from before 0.8.0 wrote once every choice was changed:
/// without the fields added since, and with the one folder for every
/// account that is no longer read.
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
fn settings_written_by_an_older_version_still_load() {
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
        file: Some(SettingsFile::Main),
        // A field added since keeps its default.
        ..Settings::default()
    };
    let settings = read(&path, SettingsFile::Main);
    assert_eq!(settings, expected);
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);

    // Written back and read again, they are the same.
    let written = serde_json::to_string_pretty(&settings).unwrap();
    fs::write(&path, written).unwrap();
    let again = read(&path, SettingsFile::Main);
    assert_eq!(again, settings);
    assert_eq!(again.start_folder(), Folder::System(MailFolder::Archive));
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn a_damaged_file_is_kept_aside_before_defaults_replace_it() {
    let folder = empty_folder("damaged");
    let path = folder.join(FILE);
    let writable_defaults = Settings {
        file: Some(SettingsFile::Main),
        ..Settings::default()
    };
    // A first run has nothing to keep.
    assert_eq!(read(&path, SettingsFile::Main), writable_defaults);
    assert_eq!(fs::read_dir(&folder).unwrap().count(), 0);

    // A hand edit that lost a comma.
    let damaged = RELEASED.replacen(',', "", 1);
    fs::write(&path, &damaged).unwrap();

    assert_eq!(read(&path, SettingsFile::Main), writable_defaults);
    assert!(!path.exists());
    let kept = folder.join("settings.json.unreadable");
    assert_eq!(fs::read_to_string(kept).unwrap(), damaged);
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn settings_no_file_takes_count_as_saved() {
    // Built in memory, as every test does: nothing is written, nothing waits.
    assert!(Settings::default().save());
}

#[test]
fn a_damaged_file_that_cannot_be_kept_aside_is_not_written_over() {
    let folder = empty_folder("unmovable");
    let path = folder.join(FILE);
    fs::write(&path, b"{").unwrap();
    // Something already holds the name it would be kept under.
    fs::create_dir_all(folder.join("settings.json.unreadable").join("taken")).unwrap();

    let settings = read(&path, SettingsFile::Main);

    assert_eq!(settings.file, None);
    assert_eq!(settings, Settings::default());
    assert_eq!(fs::read(&path).unwrap(), b"{");
    let _ = fs::remove_dir_all(folder);
}

#[test]
fn a_pinned_folder_wins_over_the_one_last_read() {
    let last_read = Folder::custom("kZ9", "Invoices");
    let settings = Settings::opening(last_read.clone());

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
    assert_eq!(pinned.accounts.folder(), last_read);
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
