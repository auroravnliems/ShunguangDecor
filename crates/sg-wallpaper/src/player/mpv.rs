// Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).
//! mpv player (optional): plays almost any format (mkv, 10-bit HEVC, GIF...).
//! mpv draws straight into the host window via `--wid` and is controlled over a named-pipe JSON IPC.

use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use windows::Win32::Foundation::{HWND, RECT};

use super::{host, PauseReason, Player};
use sg_desktop::DesktopLayer as Desktop;
use crate::config::{Scaling, WallpaperConfig};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Locates mpv.exe: settings → next to the exe → the mpv\ folder → Scoop → PATH.
pub fn find_mpv(settings: &WallpaperConfig) -> Option<PathBuf> {
    let configured = settings.mpv_path.trim();
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        return p.exists().then_some(p);
    }
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let mut candidates = vec![exe_dir.join("mpv.exe"), exe_dir.join("mpv").join("mpv.exe")];
    if let Some(home) = std::env::var_os("USERPROFILE") {
        candidates.push(Path::new(&home).join(r"scoop\apps\mpv\current\mpv.exe"));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        candidates.push(Path::new(&pf).join(r"mpv\mpv.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join("mpv.exe"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

pub struct MpvPlayer {
    host: HWND,
    child: Child,
    pipe: String,
    paused: bool,
}

impl MpvPlayer {
    pub fn new(
        desktop: &Desktop,
        rect: RECT,
        mpv: &Path,
        source: &str,
        settings: &WallpaperConfig,
        is_gif: bool,
    ) -> Result<MpvPlayer, String> {
        let host = host::create(desktop, rect)?;
        let pipe = format!(r"\\.\pipe\tachyon-mpv-{}", host.0 as isize);
        let mut cmd = Command::new(mpv);
        cmd.arg(format!("--wid={}", host.0 as isize))
            .arg(format!("--input-ipc-server={pipe}"))
            .args([
                "--no-config",
                "--loop-file=inf",
                "--keep-open=yes",
                "--force-window=yes",
                "--no-border",
                "--no-osc",
                "--osd-level=0",
                "--no-input-default-bindings",
                "--input-cursor=no",
                "--cursor-autohide=no",
                "--stop-screensaver=no",
                "--media-controls=no",
                "--background-color=#000000",
                "--msg-level=all=error",
            ])
            .arg(format!("--volume={}", settings.volume))
            .arg(if settings.volume == 0 { "--mute=yes" } else { "--mute=no" })
            .arg(if settings.hardware_decoding { "--hwdec=auto-safe" } else { "--hwdec=no" });
        match settings.scaling {
            Scaling::Fill => {
                cmd.arg("--panscan=1.0");
            }
            Scaling::Stretch => {
                cmd.arg("--keepaspect=no");
            }
            Scaling::Fit => {}
        }
        if is_gif {
            cmd.arg("--scale=nearest");
        }
        cmd.arg("--").arg(source);
        cmd.creation_flags(CREATE_NO_WINDOW);
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                host::destroy(host);
                return Err(format!("Không chạy được mpv ({}): {e}", mpv.display()));
            }
        };
        crate::job::adopt(&child);
        logln!("mpv pid {} phát {source}", child.id());
        Ok(MpvPlayer { host, child, pipe, paused: false })
    }

    fn send(&self, json: &str) {
        match std::fs::OpenOptions::new().write(true).open(&self.pipe) {
            Ok(mut f) => {
                let _ = f.write_all(json.as_bytes());
                let _ = f.write_all(b"\n");
            }
            Err(e) => logln!("mpv IPC chưa sẵn sàng: {e}"),
        }
    }
}

impl Player for MpvPlayer {
    fn backend(&self) -> &'static str {
        "mpv"
    }

    fn host(&self) -> Option<HWND> {
        Some(self.host)
    }

    fn set_paused(&mut self, paused: bool, _reason: PauseReason) {
        if paused != self.paused {
            self.paused = paused;
            self.send(&format!(r#"{{"command":["set_property","pause",{paused}]}}"#));
        }
    }

    fn set_volume(&mut self, volume: u8) {
        self.send(&format!(r#"{{"command":["set_property","volume",{volume}]}}"#));
        self.send(&format!(r#"{{"command":["set_property","mute",{}]}}"#, volume == 0));
    }

    fn resize(&mut self, rect: RECT) {
        host::move_to(self.host, rect);
    }

    fn tick(&mut self) -> Result<(), String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Err(format!("mpv đã thoát ({status})")),
            _ => Ok(()),
        }
    }

    fn close(&mut self) {
        self.send(r#"{"command":["quit"]}"#);
        // Give mpv 300 ms to exit by itself before force-terminating it.
        for _ in 0..6 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        host::destroy(self.host);
    }
}
