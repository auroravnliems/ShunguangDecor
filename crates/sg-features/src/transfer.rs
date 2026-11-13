//! Export/import of the app's configuration: `{ "format": 1, "runtime": [...], "boot": [...] }`.
//! (ShunguangDecor's own format; not a ViVeTool export file.)

use serde::Serialize;
use serde_json::Value;

use crate::model::{FeatureConfig, MAX_VARIANT, Store};

pub const FORMAT: u32 = 1;

#[derive(Serialize)]
struct Export<'a> {
    format: u32,
    runtime: &'a [FeatureConfig],
    boot: &'a [FeatureConfig],
}

/// Indented JSON for the two stores.
pub fn export(runtime: &[FeatureConfig], boot: &[FeatureConfig]) -> String {
    serde_json::to_string_pretty(&Export {
        format: FORMAT,
        runtime,
        boot,
    })
    .unwrap_or_default()
}

/// Reads an import file. Bad entries (ID 0, variant > 63, unknown priority, wrong shape) are dropped with a warning;
/// the valid part is returned. If everything is broken, the list is empty.
pub fn parse_import(text: &str) -> (Vec<(Store, FeatureConfig)>, Vec<String>) {
    let mut out = Vec::new();
    let mut warns = Vec::new();
    let root: Value = match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
        Ok(v) => v,
        Err(e) => {
            warns.push(format!("Tệp không phải JSON hợp lệ: {e}"));
            return (out, warns);
        }
    };
    let Some(obj) = root.as_object() else {
        warns.push("Tệp phải là một đối tượng JSON.".into());
        return (out, warns);
    };
    if obj.get("format").and_then(Value::as_u64) != Some(u64::from(FORMAT)) {
        warns.push(format!(
            "Tệp không đúng định dạng (cần \"format\": {FORMAT})."
        ));
        return (out, warns);
    }
    for (key, store) in [("runtime", Store::Runtime), ("boot", Store::Boot)] {
        let Some(section) = obj.get(key) else {
            continue;
        };
        let Some(rows) = section.as_array() else {
            warns.push(format!("Mục \"{key}\" phải là danh sách."));
            continue;
        };
        for (i, row) in rows.iter().enumerate() {
            let at = i + 1;
            match serde_json::from_value::<FeatureConfig>(row.clone()) {
                Err(e) => warns.push(format!("{key} #{at}: bỏ qua, sai dạng ({e}).")),
                Ok(c) if c.id == 0 => {
                    warns.push(format!("{key} #{at}: bỏ qua, ID không được bằng 0."))
                }
                Ok(c) if c.variant > MAX_VARIANT => warns.push(format!(
                    "{key} #{at} (ID {}): bỏ qua, variant {} vượt quá {MAX_VARIANT}.",
                    c.id, c.variant
                )),
                Ok(c) => out.push((store, c)),
            }
        }
    }
    (out, warns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EnabledState, PayloadKind, Priority};

    fn c(id: u32, state: EnabledState, variant: u8) -> FeatureConfig {
        FeatureConfig {
            id,
            priority: Priority::User,
            state,
            variant,
            variant_payload_kind: PayloadKind::Resident,
            variant_payload: 77,
            wexp: false,
        }
    }

    #[test]
    fn export_roundtrip() {
        let rt = vec![
            c(1, EnabledState::Enabled, 0),
            c(2, EnabledState::Disabled, 63),
        ];
        let boot = vec![c(3, EnabledState::Default, 5)];
        let text = export(&rt, &boot);
        assert!(text.contains("\"user\""));
        let (rows, warns) = parse_import(&text);
        assert!(warns.is_empty(), "{warns:?}");
        let expect: Vec<_> = rt
            .iter()
            .map(|x| (Store::Runtime, *x))
            .chain(boot.iter().map(|x| (Store::Boot, *x)))
            .collect();
        assert_eq!(rows, expect);
    }

    #[test]
    fn import_parse_skips_bad_rows() {
        let text = r#"{
          "format": 1,
          "runtime": [
            {"id": 5, "priority": "user", "state": "enabled"},
            {"id": 0, "priority": "user", "state": "enabled"},
            {"id": 6, "priority": "user", "state": "enabled", "variant": 64},
            {"id": 7, "priority": "khong-co", "state": "enabled"},
            {"id": 8, "priority": "user"},
            "rác"
          ],
          "boot": 12
        }"#;
        let (rows, warns) = parse_import(text);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, Store::Runtime);
        assert_eq!(rows[0].1.id, 5);
        assert_eq!(rows[0].1.variant, 0);
        assert_eq!(warns.len(), 6, "{warns:?}");
    }

    #[test]
    fn import_rejects_wrong_files() {
        for t in [
            "",
            "không phải json",
            "[]",
            r#"{"format": 2}"#,
            r#"{"runtime": []}"#,
        ] {
            let (rows, warns) = parse_import(t);
            assert!(rows.is_empty(), "{t}");
            assert_eq!(warns.len(), 1, "{t}");
        }
    }
}
