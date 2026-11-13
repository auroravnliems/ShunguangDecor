// Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).
//! Static image: set directly as the Windows wallpaper via IDesktopWallpaper (per monitor).
//! Creates no window and uses no CPU/RAM while running.

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_LOCAL_SERVER};
use windows::Win32::UI::Shell::{
    DesktopWallpaper, IDesktopWallpaper, DESKTOP_WALLPAPER_POSITION, DWPOS_FILL, DWPOS_FIT, DWPOS_SPAN,
    DWPOS_STRETCH,
};

use super::{PauseReason, Player};
use crate::config::Scaling;
use crate::util::wide;

pub struct PicturePlayer;

fn find_monitor_id(dw: &IDesktopWallpaper, rect: RECT) -> Option<Vec<u16>> {
    unsafe {
        let n = dw.GetMonitorDevicePathCount().ok()?;
        for i in 0..n {
            let Ok(id) = dw.GetMonitorDevicePathAt(i) else { continue };
            let r = dw.GetMonitorRECT(PCWSTR(id.0));
            let owned = pwstr_to_vec(id);
            CoTaskMemFree(Some(id.0 as *const _));
            if let Ok(r) = r {
                if r == rect {
                    return Some(owned);
                }
            }
        }
    }
    None
}

unsafe fn pwstr_to_vec(p: PWSTR) -> Vec<u16> {
    let mut v = p.as_wide().to_vec();
    v.push(0);
    v
}

impl PicturePlayer {
    /// `monitor_rect = None` means span across every monitor.
    pub fn new(path: &str, monitor_rect: Option<RECT>, scaling: Scaling, backup_file: &std::path::Path) -> Result<PicturePlayer, String> {
        // Before the first change of the Windows wallpaper: save the old one so turning the module off restores it exactly.
        crate::backup::ensure_backup(backup_file)?;
        unsafe {
            let dw: IDesktopWallpaper = CoCreateInstance(&DesktopWallpaper, None, CLSCTX_LOCAL_SERVER)
                .map_err(|e| format!("Không mở được IDesktopWallpaper: {e}"))?;
            let file = wide(path);
            let position: DESKTOP_WALLPAPER_POSITION = match (monitor_rect, scaling) {
                (None, _) => DWPOS_SPAN,
                (_, Scaling::Fill) => DWPOS_FILL,
                (_, Scaling::Fit) => DWPOS_FIT,
                (_, Scaling::Stretch) => DWPOS_STRETCH,
            };
            match monitor_rect {
                Some(rect) => {
                    let id = find_monitor_id(&dw, rect).ok_or("Không xác định được màn hình để đặt ảnh")?;
                    dw.SetWallpaper(PCWSTR(id.as_ptr()), file.pcwstr())
                        .map_err(|e| format!("Đặt ảnh nền thất bại: {e}"))?;
                }
                None => {
                    dw.SetWallpaper(PCWSTR::null(), file.pcwstr()).map_err(|e| format!("Đặt ảnh nền thất bại: {e}"))?;
                }
            }
            let _ = dw.SetPosition(position);
            logln!("set the Windows wallpaper: {path}");
            Ok(PicturePlayer)
        }
    }
}

impl Player for PicturePlayer {
    fn backend(&self) -> &'static str {
        "windows-wallpaper"
    }
    fn host(&self) -> Option<HWND> {
        None
    }
    fn set_paused(&mut self, _paused: bool, _reason: PauseReason) {}
    fn set_volume(&mut self, _volume: u8) {}
    fn resize(&mut self, _rect: RECT) {}
    fn close(&mut self) {}
}
