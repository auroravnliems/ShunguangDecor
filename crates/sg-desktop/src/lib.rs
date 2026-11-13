//! Monitors and the desktop layer. Shared by `sg-capture`, `sg-wallpaper` and later stages.
//!
//! Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).

pub mod monitor;

pub mod layer;


pub use layer::DesktopLayer;
pub use monitor::{Monitor, Rect, find};
pub use monitor::{monitors, virtual_screen};
