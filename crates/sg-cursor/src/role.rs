//! The 17 Windows cursor roles, in the same order as in the registry/`install.inf`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Role {
    Arrow,
    Help,
    AppStarting,
    Wait,
    Crosshair,
    IBeam,
    NWPen,
    No,
    SizeNS,
    SizeWE,
    SizeNWSE,
    SizeNESW,
    SizeAll,
    UpArrow,
    Hand,
    Pin,
    Person,
}

impl Role {
    pub const ALL: [Role; 17] = [
        Role::Arrow,
        Role::Help,
        Role::AppStarting,
        Role::Wait,
        Role::Crosshair,
        Role::IBeam,
        Role::NWPen,
        Role::No,
        Role::SizeNS,
        Role::SizeWE,
        Role::SizeNWSE,
        Role::SizeNESW,
        Role::SizeAll,
        Role::UpArrow,
        Role::Hand,
        Role::Pin,
        Role::Person,
    ];

    /// Value name in `HKCU\Control Panel\Cursors`.
    pub fn reg_name(self) -> &'static str {
        match self {
            Role::Arrow => "Arrow",
            Role::Help => "Help",
            Role::AppStarting => "AppStarting",
            Role::Wait => "Wait",
            Role::Crosshair => "Crosshair",
            Role::IBeam => "IBeam",
            Role::NWPen => "NWPen",
            Role::No => "No",
            Role::SizeNS => "SizeNS",
            Role::SizeWE => "SizeWE",
            Role::SizeNWSE => "SizeNWSE",
            Role::SizeNESW => "SizeNESW",
            Role::SizeAll => "SizeAll",
            Role::UpArrow => "UpArrow",
            Role::Hand => "Hand",
            Role::Pin => "Pin",
            Role::Person => "Person",
        }
    }

    /// Case-insensitive.
    pub fn parse(s: &str) -> Option<Role> {
        let s = s.trim();
        Role::ALL.into_iter().find(|r| r.reg_name().eq_ignore_ascii_case(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_order_and_names() {
        assert_eq!(Role::ALL.len(), 17);
        assert_eq!(Role::ALL[10].reg_name(), "SizeNWSE");
        assert_eq!(Role::parse("ibeam"), Some(Role::IBeam));
        assert_eq!(Role::parse("nope"), None);
        // The `Ord` order matches the ALL order (so BTreeMap sorts correctly).
        let mut sorted = Role::ALL;
        sorted.sort();
        assert_eq!(sorted, Role::ALL);
        for r in Role::ALL {
            assert_eq!(Role::parse(r.reg_name()), Some(r));
        }
    }
}
