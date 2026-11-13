//! Machine information for bug reports.

use crate::platform;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SysInfo {
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub gpus: Vec<String>,
    pub webview: Option<String>,
    pub locale: String,
    pub portable: bool,
}

pub fn collect(app_version: &str, webview: Option<String>, portable: bool) -> SysInfo {
    SysInfo {
        app_version: app_version.to_string(),
        os: platform::os_description(),
        arch: std::env::consts::ARCH.to_string(),
        gpus: platform::gpus(),
        webview,
        locale: crate::i18n::os_locale(),
        portable,
    }
}

impl SysInfo {
    pub fn to_text(&self) -> String {
        let gpus = if self.gpus.is_empty() { "unknown".to_string() } else { self.gpus.join("; ") };
        let mut lines = vec![
            format!("App: ShunguangDecor {}", self.app_version),
            format!("OS: {}", self.os),
            format!("Arch: {}", self.arch),
        ];
        lines.push(format!("GPU: {gpus}"));
        lines.push(format!("WebView: {}", self.webview.clone().unwrap_or_else(|| "unknown".into())));
        lines.push(format!("Locale: {}", if self.locale.is_empty() { "-" } else { &self.locale }));
        lines.push(format!("Portable: {}", if self.portable { "yes" } else { "no" }));
        lines.join("\n") + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysinfo_text_has_fields() {
        let s = collect("0.1.0", Some("130.0".into()), true);
        let t = s.to_text();
        for k in ["App:", "OS:", "Arch:", "Locale:", "Portable:", "WebView:"] {
            assert!(t.contains(k), "thiếu {k} trong\n{t}");
        }
        assert!(t.contains("0.1.0"));
    }
}
