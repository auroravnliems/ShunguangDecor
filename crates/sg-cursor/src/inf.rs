//! Reads the `install.inf` of a cursor scheme. Only the scheme name and the file name of each role are taken; every other
//! directive (for example `HKLM ... Runonce`) is ignored completely.

use crate::role::Role;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InfScheme {
    pub name: Option<String>,
    /// File name only, no path.
    pub roles: BTreeMap<Role, String>,
}

/// UTF-16LE/BE BOM, UTF-8 BOM, valid UTF-8; anything else is read as Windows-1252.
pub fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, u16::from_be_bytes);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
    }
}

fn utf16(bytes: &[u8], conv: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| conv(*c)).collect();
    String::from_utf16_lossy(&units)
}

fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘', '’',
        '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9F => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

/// Strips a `;` comment that lies outside double quotes.
fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ';' if !in_quotes => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Splits on commas outside double quotes and drops the quotes surrounding each field.
fn split_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for c in line.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => out.push(std::mem::take(&mut cur).trim().to_string()),
            _ => cur.push(c),
        }
    }
    out.push(cur.trim().to_string());
    out
}

/// `key = "value"` or `key = value`.
fn parse_kv(line: &str) -> Option<(String, String)> {
    let (k, v) = line.split_once('=')?;
    let v = v.trim();
    let v = match v.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(""),
        None => v,
    };
    Some((k.trim().to_ascii_lowercase(), v.to_string()))
}

/// Takes the file name at the end of the path, then substitutes `%variables%`; an unknown variable gives `None`.
fn file_name_of(raw: &str, vars: &HashMap<String, String>) -> Option<String> {
    let last = raw.rsplit(['\\', '/']).next().unwrap_or(raw).trim();
    let resolved = substitute(last, vars)?;
    let name = resolved.rsplit(['\\', '/']).next().unwrap_or(&resolved).trim().to_string();
    (!name.is_empty()).then_some(name)
}

