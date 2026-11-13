//! Undo log: records the OLD value before each change, so "Undo all" restores the state from before
//! ShunguangDecor's first change. A TOML file, append-only (`[[entry]]`).

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{FeatureConfig, Priority, Store, Update};

/// `<backup>/features-undo.toml` — the app writes here; the uninstaller reads it to remind the user.
pub fn journal_path(paths: &sg_core::paths::Paths) -> PathBuf {
    paths.backup_dir().join("features-undo.toml")
}

/// One log entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoEntry {
    /// Time of writing (Unix seconds).
    pub unix: u64,
    pub store: Store,
    /// The original configuration before the change; `None` = there was no configuration at that time.
    #[serde(default)]
    pub original: Option<FeatureConfig>,
    pub id: u32,
    pub priority: Priority,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    #[serde(default)]
    entry: Vec<UndoEntry>,
}

pub struct UndoJournal {
    path: PathBuf,
    entries: Vec<UndoEntry>,
}

impl UndoJournal {
    /// Opens (or treats as empty if the file does not exist). A corrupt file returns an `InvalidData` error and is not overwritten.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let entries = match std::fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str::<File>(&text)
                    .map_err(|e| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("nhật ký hoàn tác hỏng: {e}"),
                        )
                    })?
                    .entry
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        Ok(Self { path, entries })
    }

    /// Appends an entry to the file (flushed to disk immediately) and then to memory.
    pub fn record(&mut self, entry: UndoEntry) -> std::io::Result<()> {
        let block = toml::to_string(&File { entry: vec![entry] })
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(block.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        self.entries.push(entry);
        Ok(())
    }

    pub fn entries(&self) -> &[UndoEntry] {
        &self.entries
    }

    /// Undo plan: for each `(id, store, priority)` keep the FIRST original; if the original was "no
    /// configuration", delete the configuration (`Update::reset`, like `ViVeTool /reset`). Order is the reverse of the first writes.
    pub fn plan_undo_all(&self) -> Vec<(Store, Update)> {
        let mut seen = HashSet::new();
        let mut firsts = Vec::new();
        for e in &self.entries {
            if seen.insert((e.id, e.store, e.priority)) {
                firsts.push(e);
            }
        }
        firsts
            .into_iter()
            .rev()
            .map(|e| {
                let update = match &e.original {
                    Some(cfg) => Update::set_from(cfg),
                    None => Update::reset(e.id, e.priority),
                };
                (e.store, update)
            })
            .collect()
    }

    /// Clears the whole log (after the undo completes).
    pub fn clear(&mut self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        self.entries.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EnabledState, Operation, PayloadKind};

    fn cfg(id: u32, state: EnabledState, variant: u8) -> FeatureConfig {
        FeatureConfig {
            id,
            priority: Priority::User,
            state,
            variant,
            variant_payload_kind: PayloadKind::None,
            variant_payload: 0,
            wexp: false,
        }
    }

    fn entry(id: u32, store: Store, original: Option<FeatureConfig>) -> UndoEntry {
        UndoEntry {
            unix: 1,
            store,
            original,
            id,
            priority: Priority::User,
        }
    }

    fn journal() -> (tempfile::TempDir, UndoJournal) {
        let dir = tempfile::tempdir().unwrap();
        let j = UndoJournal::open(dir.path().join("sub").join("features-undo.toml")).unwrap();
        (dir, j)
    }

    #[test]
    fn undo_plan_uses_first_original() {
        let (_d, mut j) = journal();
        let a1 = cfg(10, EnabledState::Disabled, 3);
        let a2 = cfg(10, EnabledState::Enabled, 5);
        j.record(entry(10, Store::Runtime, Some(a1))).unwrap();
        j.record(entry(
            20,
            Store::Runtime,
            Some(cfg(20, EnabledState::Enabled, 0)),
        ))
        .unwrap();
        // Second change of id 10: the original recorded then is a2, which must be ignored.
        j.record(entry(10, Store::Runtime, Some(a2))).unwrap();
        // The same id in a different store is a separate key.
        j.record(entry(10, Store::Boot, Some(a2))).unwrap();

        let plan = j.plan_undo_all();
        assert_eq!(plan.len(), 3);
        // Reverse of the first-write order: (10,Boot), (20,Runtime), (10,Runtime).
        assert_eq!(plan[0].0, Store::Boot);
        assert_eq!(plan[0].1, Update::set_from(&a2));
        assert_eq!(plan[1].1.id, 20);
        assert_eq!(plan[2].0, Store::Runtime);
        assert_eq!(plan[2].1, Update::set_from(&a1));
        assert_eq!(plan[2].1.variant, 3);
    }

    #[test]
    fn undo_plan_resets_when_originally_absent() {
        let (_d, mut j) = journal();
        j.record(entry(7, Store::Runtime, None)).unwrap();
        // A second change (whose original at the time is a configuration we set) does not change the plan.
        j.record(entry(
            7,
            Store::Runtime,
            Some(cfg(7, EnabledState::Enabled, 0)),
        ))
        .unwrap();
        let plan = j.plan_undo_all();
        assert_eq!(plan.len(), 1);
        let (store, u) = plan[0];
        assert_eq!(store, Store::Runtime);
        assert_eq!(u, Update::reset(7, Priority::User));
        assert!(u.operation.contains(Operation::RESET_STATE));
    }

    #[test]
    fn undo_journal_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("features-undo.toml");
        {
            let mut j = UndoJournal::open(&path).unwrap();
            j.record(entry(1, Store::Runtime, None)).unwrap();
            j.record(entry(
                2,
                Store::Boot,
                Some(cfg(2, EnabledState::Disabled, 9)),
            ))
            .unwrap();
        }
        let mut j = UndoJournal::open(&path).unwrap();
        assert_eq!(j.entries().len(), 2);
        assert_eq!(j.entries()[0].original, None);
        assert_eq!(j.entries()[1].original.unwrap().variant, 9);
        j.clear().unwrap();
        assert!(j.entries().is_empty());
        assert!(UndoJournal::open(&path).unwrap().entries().is_empty());
    }

    #[test]
    fn undo_journal_rejects_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.toml");
        std::fs::write(&path, "[[entry]]\nid = \"abc\"").unwrap();
        assert!(UndoJournal::open(&path).is_err());
    }
}
