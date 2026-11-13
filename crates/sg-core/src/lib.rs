//! Core of ShunguangDecor. It does not depend on Tauri, so it can be tested and reused
//! by the guard process (`--guard`).

pub mod archive;
pub mod config;
pub mod desktop;
pub mod explorer_identity;
pub mod guard;
pub mod host;
pub mod i18n;
pub mod ids;
pub mod labels_backup;
pub mod logging;
pub mod module;
pub mod paths;
pub mod platform;
pub mod profiles;
pub mod report;
pub mod sysinfo;
pub mod placeholder;
pub mod text;
pub mod transfer;
