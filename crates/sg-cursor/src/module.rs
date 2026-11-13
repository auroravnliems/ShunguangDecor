//! `CursorModule`: plugs the Cursor module into ModuleHost.

use crate::ScaleMode;
use crate::config::CursorConfig;
use crate::service::CursorService;
use sg_core::ids::ModuleId;
use sg_core::module::{Availability, Module, ModuleStatus};
use sg_core::text::I18nText;
use std::sync::Arc;

pub struct CursorModule {
    service: Arc<CursorService>,
}

impl CursorModule {
    pub fn new(service: Arc<CursorService>) -> CursorModule {
        CursorModule { service }
    }

    fn apply_table(&self, cfg: &toml::Table) -> Result<(), I18nText> {
        self.service.apply_config(&CursorConfig::from_table(cfg))
    }
}

fn text(v: impl ToString) -> I18nText {
    I18nText::new("cursor.val.text").arg("v", v)
}

impl Module for CursorModule {
    fn id(&self) -> ModuleId {
        ModuleId::Cursor
    }

    fn availability(&self) -> Availability {
        Availability::Available
    }

    fn enable(&mut self, cfg: &toml::Table) -> Result<(), I18nText> {
        self.apply_table(cfg)
    }

    fn apply(&mut self, cfg: &toml::Table) -> Result<(), I18nText> {
        self.apply_table(cfg)
    }

    fn disable(&mut self) -> Result<(), I18nText> {
        self.service.restore()
    }

    fn restore_os_default(&mut self) -> Result<(), I18nText> {
        self.service.restore()
    }

    /// The cursor is a persistent Windows setting: leave it as is when the app exits.
    fn on_app_exit(&mut self) -> Result<(), I18nText> {
        Ok(())
    }

    fn status(&self) -> ModuleStatus {
        let Some((id, cfg)) = self.service.applied() else { return ModuleStatus::default() };
        let name = self.service.library().list().into_iter().find(|s| s.id == id);
        let roles = name.as_ref().map(|s| s.role_count).unwrap_or(0);
        let scaling = match cfg.scaling {
            ScaleMode::Sharp => I18nText::new("cursor.val.sharp"),
            ScaleMode::Smooth => I18nText::new("cursor.val.smooth"),
        };
        let scheme_name = name.map(|s| s.name).unwrap_or(id);
        ModuleStatus {
            summary: Some(text(&scheme_name)),
            details: vec![
                (I18nText::new("cursor.status.scheme"), text(scheme_name)),
                (I18nText::new("cursor.status.size"), I18nText::new("cursor.val.px").arg("n", cfg.size)),
                (I18nText::new("cursor.status.scaling"), scaling),
                (I18nText::new("cursor.status.roles"), I18nText::new("cursor.val.roles").arg("n", roles)),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup;
    use crate::service::tests::{make_scheme, original, setup};

    fn table(id: &str) -> toml::Table {
        format!("scheme = \"{id}\"\nsize = 48").parse().unwrap()
    }

    #[test]
    fn on_app_exit_keeps_cursor() {
        let (tmp, svc, fake) = setup();
        let id = make_scheme(&svc, tmp.path(), "Bộ X");
        let mut m = CursorModule::new(svc.clone());
        m.enable(&table(&id)).unwrap();
        let applied = fake.reg.lock().unwrap().clone();
        m.on_app_exit().unwrap();
        assert_eq!(*fake.reg.lock().unwrap(), applied);
        assert!(backup::take_backup(&svc.ctx().paths).unwrap().is_some());
        // Then a real disable → back to the original.
        m.disable().unwrap();
        assert_eq!(*fake.reg.lock().unwrap(), original());
    }

    #[test]
    fn status_reports_scheme_and_size() {
        let (tmp, svc, _f) = setup();
        let id = make_scheme(&svc, tmp.path(), "Bộ Y");
        let mut m = CursorModule::new(svc);
        assert_eq!(m.status(), ModuleStatus::default());
        m.enable(&table(&id)).unwrap();
        let s = m.status();
        assert_eq!(s.details.len(), 4);
        assert_eq!(s.details[1].1.args["n"], "48");
        assert_eq!(s.details[3].1.args["n"], "2");
        assert!(m.enable(&toml::Table::new()).is_err());
    }
}
