use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use windows::{
    Foundation::TypedEventHandler,
    UI::Notifications::{
        NotificationSetting, ToastNotification, ToastNotificationManager, ToastTemplateType,
    },
    Win32::{
        Foundation::PROPERTYKEY,
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile, STGM_READ,
                StructuredStorage::PROPVARIANT,
            },
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
        UI::Shell::{
            FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, PropertiesSystem::IPropertyStore,
            SHGetKnownFolderPath, ShellLink,
        },
    },
    core::{BSTR, GUID, HSTRING, Interface, PCWSTR},
};

use super::{Status, egui};

// System.AppUserModel.ID, defined by Windows' property schema.
const APP_ID_KEY: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: this guard is created after a successful RoInitialize on the
        // same blocking worker and cannot be moved to another thread.
        unsafe { RoUninitialize() };
    }
}

pub(super) fn show(
    sender: &str,
    subject: &str,
    enabled: Arc<AtomicBool>,
    context: egui::Context,
) -> Status {
    let result = || -> Result<Status, Box<dyn std::error::Error>> {
        // SAFETY: the call initializes this blocking worker's WinRT apartment;
        // the guard balances initialization on all return paths.
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        let _apartment = Apartment;
        ensure_shortcut()?;
        if !enabled.load(Ordering::Acquire) {
            return Ok(Status::Off);
        }
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(
            crate::ui::identity::APP_ID,
        ))?;
        if notifier.Setting()? != NotificationSetting::Enabled {
            return Ok(Status::Denied);
        }
        let document =
            ToastNotificationManager::GetTemplateContent(ToastTemplateType::ToastText02)?;
        let texts = document.GetElementsByTagName(&HSTRING::from("text"))?;
        for (index, value) in [sender, subject].into_iter().enumerate() {
            let text = document.CreateTextNode(&HSTRING::from(value))?;
            texts.Item(index as u32)?.AppendChild(&text)?;
        }
        let toast = ToastNotification::CreateToastNotification(&document)?;
        let activation_enabled = enabled.clone();
        toast.Activated(&TypedEventHandler::new(move |_, _| {
            if activation_enabled.load(Ordering::Acquire) {
                context.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::Minimized(false),
                );
                context.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
                context.request_repaint();
            }
            Ok(())
        }))?;
        if !enabled.load(Ordering::Acquire) {
            return Ok(Status::Off);
        }
        notifier.Show(&toast)?;
        Ok(Status::Ready)
    };
    result().unwrap_or(Status::Unavailable)
}

/// Win32 toasts need an AUMID-bearing Start menu shortcut. Registration stays
/// per-user and never replaces a shortcut created by someone else.
fn ensure_shortcut() -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY: known-folder GUID and flags are valid; copy then free the COM
    // allocation regardless of whether UTF-16 conversion succeeds.
    let folder = unsafe {
        let raw = SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None)?;
        let folder = raw.to_string();
        CoTaskMemFree(Some(raw.0.cast()));
        folder?
    };
    let path = std::path::Path::new(&folder).join("Ruston Mail.lnk");
    let path_string = HSTRING::from(path.as_os_str());
    let executable = HSTRING::from(std::env::current_exe()?.as_os_str());
    // SAFETY: COM is initialized here; all strings and property values remain
    // live across each synchronous API call. Interface casts are checked.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let persist: IPersistFile = link.cast()?;
        let properties: IPropertyStore = link.cast()?;
        if path.exists() {
            persist.Load(PCWSTR(path_string.as_ptr()), STGM_READ)?;
            let existing = BSTR::try_from(&properties.GetValue(&APP_ID_KEY)?)?;
            if existing == crate::ui::identity::APP_ID {
                return Ok(());
            }
            return Err("Existing Start menu shortcut belongs to another application".into());
        }
        link.SetPath(PCWSTR(executable.as_ptr()))?;
        link.SetIconLocation(PCWSTR(executable.as_ptr()), 0)?;
        properties.SetValue(&APP_ID_KEY, &PROPVARIANT::from(crate::ui::identity::APP_ID))?;
        properties.Commit()?;
        // Reserve exclusively so a shortcut appearing during setup is preserved.
        let reservation = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        drop(reservation);
        if let Err(error) = persist.Save(PCWSTR(path_string.as_ptr()), true) {
            let _ = std::fs::remove_file(&path);
            return Err(error.into());
        }
    }
    Ok(())
}
