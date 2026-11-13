//! Guesses the role of a cursor file from its name (used when the set has no `install.inf`).

use crate::role::Role;

/// `(keyword, role, whole-word match only)`. Short keywords match whole words only so that "no" does not match "normal".
const KEYWORDS: &[(&str, Role, bool)] = &[
    ("normal", Role::Arrow, false),
    ("arrow", Role::Arrow, false),
    ("pointer", Role::Arrow, false),
    ("help", Role::Help, false),
    ("working", Role::AppStarting, false),
    ("appstarting", Role::AppStarting, false),
    ("background", Role::AppStarting, false),
    ("busy", Role::Wait, false),
    ("wait", Role::Wait, false),
    ("precision", Role::Crosshair, false),
    ("cross", Role::Crosshair, false),
    ("text", Role::IBeam, false),
    ("ibeam", Role::IBeam, false),
    ("beam", Role::IBeam, false),
    ("handwriting", Role::NWPen, false),
    ("pen", Role::NWPen, true),
    ("unavailable", Role::No, false),
    ("no", Role::No, true),
    ("vertical", Role::SizeNS, false),
    ("vert", Role::SizeNS, false),
    ("ns", Role::SizeNS, true),
    ("horizontal", Role::SizeWE, false),
    ("horiz", Role::SizeWE, false),
    ("horz", Role::SizeWE, false),
    ("we", Role::SizeWE, true),
    ("nwse", Role::SizeNWSE, false),
    ("dgn1", Role::SizeNWSE, false),
    ("nesw", Role::SizeNESW, false),
    ("dgn2", Role::SizeNESW, false),
    ("sizeall", Role::SizeAll, false),
    ("move", Role::SizeAll, false),
    ("alternate", Role::UpArrow, false),
    ("uparrow", Role::UpArrow, false),
    ("up", Role::UpArrow, true),
    ("link", Role::Hand, false),
    ("hand", Role::Hand, false),
    ("location", Role::Pin, false),
    ("pin", Role::Pin, true),
    ("person", Role::Person, false),
];

pub fn guess_role(file_name: &str) -> Option<Role> {
    let base = file_name.rsplit(['\\', '/']).next().unwrap_or(file_name);
    let stem = match base.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s,
        _ => base,
    };
    let name = stem.to_ascii_lowercase();
    // "Diagonal Resize 1/2" (without "dgn"/"nwse"): 1 is NWSE, 2 is NESW.
    if name.contains("diag") {
        match name.chars().rev().find(char::is_ascii_digit) {
            Some('1') => return Some(Role::SizeNWSE),
            Some('2') => return Some(Role::SizeNESW),
            _ => {}
        }
    }
    let tokens: Vec<&str> = name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()).collect();
    let mut order: Vec<&(&str, Role, bool)> = KEYWORDS.iter().collect();
    order.sort_by_key(|(kw, _, _)| std::cmp::Reverse(kw.len()));
    order
        .into_iter()
        .find(|(kw, _, whole)| if *whole { tokens.contains(kw) } else { name.contains(kw) })
        .map(|(_, role, _)| *role)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guess_cases() {
        let cases = [
            ("Link Select.ani", Some(Role::Hand)),
            ("Diagonal Resize 2.cur", Some(Role::SizeNESW)),
            ("Diagonal Resize 1.cur", Some(Role::SizeNWSE)),
            ("Text.cur", Some(Role::IBeam)),
            ("readme.txt", None),
            ("Normal Select.ani", Some(Role::Arrow)),
            ("No.cur", Some(Role::No)),
            ("Unavailable.ani", Some(Role::No)),
            ("Handwriting.cur", Some(Role::NWPen)),
            ("Horizontal Resize.cur", Some(Role::SizeWE)),
            ("Vertical Resize.cur", Some(Role::SizeNS)),
            ("Working In Background.ani", Some(Role::AppStarting)),
            ("Busy.ani", Some(Role::Wait)),
            ("Alternate Select.cur", Some(Role::UpArrow)),
            ("Location Select.cur", Some(Role::Pin)),
            ("a/b\\Person Select.cur", Some(Role::Person)),
            ("Move.cur", Some(Role::SizeAll)),
            ("dgn2.cur", Some(Role::SizeNESW)),
        ];
        for (name, want) in cases {
            assert_eq!(guess_role(name), want, "{name}");
        }
    }
}
