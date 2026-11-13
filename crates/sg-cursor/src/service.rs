//! Service shared between the module (own thread) and the Tauri commands: library + apply/restore.

use crate::backup::{self, RegistrySnapshot, SCHEME_PREFIX};
use crate::config::CursorConfig;
use crate::import;
use crate::library::{Library, io_err};
use crate::{Role, platform};
use sg_core::module::ModuleCtx;
use sg_core::text::I18nText;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Where the registry is read/written and cursors are reloaded; tests swap in a fake.
pub trait Backend: Send + Sync {
    fn read(&self) -> io::Result<RegistrySnapshot>;
    fn write(&self, snapshot: &RegistrySnapshot) -> io::Result<()>;
    /// Sets the session's cursor size (`base_size` = the `CursorBaseSize` just written, `None` = default), then reloads the cursor scheme.
    fn reload(&self, base_size: Option<u32>) -> io::Result<()>;
    /// System DPI, used to generate images at the right size for the primary display.
    fn dpi(&self) -> u32;
}

pub struct SystemBackend;

impl Backend for SystemBackend {
    fn read(&self) -> io::Result<RegistrySnapshot> {
        platform::read_registry()
    }
    fn write(&self, s: &RegistrySnapshot) -> io::Result<()> {
        platform::write_registry(s)
    }
    fn reload(&self, base_size: Option<u32>) -> io::Result<()> {
        platform::reload_cursors(base_size)
    }
    fn dpi(&self) -> u32 {
        platform::system_dpi()
    }
}

#[derive(Default)]
struct State {
    /// The scheme currently applied (module enabled) and the config that was applied.
    applied: Option<(String, CursorConfig)>,
}

pub struct CursorService {
    library: Library,
    ctx: ModuleCtx,
    backend: Box<dyn Backend>,
    state: Mutex<State>,
    /// Only one apply/restore runs at a time (both the module thread and Tauri commands call them).
    op: Mutex<()>,
}

/// Snapshot after applying: the backup is used as the base so roles the scheme lacks keep their old cursors.
pub fn applied_snapshot(
    scheme_name: &str,
    size: u32,
    role_paths: &BTreeMap<Role, std::path::PathBuf>,
    backup: &RegistrySnapshot,
) -> RegistrySnapshot {
    let mut roles = backup.roles.clone();
    for (role, path) in role_paths {
        roles.insert(*role, path.to_string_lossy().into_owned());
    }
    RegistrySnapshot {
        scheme_name: Some(format!("{SCHEME_PREFIX}{scheme_name}")),
        scheme_source: Some(1),
        base_size: Some(size),
        roles,
    }
}

impl CursorService {
    pub fn new(ctx: ModuleCtx) -> Arc<CursorService> {
        CursorService::with_backend(ctx, Box::new(SystemBackend))
    }

    pub fn with_backend(ctx: ModuleCtx, backend: Box<dyn Backend>) -> Arc<CursorService> {
        let library = Library::new(ctx.paths.local.join("cursors"));
        Arc::new(CursorService { library, ctx, backend, state: Mutex::new(State::default()), op: Mutex::new(()) })
    }

    pub fn library(&self) -> &Library {
        &self.library
    }

    pub fn ctx(&self) -> &ModuleCtx {
        &self.ctx
    }

    /// Imports a `.zip`/folder/`install.inf`; tells the UI to reload the list.
    pub fn import(&self, src: &Path) -> Result<Vec<String>, I18nText> {
        let ids = import::import(&self.library, src)?;
        self.ctx.emit("cursor-library-changed", ());
        Ok(ids)
    }

