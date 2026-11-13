//! Identifiers of the 5 parts. Order = order in the menu and on the home page.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleId {
    /// 01 — YASB taskbar (separate bar)
    Taskbar,
    /// 02 — Live wallpaper
    Wallpaper,
    /// 03 — Mouse cursors
    Cursor,
    /// 04 — Desktop file-name label colors
    Labels,
    /// 05 — Hidden Windows features
    Features,
}

impl ModuleId {
    pub const ALL: [ModuleId; 5] = [
        ModuleId::Taskbar,
        ModuleId::Wallpaper,
        ModuleId::Cursor,
        ModuleId::Labels,
        ModuleId::Features,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ModuleId::Taskbar => "taskbar",
            ModuleId::Wallpaper => "wallpaper",
            ModuleId::Cursor => "cursor",
            ModuleId::Labels => "labels",
            ModuleId::Features => "features",
        }
    }

    /// Ordinal 1…5 (the "›››01" tag).
    pub fn number(self) -> u8 {
        ModuleId::ALL.iter().position(|&m| m == self).map(|i| i as u8 + 1).unwrap_or(0)
    }

    pub fn parse(s: &str) -> Option<ModuleId> {
        ModuleId::ALL.into_iter().find(|m| m.as_str() == s)
    }
}

impl fmt::Display for ModuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_id_order_and_numbers() {
        let names: Vec<&str> = ModuleId::ALL.iter().map(|m| m.as_str()).collect();
        assert_eq!(names, ["taskbar", "wallpaper", "cursor", "labels", "features"]);
        let nums: Vec<u8> = ModuleId::ALL.iter().map(|m| m.number()).collect();
        assert_eq!(nums, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn module_id_parse_and_serde() {
        assert_eq!(ModuleId::parse("cursor"), Some(ModuleId::Cursor));
        assert_eq!(ModuleId::parse("abc"), None);
        assert_eq!(serde_json::to_string(&ModuleId::Labels).unwrap(), "\"labels\"");
        assert_eq!(ModuleId::Features.to_string(), "features");
    }
}
