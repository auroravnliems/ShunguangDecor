//! Fallback for when the DLL cannot be loaded (blocked by antivirus, …): one shared color via `LVM_SETTEXTCOLOR` +
//! `LVM_SETTEXTBKCOLOR(CLR_NONE)`, then a repaint. Turning off → restores exactly the color read at the start (saved in
//! `backup/labels-original.toml`; the guard process can also restore it if the app dies).

use sg_core::labels_backup::{self, CLR_NONE, LabelColors};
use std::path::PathBuf;

/// Reads/writes the desktop listview's text colors (real: `sg_core::platform`; tests: fake).
pub trait ListViewColors: Send + Sync {
    fn read(&self) -> Option<LabelColors>;
    fn write(&self, c: LabelColors) -> bool;
}

pub struct RealListView;

impl ListViewColors for RealListView {
    fn read(&self) -> Option<LabelColors> {
        sg_core::platform::desktop_label_colors()
    }
    fn write(&self, c: LabelColors) -> bool {
        sg_core::platform::set_desktop_label_colors(c).is_ok()
    }
}

/// 0xRRGGBB → COLORREF 0x00BBGGRR.
pub fn colorref(rgb: u32) -> u32 {
    ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)
}

pub struct Fallback {
    ops: Box<dyn ListViewColors>,
    backup: PathBuf,
}

impl Fallback {
    pub fn new(ops: Box<dyn ListViewColors>, backup: PathBuf) -> Fallback {
        Fallback { ops, backup }
    }

    /// The fallback is in use (a backup of the original colors exists).
    pub fn is_applied(&self) -> bool {
        self.backup.exists()
    }

    /// Whether the applied color is still `rgb` (if Explorer restarts or the theme changes, the color is reset).
    pub fn matches(&self, rgb: u32) -> bool {
        self.ops.read().is_some_and(|c| c.text == colorref(rgb) && c.text_bk == CLR_NONE)
    }

    /// One shared color for all file names. The first time it saves the original color; an existing backup (even from a previous crashed run) is kept.
    pub fn apply_uniform(&self, rgb: u32) -> Result<(), String> {
        if !self.backup.exists() {
            let orig = self.ops.read().ok_or("cannot read the desktop label colors")?;
            labels_backup::save(&self.backup, orig).map_err(|e| e.to_string())?;
        }
        if self.ops.write(LabelColors { text: colorref(rgb), text_bk: CLR_NONE }) {
            Ok(())
        } else {
            Err("explorer did not accept the text color".into())
        }
    }

    /// Restores the original color. `Ok(false)` = nothing to restore.
    pub fn restore(&self) -> Result<bool, String> {
        let Some(orig) = labels_backup::load(&self.backup) else {
            let _ = std::fs::remove_file(&self.backup);
            return Ok(false);
        };
        if !self.ops.write(orig) {
            return Err("explorer did not accept the original text color".into());
        }
        let _ = std::fs::remove_file(&self.backup);
        Ok(true)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Fake listview: remembers the current color plus every write.
    #[derive(Clone)]
    pub struct FakeLv(pub Arc<Mutex<(LabelColors, Vec<LabelColors>)>>);

    impl FakeLv {
        pub fn new(text: u32) -> FakeLv {
            FakeLv(Arc::new(Mutex::new((LabelColors { text, text_bk: CLR_NONE }, Vec::new()))))
        }
        pub fn current(&self) -> LabelColors {
            self.0.lock().unwrap().0
        }
        pub fn writes(&self) -> Vec<LabelColors> {
            self.0.lock().unwrap().1.clone()
        }
    }

    impl ListViewColors for FakeLv {
        fn read(&self) -> Option<LabelColors> {
            Some(self.current())
        }
        fn write(&self, c: LabelColors) -> bool {
            let mut g = self.0.lock().unwrap();
            g.0 = c;
            g.1.push(c);
            true
        }
    }

    #[test]
    fn fallback_restore_uses_saved_colors() {
        let d = tempfile::tempdir().unwrap();
        let lv = FakeLv::new(0x00FF_FFFF);
        let fb = Fallback::new(Box::new(lv.clone()), d.path().join("backup").join("labels-original.toml"));
        assert!(!fb.is_applied());
        fb.apply_uniform(0xFF69B4).unwrap();
        assert_eq!(lv.current().text, 0xB469FF, "COLORREF đảo kênh");
        assert!(fb.is_applied() && fb.matches(0xFF69B4));
        // A second apply must not overwrite the backup with the pink color.
        fb.apply_uniform(0x00FF00).unwrap();
        assert!(fb.restore().unwrap());
        assert_eq!(lv.current(), LabelColors { text: 0x00FF_FFFF, text_bk: CLR_NONE }, "trả đúng màu đọc lúc đầu");
        assert!(!fb.is_applied());
        assert!(!fb.restore().unwrap(), "không có gì để trả");
        assert_eq!(lv.writes().len(), 3);
    }

    #[test]
    fn leftover_backup_from_crash_is_kept() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("labels-original.toml");
        labels_backup::save(&p, LabelColors { text: 0x00FF_FFFF, text_bk: CLR_NONE }).unwrap();
        // App dies while pink: the listview stays pink; the next run applies again, then turns off → must go back to white.
        let lv = FakeLv::new(colorref(0xFF69B4));
        let fb = Fallback::new(Box::new(lv.clone()), p);
        fb.apply_uniform(0x123456).unwrap();
        fb.restore().unwrap();
        assert_eq!(lv.current().text, 0x00FF_FFFF);
    }
}
