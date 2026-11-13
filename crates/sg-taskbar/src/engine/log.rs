//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! The engine's logs go through `tracing` (target `sg::taskbar`); the app takes care of writing them to file/console.

pub fn info(msg: &str) {
    tracing::info!(target: "sg::taskbar", "{msg}");
}

pub fn warn(msg: &str) {
    tracing::warn!(target: "sg::taskbar", "{msg}");
}

pub fn error(msg: &str) {
    tracing::error!(target: "sg::taskbar", "{msg}");
}

pub fn debug(msg: &str) {
    tracing::debug!(target: "sg::taskbar", "{msg}");
}
