//! Original text color of the desktop listview, for the **fallback mode** of the desktop labels part: `LVM_SETTEXTCOLOR`
//! changes the color inside Explorer itself and is not reverted when the app dies, so the original color is saved to
//! `backup/labels-original.toml` before changing it, and the guard process restores it if the app dies while this mode
//! is in use (`RestoreAction::RestoreDesktopLabels`).

use crate::config::write_atomic;
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "labels-original.toml";
/// comctl32's `CLR_NONE` (transparent text background).
pub const CLR_NONE: u32 = 0xFFFF_FFFF;

/// Colors as the listview returns them (`COLORREF` 0x00BBGGRR, or [`CLR_NONE`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelColors {
    pub text: u32,
    pub text_bk: u32,
}

pub fn path(paths: &Paths) -> PathBuf {
    paths.backup_dir().join(FILE_NAME)
}

pub fn load(p: &Path) -> Option<LabelColors> {
    let text = std::fs::read_to_string(p).ok()?;
    let t: toml::Table = crate::config::strip_bom(&text).parse().ok()?;
    let get = |k: &str| t.get(k).and_then(toml::Value::as_integer).and_then(|v| u32::try_from(v).ok());
    Some(LabelColors { text: get("text")?, text_bk: get("text_bk")? })
}

pub fn save(p: &Path, c: LabelColors) -> io::Result<()> {
    let text = format!(
        "# Màu chữ gốc của icon desktop trước khi ShunguangDecor đổi (cách dự phòng). Xoá sau khi trả lại.\ntext = {}\ntext_bk = {}\n",
        c.text, c.text_bk
    );
    write_atomic(p, text.as_bytes())
}

/// Restores the original colors (if a backup exists), then deletes the backup. `Ok(false)` = nothing to restore.
pub fn restore(p: &Path) -> io::Result<bool> {
    let Some(c) = load(p) else {
        let _ = std::fs::remove_file(p);
        return Ok(false);
    };
    crate::platform::set_desktop_label_colors(c)?;
    let _ = std::fs::remove_file(p);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_corrupt() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("backup").join(FILE_NAME);
        assert_eq!(load(&p), None);
        let c = LabelColors { text: 0x00FF_FFFF, text_bk: CLR_NONE };
        save(&p, c).unwrap();
        assert_eq!(load(&p), Some(c));
        std::fs::write(&p, "text = -1\ntext_bk = 3").unwrap();
        assert_eq!(load(&p), None, "giá trị ngoài khoảng → coi như không có");
    }
}
