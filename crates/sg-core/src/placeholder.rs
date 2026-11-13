//! Placeholder "coming soon" part: shows a card and a page but can't be enabled. Now only used in `ModuleHost` tests.

use crate::ids::ModuleId;
use crate::module::{Availability, Module, ModuleStatus};
use crate::text::I18nText;

pub struct Placeholder {
    id: ModuleId,
}

impl Placeholder {
    pub fn new(id: ModuleId) -> Placeholder {
        Placeholder { id }
    }

    fn reason(&self) -> I18nText {
        I18nText::new("reason.coming_soon")
    }
}

impl Module for Placeholder {
    fn id(&self) -> ModuleId {
        self.id
    }
    fn availability(&self) -> Availability {
        Availability::Unsupported { reason: self.reason() }
    }
    fn enable(&mut self, _cfg: &toml::Table) -> Result<(), I18nText> {
        Err(self.reason())
    }
    fn apply(&mut self, _cfg: &toml::Table) -> Result<(), I18nText> {
        Ok(())
    }
    fn disable(&mut self) -> Result<(), I18nText> {
        Ok(())
    }
    fn restore_os_default(&mut self) -> Result<(), I18nText> {
        Ok(())
    }
    fn status(&self) -> ModuleStatus {
        ModuleStatus::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::Availability;

    fn reason_key(id: ModuleId) -> String {
        match Placeholder::new(id).availability() {
            Availability::Unsupported { reason } => reason.key,
            other => panic!("Placeholder phải là Unsupported, nhận {other:?}"),
        }
    }

    #[test]
    fn placeholder_reason() {
        for id in ModuleId::ALL {
            assert_eq!(reason_key(id), "reason.coming_soon", "{id}");
        }
    }

    #[test]
    fn placeholder_enable_fails() {
        let mut p = Placeholder::new(ModuleId::Cursor);
        assert!(p.enable(&toml::Table::new()).is_err());
        assert!(p.disable().is_ok());
    }
}
