//! The list of system font families for the font picker on the Taskbar page (DirectWrite, exactly the collection the engine uses
//! to draw text, so the chosen name always matches). Not Windows → an empty list.

/// Sorts case-insensitively, dropping duplicates and empty names. Pure: has tests.
pub fn normalize(mut names: Vec<String>) -> Vec<String> {
    for n in &mut names {
        *n = n.trim().to_string();
    }
    names.retain(|n| !n.is_empty());
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup_by(|a, b| a.to_lowercase() == b.to_lowercase());
    names
}

/// Font families installed on the machine, named in the app's language (`lang_code`, e.g. `vi`), falling back to `en-us`, then to the first name.
pub fn list_system_fonts(lang_code: &str) -> Vec<String> {
    match win::list(lang_code) {
        Ok(v) => normalize(v),
        Err(e) => {
            tracing::warn!(target: "sg::taskbar", "không liệt kê được font hệ thống: {e}");
            Vec::new()
        }
    }
}

mod win {
    use windows::Win32::Graphics::DirectWrite::{
        DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_FAMILY_MODEL_TYPOGRAPHIC, DWriteCreateFactory, IDWriteFactory2, IDWriteFactory6,
        IDWriteFontCollection, IDWriteLocalizedStrings,
    };
    use windows::core::{BOOL, HSTRING, Interface};

    fn find(names: &IDWriteLocalizedStrings, locale: &str) -> Option<u32> {
        let mut index = 0u32;
        let mut exists = BOOL(0);
        // SAFETY: the out parameter is valid for the whole call.
        unsafe { names.FindLocaleName(&HSTRING::from(locale), &mut index, &mut exists) }.ok()?;
        exists.as_bool().then_some(index)
    }

    fn text(names: &IDWriteLocalizedStrings, index: u32) -> windows::core::Result<String> {
        // SAFETY: `buf` has room for the string and the terminator according to `GetStringLength`.
        unsafe {
            let len = names.GetStringLength(index)? as usize;
            let mut buf = vec![0u16; len + 1];
            names.GetString(index, &mut buf)?;
            Ok(String::from_utf16_lossy(&buf[..len]))
        }
    }

    pub fn list(lang_code: &str) -> windows::core::Result<Vec<String>> {
        // SAFETY: read-only DirectWrite calls; every object lives within this function.
        unsafe {
            let dwrite: IDWriteFactory2 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let collection: IDWriteFontCollection = match dwrite.cast::<IDWriteFactory6>() {
                Ok(f6) => f6.GetSystemFontCollection(false, DWRITE_FONT_FAMILY_MODEL_TYPOGRAPHIC)?.cast()?,
                Err(_) => {
                    let mut c = None;
                    dwrite.GetSystemFontCollection(&mut c, false)?;
                    c.ok_or_else(windows::core::Error::empty)?
                }
            };
            let mut out = Vec::new();
            for i in 0..collection.GetFontFamilyCount() {
                let Ok(family) = collection.GetFontFamily(i) else { continue };
                let Ok(names) = family.GetFamilyNames() else { continue };
                let index = find(&names, lang_code).or_else(|| find(&names, "en-us")).unwrap_or(0);
                if let Ok(name) = text(&names, index) {
                    out.push(name);
                }
            }
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_sorts_dedupes_and_drops_empty() {
        let v = ["Segoe UI", "arial", "Arial", " ", "Calibri", "segoe ui", ""].map(String::from).to_vec();
        assert_eq!(normalize(v), vec!["arial".to_string(), "Calibri".into(), "Segoe UI".into()]);
    }

    #[test]
    fn real_system_fonts_include_segoe_ui() {
        let v = list_system_fonts("en");
        assert!(v.iter().any(|f| f.eq_ignore_ascii_case("Segoe UI")), "got {} fonts", v.len());
        assert!(v.windows(2).all(|w| w[0].to_lowercase() <= w[1].to_lowercase()));
    }
}
