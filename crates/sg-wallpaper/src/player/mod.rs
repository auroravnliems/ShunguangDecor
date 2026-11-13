// Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).
//! Wallpaper players. Each running wallpaper = one `Player`.
//! Except for still images (which use the native Windows wallpaper), every player draws into a "host" window
//! that is a child of the desktop layer (see `desktop.rs`).

pub mod app;
pub mod host;
pub mod mf;
pub mod mpv;
pub mod picture;
pub mod web;

use windows::Win32::Foundation::{HWND, RECT};

/// Reason for pausing — decides how to pause (hide completely when covered, or just stop playback).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseReason {
    /// The monitor is fully covered by a maximized / fullscreen app: can be hidden entirely to save GPU.
    Covered,
    /// Low battery, locked machine, the user pressed pause...: keep showing the current frame.
    System,
}

/// Events a player sends back to the main thread (via PostMessage to the main window).
pub const EV_MF_LOADED: u32 = 1;
pub const EV_MF_ERROR: u32 = 2;
pub const EV_WEB_LOADED: u32 = 3;

pub trait Player {
    /// Player name shown in the log/control panel.
    fn backend(&self) -> &'static str;
    fn host(&self) -> Option<HWND>;
    fn set_paused(&mut self, paused: bool, reason: PauseReason);
    fn set_volume(&mut self, volume: u8);
    /// Reset position/size (client coordinates of the desktop parent window).
    fn resize(&mut self, rect: RECT);
    /// Apply a customization (LivelyProperties) to the wallpaper.
    fn apply_property(&mut self, _name: &str, _value: &serde_json::Value) {}
    /// Event from a background thread (MF/Web). Returns `Err(reason)` if the player is broken → the app will try another player.
    fn on_event(&mut self, _code: u32, _param: isize) -> Result<(), String> {
        Ok(())
    }
    /// Called periodically (about every second) — for players that need to wait (external applications).
    fn tick(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn open_devtools(&self) {}
    fn close(&mut self);
}

pub fn rect_size(r: &RECT) -> (i32, i32) {
    (r.right - r.left, r.bottom - r.top)
}

/// Compute the source region (normalized 0..1) and the destination region for fill / fit / stretch.
pub fn fit_rects(video: (u32, u32), target: (i32, i32), scaling: crate::config::Scaling) -> ([f32; 4], RECT) {
    use crate::config::Scaling;
    let (vw, vh) = (video.0.max(1) as f32, video.1.max(1) as f32);
    let (tw, th) = (target.0.max(1) as f32, target.1.max(1) as f32);
    let full = RECT { left: 0, top: 0, right: target.0, bottom: target.1 };
    match scaling {
        Scaling::Stretch => ([0.0, 0.0, 1.0, 1.0], full),
        Scaling::Fill => {
            let va = vw / vh;
            let ta = tw / th;
            if va > ta {
                // the video is wider → crop both sides
                let keep = ta / va;
                let off = (1.0 - keep) / 2.0;
                ([off, 0.0, off + keep, 1.0], full)
            } else {
                let keep = va / ta;
                let off = (1.0 - keep) / 2.0;
                ([0.0, off, 1.0, off + keep], full)
            }
        }
        Scaling::Fit => {
            let s = (tw / vw).min(th / vh);
            let (w, h) = ((vw * s).round() as i32, (vh * s).round() as i32);
            let x = (target.0 - w) / 2;
            let y = (target.1 - h) / 2;
            ([0.0, 0.0, 1.0, 1.0], RECT { left: x, top: y, right: x + w, bottom: y + h })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Scaling;

    #[test]
    fn fill_crops_wide_video() {
        let (src, dst) = fit_rects((3840, 1080), (1920, 1080), Scaling::Fill);
        assert!((src[0] - 0.25).abs() < 1e-4 && (src[2] - 0.75).abs() < 1e-4);
        assert_eq!(dst.right, 1920);
    }

    #[test]
    fn fill_crops_tall_video() {
        let (src, _) = fit_rects((1080, 1920), (1920, 1080), Scaling::Fill);
        assert!(src[1] > 0.0 && src[3] < 1.0);
        assert_eq!(src[0], 0.0);
    }

    #[test]
    fn fit_letterboxes() {
        let (src, dst) = fit_rects((1920, 1080), (1920, 1200), Scaling::Fit);
        assert_eq!(src, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(dst.top, 60);
        assert_eq!(dst.bottom, 1140);
    }
}
