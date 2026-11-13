// Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).
//! Wallpaper customization following the `LivelyProperties.json` standard (slider, checkbox, dropdown, color,
//! textbox, folderDropdown, button, label). The user's edited copy is saved at `properties\<id>.json`.

use serde_json::{Map, Value};
use std::path::Path;

use crate::config::WallpaperPaths;
use crate::library::Wallpaper;

pub type Props = Map<String, Value>;

fn user_path(paths: &WallpaperPaths, id: &str) -> std::path::PathBuf {
    paths.properties.join(format!("{id}.json"))
}

fn read_json(p: &Path) -> Option<Props> {
    let text = std::fs::read_to_string(p).ok()?;
    match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// The current set of customizations: the user's copy if present, otherwise the wallpaper's original.
pub fn load(paths: &WallpaperPaths, w: &Wallpaper) -> Option<Props> {
    let template = read_json(&w.properties_template()?)?;
    let Some(user) = read_json(&user_path(paths, &w.id)) else { return Some(template) };
    // Merge: keep the original's structure (in case the wallpaper is updated), take "value" from the user's copy.
    let mut merged = template;
    for (k, v) in merged.iter_mut() {
        if let (Some(uv), Value::Object(obj)) = (user.get(k).and_then(|u| u.get("value")), v) {
            obj.insert("value".into(), uv.clone());
        }
    }
    Some(merged)
}

pub fn reset(paths: &WallpaperPaths, id: &str) {
    let _ = std::fs::remove_file(user_path(paths, id));
}

fn control_type(v: &Value) -> &str {
    v.get("type").and_then(Value::as_str).unwrap_or("")
}

/// The value sent to the web page exactly the way Lively does it.
pub fn effective_value(w: &Wallpaper, control: &Value) -> Option<Value> {
    match control_type(control) {
        "button" | "label" => None,
        "folderDropdown" => {
            let folder = control.get("folder").and_then(Value::as_str);
            let file = control.get("value").and_then(Value::as_str);
            match (folder, file) {
                (Some(folder), Some(file)) => {
                    let rel = Path::new(folder).join(file);
                    if w.content_dir().join(&rel).exists() {
                        Some(Value::String(rel.to_string_lossy().into_owned()))
                    } else {
                        Some(Value::Null)
                    }
                }
                _ => Some(Value::Null),
            }
        }
        _ => control.get("value").cloned(),
    }
}

/// List of (name, value) pairs to send once the page has loaded.
pub fn startup_values(paths: &WallpaperPaths, w: &Wallpaper) -> Vec<(String, Value)> {
    let Some(props) = load(paths, w) else { return Vec::new() };
    props
        .iter()
        .filter_map(|(k, c)| effective_value(w, c).map(|v| (k.clone(), v)))
        .collect()
}

/// The user changes a customization. Returns the value to send to the page (None for a label).
pub fn set_value(paths: &WallpaperPaths, w: &Wallpaper, name: &str, value: Value) -> Result<Option<Value>, String> {
    let mut props = load(paths, w).ok_or("Hình nền này không có tuỳ chỉnh")?;
    let control = props.get_mut(name).ok_or_else(|| format!("Không có tuỳ chỉnh '{name}'"))?;
    if control_type(control) == "button" {
        // Button: Lively sends true, nothing is saved.
        return Ok(Some(Value::Bool(true)));
    }
    if let Value::Object(obj) = control {
        obj.insert("value".into(), value);
    }
    let out = effective_value(w, &props[name]);
    let _ = std::fs::create_dir_all(&paths.properties);
    std::fs::write(user_path(paths, &w.id), serde_json::to_string_pretty(&props).map_err(|e| e.to_string())?)
        .map_err(|e| format!("Không lưu được tuỳ chỉnh: {e}"))?;
    Ok(out)
}

/// List of files for folderDropdown (so the control panel can show the choices).
pub fn folder_items(w: &Wallpaper, control: &Value) -> Vec<String> {
    let Some(folder) = control.get("folder").and_then(Value::as_str) else { return Vec::new() };
    let filters: Vec<String> = control
        .get("filter")
        .and_then(Value::as_str)
        .unwrap_or("*.*")
        .split('|')
        .map(|f| f.trim().trim_start_matches('*').to_lowercase())
        .collect();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(w.content_dir().join(folder)) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            let ok = filters.iter().any(|f| f.is_empty() || f == "." || f == ".*" || lower.ends_with(f.as_str()));
            if e.path().is_file() && ok {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{WallpaperInfo, WallpaperType};
    use serde_json::json;

    fn setup(d: &Path) -> (WallpaperPaths, Wallpaper) {
        let paths = WallpaperPaths {
            library: d.join("lib"),
            properties: d.join("props"),
            thumbs: d.join("thumbs"),
            webview: d.join("web"),
            backup_file: d.join("b.toml"),
        };
        let dir = paths.library.join("w1");
        std::fs::create_dir_all(dir.join("img")).unwrap();
        std::fs::write(dir.join("img").join("a.png"), b"x").unwrap();
        std::fs::write(
            dir.join("LivelyProperties.json"),
            json!({
                "speed": {"type": "slider", "value": 5, "min": 0, "max": 10},
                "pic": {"type": "folderDropdown", "folder": "img", "value": "a.png"},
                "gone": {"type": "folderDropdown", "folder": "img", "value": "zzz.png"},
                "btn": {"type": "button"},
                "lbl": {"type": "label"}
            })
            .to_string(),
        )
        .unwrap();
        let w = Wallpaper { id: "w1".into(), dir, info: WallpaperInfo::new(WallpaperType::Web, "W", "index.html", false) };
        (paths, w)
    }

    #[test]
    fn properties_effective_value() {
        let d = tempfile::tempdir().unwrap();
        let (paths, w) = setup(d.path());
        let vals: std::collections::BTreeMap<_, _> = startup_values(&paths, &w).into_iter().collect();
        assert_eq!(vals["speed"], json!(5));
        assert_eq!(vals["pic"], json!(Path::new("img").join("a.png").to_string_lossy()));
        assert_eq!(vals["gone"], Value::Null, "file không còn → null");
        assert!(!vals.contains_key("btn") && !vals.contains_key("lbl"));
        // The user changes it → the user's copy wins; back to the default → returns to the original.
        assert_eq!(set_value(&paths, &w, "speed", json!(9)).unwrap(), Some(json!(9)));
        assert_eq!(startup_values(&paths, &w).into_iter().find(|(k, _)| k == "speed").unwrap().1, json!(9));
        assert_eq!(set_value(&paths, &w, "btn", json!(1)).unwrap(), Some(json!(true)));
        assert!(set_value(&paths, &w, "nope", json!(1)).is_err());
        reset(&paths, "w1");
        assert_eq!(startup_values(&paths, &w).into_iter().find(|(k, _)| k == "speed").unwrap().1, json!(5));
    }
}
