//! Backs up and restores the Windows wallpaper: the static-image player changes the real Windows wallpaper,
//! so before the first change we record each display's image path + placement style; disabling the module restores exactly what was there.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct MonitorWallpaper {
    /// The display's device path in `IDesktopWallpaper`.
    pub monitor: String,
    /// The image currently set for that display (empty = no image).
    pub wallpaper: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct WallpaperBackup {
    pub monitors: Vec<MonitorWallpaper>,
    /// The `DESKTOP_WALLPAPER_POSITION` value (0 center, 1 tile, 2 stretch, 3 fit, 4 fill, 5 span).
    pub position: i32,
}

impl WallpaperBackup {
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
    pub fn from_toml(s: &str) -> Option<WallpaperBackup> {
        toml::from_str(s).ok()
    }
}

/// Writes the backup if none exists (if one exists it is kept: it holds the user's real original wallpaper,
/// even when the previous run crashed midway).
pub fn ensure_backup(file: &Path) -> Result<(), String> {
    if file.is_file() {
        return Ok(());
    }
    let snap = platform::read_current()?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    sg_core::config::write_atomic(file, snap.to_toml().as_bytes()).map_err(|e| e.to_string())?;
    tracing::info!(target: "sg::wallpaper", "saved original Windows wallpaper ({} monitors)", snap.monitors.len());
    Ok(())
}

/// Restores the old wallpaper, then deletes the backup. With no backup it does nothing.
pub fn restore_and_clear(file: &Path) -> Result<bool, String> {
    let Ok(text) = std::fs::read_to_string(file) else { return Ok(false) };
    let Some(b) = WallpaperBackup::from_toml(sg_core::config::strip_bom(&text)) else {
        // Corrupt file: keep it for the user to inspect, do not guess.
        return Err("wallpaper backup file is unreadable".into());
    };
    platform::apply(&b)?;
    let _ = std::fs::remove_file(file);
    tracing::info!(target: "sg::wallpaper", "restored original Windows wallpaper");
    Ok(true)
}

mod platform {
    use super::{MonitorWallpaper, WallpaperBackup};
    use crate::util::wide;
    use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::{DESKTOP_WALLPAPER_POSITION, DesktopWallpaper, IDesktopWallpaper};
    use windows::core::{PCWSTR, PWSTR};

    fn dw() -> Result<IDesktopWallpaper, String> {
        // SAFETY: COM has been initialized by the engine on this thread.
        unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_LOCAL_SERVER) }.map_err(|e| format!("IDesktopWallpaper: {e}"))
    }

    /// Takes the string from a COM-allocated PWSTR and then frees it.
    unsafe fn take(p: PWSTR) -> String {
        // SAFETY: p was allocated by COM (CoTaskMem) and is still valid.
        unsafe {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        }
    }

    pub fn read_current() -> Result<WallpaperBackup, String> {
        let d = dw()?;
        // SAFETY: COM calls on the object just created; the returned strings are freed by `take`.
        unsafe {
            let n = d.GetMonitorDevicePathCount().map_err(|e| e.to_string())?;
            let mut monitors = Vec::new();
            for i in 0..n {
                let Ok(id) = d.GetMonitorDevicePathAt(i) else { continue };
                let wp = d.GetWallpaper(PCWSTR(id.0)).map(|p| take(p)).unwrap_or_default();
                monitors.push(MonitorWallpaper { monitor: take(id), wallpaper: wp });
            }
            let position = d.GetPosition().map(|p| p.0).unwrap_or(4);
            Ok(WallpaperBackup { monitors, position })
        }
    }

    pub fn apply(b: &WallpaperBackup) -> Result<(), String> {
        let d = dw()?;
        let mut first_err = None;
        // SAFETY: COM calls on the object just created; the wide string lives until the end of the call.
        unsafe {
            for m in &b.monitors {
                let id = wide(&m.monitor);
                let wp = wide(&m.wallpaper);
                if let Err(e) = d.SetWallpaper(id.pcwstr(), wp.pcwstr()) {
                    first_err.get_or_insert(e.to_string());
                }
            }
            let _ = d.SetPosition(DESKTOP_WALLPAPER_POSITION(b.position));
        }
        first_err.map_or(Ok(()), Err)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_roundtrips_through_toml() {
        let b = WallpaperBackup {
            monitors: vec![
                MonitorWallpaper { monitor: r"\\?\DISPLAY#X#1".into(), wallpaper: r"C:\a b\ảnh.png".into() },
                MonitorWallpaper { monitor: "m2".into(), wallpaper: String::new() },
            ],
            position: 4,
        };
        assert_eq!(WallpaperBackup::from_toml(&b.to_toml()), Some(b));
        assert_eq!(WallpaperBackup::from_toml("không phải toml {"), None);
    }

    #[test]
    fn restore_without_backup_is_noop() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(restore_and_clear(&d.path().join("none.toml")), Ok(false));
    }

    #[test]
    fn restore_keeps_unreadable_backup() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("b.toml");
        std::fs::write(&f, "xyz = [").unwrap();
        assert!(restore_and_clear(&f).is_err());
        assert!(f.exists());
    }
}

#[cfg(test)]
mod real_tests {
    /// Run manually: prints the current Windows wallpaper via `IDesktopWallpaper` (writes nothing).
    #[test]
    #[ignore]
    fn print_current_windows_wallpaper() {
        // SAFETY: initializes COM for the test thread.
        let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED) };
        println!("{:?}", super::platform::read_current());
    }
}