fn substitute(s: &str, vars: &HashMap<String, String>) -> Option<String> {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find('%')?;
        out.push_str(vars.get(&after[..end].to_ascii_lowercase())?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

pub fn parse_inf(text: &str) -> InfScheme {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut vars: HashMap<String, String> = HashMap::new();
    let mut section = String::new();
    let mut reg_lines: Vec<Vec<String>> = Vec::new();
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = line.trim_matches(['[', ']']).trim().to_ascii_lowercase();
            continue;
        }
        if section.starts_with("strings") {
            if let Some((k, v)) = parse_kv(line) {
                vars.entry(k).or_insert(v);
            }
        } else if line.len() >= 5 && line[..5].eq_ignore_ascii_case("HKCU,") {
            reg_lines.push(split_fields(line));
        }
    }

    let mut scheme = InfScheme { name: vars.get("scheme_name").filter(|n| !n.trim().is_empty()).cloned(), roles: BTreeMap::new() };
    let mut schemes_value: Option<(String, String)> = None;
    for f in &reg_lines {
        let key = f.get(1).map(|k| k.to_ascii_lowercase()).unwrap_or_default();
        let key = key.trim_end_matches('\\');
        if key == r"control panel\cursors" {
            let (Some(name), Some(value)) = (f.get(2), f.get(4)) else { continue };
            if let Some(role) = Role::parse(name)
                && let Some(file) = file_name_of(value, &vars)
            {
                scheme.roles.entry(role).or_insert(file);
            }
        } else if key == r"control panel\cursors\schemes"
            && let (Some(name), Some(value)) = (f.get(2), f.get(4))
        {
            schemes_value.get_or_insert((name.clone(), value.clone()));
        }
    }
    if let Some((value_name, list)) = schemes_value {
        if scheme.name.is_none() {
            scheme.name = substitute(&value_name, &vars).filter(|n| !n.trim().is_empty());
        }
        if scheme.roles.is_empty() {
            for (role, item) in Role::ALL.into_iter().zip(list.split(',')) {
                if item.trim().is_empty() {
                    continue;
                }
                if let Some(file) = file_name_of(item.trim(), &vars) {
                    scheme.roles.insert(role, file);
                }
            }
        }
    }
    scheme
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLZ: &str = r#"
; Cursor pack
[Version]
Signature="$CHICAGO$"

[DefaultInstall]
CopyFiles = Scheme.Cur
AddReg    = Scheme.Reg,Wreg

[Scheme.Reg]
HKCU,"Control Panel\Cursors",,0x00020000,"%SCHEME_NAME%"
HKCU,"Control Panel\Cursors","Arrow",0x00020000,"%10%\%CUR_DIR%\%pointer%"
HKCU,"Control Panel\Cursors","Help",0x00020000,"%10%\%CUR_DIR%\%help%"
HKCU,"Control Panel\Cursors","AppStarting",0x00020000,"%10%\%CUR_DIR%\%work%"
HKCU,"Control Panel\Cursors","Wait",0x00020000,"%10%\%CUR_DIR%\%busy%"
HKCU,"Control Panel\Cursors","Crosshair",0x00020000,"%10%\%CUR_DIR%\%cross%"
HKCU,"Control Panel\Cursors","IBeam",0x00020000,"%10%\%CUR_DIR%\%text%"
HKCU,"Control Panel\Cursors","NWPen",0x00020000,"%10%\%CUR_DIR%\%hand%"
HKCU,"Control Panel\Cursors","No",0x00020000,"%10%\%CUR_DIR%\%unavail%"
HKCU,"Control Panel\Cursors","SizeNS",0x00020000,"%10%\%CUR_DIR%\%vert%"
HKCU,"Control Panel\Cursors","SizeWE",0x00020000,"%10%\%CUR_DIR%\%horz%"
HKCU,"Control Panel\Cursors","SizeNWSE",0x00020000,"%10%\%CUR_DIR%\%dgn1%"
HKCU,"Control Panel\Cursors","SizeNESW",0x00020000,"%10%\%CUR_DIR%\%dgn2%"
HKCU,"Control Panel\Cursors","SizeAll",0x00020000,"%10%\%CUR_DIR%\%move%"
HKCU,"Control Panel\Cursors","UpArrow",0x00020000,"%10%\%CUR_DIR%\%alt%"
HKCU,"Control Panel\Cursors","Hand",0x00020000,"%10%\%CUR_DIR%\%link%"
HKCU,"Control Panel\Cursors","Pin",0x00020000,"%10%\%CUR_DIR%\%pin%"
HKCU,"Control Panel\Cursors","Person",0x00020000,"%10%\%CUR_DIR%\%person%"
HKCU,"Control Panel\Cursors\Schemes","%SCHEME_NAME%",0x00020000,"%10%\%CUR_DIR%\%pointer%,%10%\%CUR_DIR%\%help%"

[Wreg]
HKLM,"SOFTWARE\Microsoft\Windows\CurrentVersion\Runonce","Evil",0,"cmd /c calc.exe"
HKLM,"Control Panel\Cursors","Arrow",0,"C:\evil.ani"

[Strings]
CUR_DIR = "Cursors\BLZ"
SCHEME_NAME = "BLZ Cursor"
pointer = "Normal Select.ani"
help = "Help Select.ani"
work = "Working.ani"
busy = "Busy.ani"
cross = "Precision.cur"
text = "Text.cur"
hand = "Handwriting.cur"
unavail = "Unavailable.cur"
vert = "Vertical.cur"
horz = "Horizontal.cur"
dgn1 = "Diagonal 1.cur"
dgn2 = "Diagonal 2.cur"
move = "Move.cur"
alt = "Alternate.cur"
link = "Link.ani"
pin = "Pin.cur"
person = "Person.cur"
"#;

    #[test]
    fn inf_blz_style() {
        let s = parse_inf(BLZ);
        assert_eq!(s.name.as_deref(), Some("BLZ Cursor"));
        assert_eq!(s.roles.len(), 17);
        assert_eq!(s.roles[&Role::Arrow], "Normal Select.ani");
        assert_eq!(s.roles[&Role::SizeNESW], "Diagonal 2.cur");
        assert_eq!(s.roles[&Role::Person], "Person.cur");
        assert!(s.roles.values().all(|f| !f.to_ascii_lowercase().contains("evil")));
    }

    #[test]
    fn inf_utf16_bom() {
        let mut bytes = vec![0xFF, 0xFE];
        for u in BLZ.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let s = parse_inf(&decode_text(&bytes));
        assert_eq!(s.roles.len(), 17);
        assert_eq!(s.name.as_deref(), Some("BLZ Cursor"));
        // UTF-16BE, UTF-8 BOM, and Windows-1252.
        let mut be = vec![0xFE, 0xFF];
        for u in "[Strings]\nA=\"é\"".encode_utf16() {
            be.extend_from_slice(&u.to_be_bytes());
        }
        assert!(decode_text(&be).contains('é'));
        assert_eq!(decode_text(b"\xEF\xBB\xBFabc"), "abc");
        assert_eq!(decode_text(b"caf\xE9"), "café");
    }

    #[test]
    fn inf_schemes_line_only() {
        let names = ["a.ani", "b.cur", "", "d.cur"];
        let list = names.iter().map(|n| if n.is_empty() { String::new() } else { format!("%10%\\Cursors\\X\\%{}%", n.replace('.', "_")) }).collect::<Vec<_>>().join(",");
        let mut text = String::from("[Strings]\nSCHEME_NAME=\"Only List\"\n");
        for n in names.iter().filter(|n| !n.is_empty()) {
            text.push_str(&format!("{} = \"{}\"\n", n.replace('.', "_"), n));
        }
        let text = format!("[Scheme.Reg]\nHKCU,\"Control Panel\\Cursors\\Schemes\",\"%SCHEME_NAME%\",0x00020000,\"{list}\"\n{text}");
        let s = parse_inf(&text);
        assert_eq!(s.name.as_deref(), Some("Only List"));
        assert_eq!(s.roles.len(), 3);
        assert_eq!(s.roles[&Role::Arrow], "a.ani");
        assert_eq!(s.roles[&Role::Help], "b.cur");
        assert_eq!(s.roles[&Role::Wait], "d.cur");
    }

    #[test]
    fn inf_unquoted_strings() {
        let text = "[Strings]\nscheme_name = My Set\nArrowFile = Arrow.cur\n[X]\nHKCU,\"Control Panel\\Cursors\",Arrow,0x20000,\"%10%\\%ArrowFile%\"\n";
        let s = parse_inf(text);
        assert_eq!(s.name.as_deref(), Some("My Set"));
        assert_eq!(s.roles[&Role::Arrow], "Arrow.cur");
    }

    #[test]
    fn inf_missing_var_skips_role() {
        let text = "[Strings]\nA = \"a.cur\"\n[X]\nHKCU,\"Control Panel\\Cursors\",Arrow,0x20000,\"%10%\\%A%\"\nHKCU,\"Control Panel\\Cursors\",Help,0x20000,\"%10%\\%B%\"\n";
        let s = parse_inf(text);
        assert_eq!(s.roles.len(), 1);
        assert!(s.roles.contains_key(&Role::Arrow));
    }

    #[test]
    fn inf_garbage_is_empty() {
        assert_eq!(parse_inf("\u{0}\u{1}%%%\n[[["), InfScheme::default());
    }
}