    /// Is this scheme currently applied (module enabled)?
    pub fn active_scheme_guard(&self, id: &str) -> bool {
        self.state().applied.as_ref().is_some_and(|(a, _)| a == id)
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn delete(&self, id: &str) -> Result<(), I18nText> {
        if self.active_scheme_guard(id) {
            return Err(I18nText::new("cursor.err.in_use"));
        }
        self.library.delete(id)?;
        self.ctx.emit("cursor-library-changed", ());
        Ok(())
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<(), I18nText> {
        self.library.rename(id, name)?;
        self.ctx.emit("cursor-library-changed", ());
        Ok(())
    }

    /// Replaces the file of one role; if the scheme is in use it is re-applied immediately.
    pub fn set_role(&self, id: &str, role: Role, file: Option<String>) -> Result<(), I18nText> {
        self.library.set_role(id, role, file)?;
        let cfg = self.state().applied.as_ref().filter(|(a, _)| a == id).map(|(_, c)| c.clone());
        if let Some(cfg) = cfg {
            self.apply_config(&cfg)?;
        }
        self.ctx.emit("cursor-library-changed", ());
        Ok(())
    }

    /// Applies the config to Windows: back up (once) → generate files at the right size → write registry → reload.
    pub fn apply_config(&self, cfg: &CursorConfig) -> Result<(), I18nText> {
        let _op = self.op.lock().unwrap_or_else(|e| e.into_inner());
        let id = cfg.scheme.as_deref().ok_or_else(|| I18nText::new("cursor.err.no_scheme"))?;
        if !self.library.exists(id) {
            return Err(I18nText::new("cursor.err.scheme_missing"));
        }
        let name = self.library.load_name(id)?;
        backup::ensure_backup(&self.ctx.paths, self.library.root(), || self.backend.read()).map_err(io_err)?;
        let side = crate::image_side(cfg.size, self.backend.dpi());
        let role_paths = self.library.build_sized(id, side, cfg.scaling)?;
        if role_paths.is_empty() {
            return Err(I18nText::new("cursor.err.empty_scheme"));
        }
        let original = backup::take_backup(&self.ctx.paths)
            .map_err(io_err)?
            .ok_or_else(|| io_err("backup missing after ensure_backup"))?;
        let snapshot = applied_snapshot(&name, cfg.size, &role_paths, &original);
        if let Err(e) = self.backend.write(&snapshot) {
            // Half-written: try to put it back as it was so Windows is not left in a mixed state.
            let _ = self.backend.write(&original);
            let _ = self.backend.reload(original.base_size);
            return Err(io_err(e));
        }
        self.backend.reload(snapshot.base_size).map_err(io_err)?;
        self.state().applied = Some((id.to_string(), cfg.clone()));
        Ok(())
    }

    /// Writes the backup back exactly, then deletes it. No backup → does nothing.
    pub fn restore(&self) -> Result<(), I18nText> {
        let _op = self.op.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(original) = backup::take_backup(&self.ctx.paths).map_err(io_err)? {
            self.backend.write(&original).map_err(io_err)?;
            self.backend.reload(original.base_size).map_err(io_err)?;
            backup::clear(&self.ctx.paths);
        }
        self.state().applied = None;
        Ok(())
    }

    /// The config that was applied (for `status()`).
    pub fn applied(&self) -> Option<(String, CursorConfig)> {
        self.state().applied.clone()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ScaleMode;
    use crate::test_util::flat_cur;
    use sg_core::paths::Paths;

    #[derive(Default)]
    pub struct Fake {
        pub reg: Mutex<RegistrySnapshot>,
        pub writes: Mutex<u32>,
        pub reloads: Mutex<u32>,
        /// The size passed to the most recent reload.
        pub live_size: Mutex<Option<Option<u32>>>,
        pub dpi: Mutex<u32>,
    }

    pub struct FakeBackend(pub Arc<Fake>);

    impl Backend for FakeBackend {
        fn read(&self) -> io::Result<RegistrySnapshot> {
            Ok(self.0.reg.lock().unwrap().clone())
        }
        fn write(&self, s: &RegistrySnapshot) -> io::Result<()> {
            *self.0.reg.lock().unwrap() = s.clone();
            *self.0.writes.lock().unwrap() += 1;
            Ok(())
        }
        fn reload(&self, base_size: Option<u32>) -> io::Result<()> {
            *self.0.reloads.lock().unwrap() += 1;
            *self.0.live_size.lock().unwrap() = Some(base_size);
            Ok(())
        }
        fn dpi(&self) -> u32 {
            match *self.0.dpi.lock().unwrap() {
                0 => 96,
                d => d,
            }
        }
    }

    pub fn original() -> RegistrySnapshot {
        let mut roles = BTreeMap::new();
        roles.insert(Role::Arrow, r"%SystemRoot%\cursors\aero_arrow.cur".to_string());
        roles.insert(Role::Wait, r"%SystemRoot%\cursors\aero_busy.ani".to_string());
        roles.insert(Role::Help, String::new());
        RegistrySnapshot { scheme_name: Some("Windows Default".into()), scheme_source: Some(2), base_size: Some(48), roles }
    }

    pub fn setup() -> (tempfile::TempDir, Arc<CursorService>, Arc<Fake>) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths { roaming: tmp.path().join("r"), local: tmp.path().join("l"), portable: false };
        let fake = Arc::new(Fake::default());
        *fake.reg.lock().unwrap() = original();
        let svc = CursorService::with_backend(ModuleCtx::for_test(paths), Box::new(FakeBackend(fake.clone())));
        (tmp, svc, fake)
    }

    /// Builds a scheme with 2 roles (Arrow, Hand) by importing from a folder.
    pub fn make_scheme(svc: &CursorService, tmp: &Path, name: &str) -> String {
        let dir = tmp.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Normal Select.cur"), flat_cur(32, 32, [1, 2, 3, 255])).unwrap();
        std::fs::write(dir.join("Link Select.cur"), flat_cur(32, 32, [4, 5, 6, 255])).unwrap();
        svc.import(&dir).unwrap().remove(0)
    }

    fn cfg(id: &str, size: u32) -> CursorConfig {
        CursorConfig { scheme: Some(id.to_string()), size, scaling: ScaleMode::Sharp }
    }

    #[test]
    fn apply_snapshot_keeps_missing_roles_from_backup() {
        let (tmp, svc, fake) = setup();
        let id = make_scheme(&svc, tmp.path(), "Bộ A");
        svc.apply_config(&cfg(&id, 64)).unwrap();
        let reg = fake.reg.lock().unwrap().clone();
        assert_eq!(reg.scheme_name.as_deref(), Some("ShunguangDecor - Bộ A"));
        assert_eq!((reg.scheme_source, reg.base_size), (Some(1), Some(64)));
        // Arrow/Hand come from the scheme; Wait/Help are kept as in the original.
        assert!(reg.roles[&Role::Arrow].contains("sized"));
        assert!(reg.roles[&Role::Hand].contains("sized"));
        assert_eq!(reg.roles[&Role::Wait], r"%SystemRoot%\cursors\aero_busy.ani");
        assert_eq!(reg.roles[&Role::Help], "");
        assert!(!reg.roles.contains_key(&Role::IBeam));
        assert_eq!(*fake.reloads.lock().unwrap(), 1);
        // The session size follows the config (writing only CursorBaseSize does not change the size until the next sign-in).
        assert_eq!(*fake.live_size.lock().unwrap(), Some(Some(64)));
    }

    #[test]
    fn apply_scales_image_to_system_dpi() {
        let (tmp, svc, fake) = setup();
        *fake.dpi.lock().unwrap() = 144;
        let id = make_scheme(&svc, tmp.path(), "Bộ DPI");
        svc.apply_config(&cfg(&id, 48)).unwrap();
        let reg = fake.reg.lock().unwrap().clone();
        // The registry keeps the base size (px at 100%); the generated image at 150% = 72.
        assert_eq!(reg.base_size, Some(48));
        assert!(reg.roles[&Role::Arrow].contains("72-sharp"));
        let img = crate::parse_ico_cur(&std::fs::read(&reg.roles[&Role::Arrow]).unwrap()).unwrap();
        assert_eq!(img.iter().map(|i| i.width).collect::<Vec<_>>(), vec![72]);
    }

    #[test]
    fn apply_missing_scheme_errors() {
        let (_t, svc, fake) = setup();
        let err = svc.apply_config(&cfg("khong-co-000000", 32)).unwrap_err();
        assert_eq!(err.key, "cursor.err.scheme_missing");
        assert_eq!(*fake.writes.lock().unwrap(), 0);
        assert!(!svc.active_scheme_guard("khong-co-000000"));
        assert!(backup::take_backup(&svc.ctx().paths).unwrap().is_none());
    }

    #[test]
    fn apply_without_scheme_errors() {
        let (_t, svc, _f) = setup();
        let err = svc.apply_config(&CursorConfig::default()).unwrap_err();
        assert_eq!(err.key, "cursor.err.no_scheme");
    }

    #[test]
    fn disable_restores_and_clears_backup() {
        let (tmp, svc, fake) = setup();
        let id = make_scheme(&svc, tmp.path(), "Bộ B");
        svc.apply_config(&cfg(&id, 32)).unwrap();
        // A second apply must not change the original.
        svc.apply_config(&cfg(&id, 96)).unwrap();
        assert!(backup::take_backup(&svc.ctx().paths).unwrap().is_some());
        svc.restore().unwrap();
        assert_eq!(*fake.reg.lock().unwrap(), original());
        // The session size goes back to the original size.
        assert_eq!(*fake.live_size.lock().unwrap(), Some(Some(48)));
        assert!(backup::take_backup(&svc.ctx().paths).unwrap().is_none());
        assert!(svc.applied().is_none());
        // No backup → Ok, nothing more is written.
        let writes = *fake.writes.lock().unwrap();
        svc.restore().unwrap();
        assert_eq!(*fake.writes.lock().unwrap(), writes);
    }

    #[test]
    fn delete_active_scheme_refused() {
        let (tmp, svc, _f) = setup();
        let a = make_scheme(&svc, tmp.path(), "Bộ C");
        let b = make_scheme(&svc, tmp.path(), "Bộ D");
        svc.apply_config(&cfg(&a, 32)).unwrap();
        assert_eq!(svc.delete(&a).unwrap_err().key, "cursor.err.in_use");
        assert!(svc.library().exists(&a));
        svc.delete(&b).unwrap();
        svc.restore().unwrap();
        svc.delete(&a).unwrap();
    }

    #[test]
    fn set_role_on_active_scheme_reapplies() {
        let (tmp, svc, fake) = setup();
        let a = make_scheme(&svc, tmp.path(), "Bộ E");
        svc.apply_config(&cfg(&a, 32)).unwrap();
        let before = *fake.reloads.lock().unwrap();
        svc.set_role(&a, Role::Hand, None).unwrap();
        assert_eq!(*fake.reloads.lock().unwrap(), before + 1);
        assert!(!fake.reg.lock().unwrap().roles.contains_key(&Role::Hand));
    }

    #[test]
    fn registry_pointing_at_us_without_backup_uses_windows_default() {
        let (tmp, svc, fake) = setup();
        let id = make_scheme(&svc, tmp.path(), "Bộ F");
        svc.apply_config(&cfg(&id, 32)).unwrap();
        // The backup was deleted by hand.
        backup::clear(&svc.ctx().paths);
        svc.apply_config(&cfg(&id, 40)).unwrap();
        svc.restore().unwrap();
        let reg = fake.reg.lock().unwrap().clone();
        assert_eq!(reg, RegistrySnapshot::windows_default());
    }
}
