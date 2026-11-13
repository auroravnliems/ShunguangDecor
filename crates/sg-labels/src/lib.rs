//! Desktop labels — the color of desktop file/icon names, core side.
//!
//! - `config`: `config/labels.toml` (Auto / Custom).
//! - `auto`: background brightness behind each label, hysteresis against flicker, automatic text style.
//! - `fallback`: a single shared color via `LVM_SETTEXTCOLOR` when the DLL cannot be loaded.
//! - `module`: `LabelsService` + `LabelsModule` (loads the DLL via `sg-explorer`, sends the paint table, status).
//!
//! The actual drawing lives in the DLL (`sg-explorer-hook`); this part only computes and sends commands.

pub mod auto;
pub mod config;
pub mod fallback;
pub mod module;

pub use config::{LabelsConfig, Mode, TextStyleCfg};
pub use module::{Backgrounds, BgIdentity, LabelsModule, LabelsService, LabelsStatus, Method};
