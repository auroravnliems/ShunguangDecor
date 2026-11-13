// Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).
//! Web player using WebView2 (the Edge Chromium runtime built into Windows 10/11).
//! Supports Lively's custom API: `livelyPropertyListener(name, value)` and
//! `livelyWallpaperPlaybackChanged(json)`.

use std::path::{Path, PathBuf};

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2_3, ICoreWebView2_8, COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW,
};
use windows::core::Interface;
use windows::Win32::Foundation::{HWND, RECT};
use wry::{MemoryUsageLevel, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder, WebViewExtWindows};

use super::host::{self, HwndRef};
use super::{rect_size, PauseReason, Player, EV_WEB_LOADED};
use sg_desktop::DesktopLayer as Desktop;
use crate::config::Scaling;
use crate::util::wide;

/// Virtual host name mapped to the wallpaper folder (WebView2 serves the files itself, which supports video seeking).
pub const VIRTUAL_HOST: &str = "wallpaper.sgdecor.example";

pub enum WebContent {
    /// A web page on the network.
    Url(String),
    /// An HTML file inside the wallpaper folder.
    Local { dir: PathBuf, file: String },
    /// A pre-built HTML page (used as a fallback for video/GIF/images), with resources in `dir`.
    Generated { dir: PathBuf, html: String },
}

pub struct WebPlayer {
    host: HWND,
    webview: Option<WebView>,
    properties: Vec<(String, serde_json::Value)>,
    paused: Option<PauseReason>,
    loaded: bool,
}

pub struct WebOptions<'a> {
    pub context: &'a mut WebContext,
    pub volume: u8,
    pub devtools: bool,
    pub properties: Vec<(String, serde_json::Value)>,
}

impl WebPlayer {
    pub fn new(desktop: &Desktop, rect: RECT, content: WebContent, opts: WebOptions) -> Result<WebPlayer, String> {
        let host = host::create(desktop, rect)?;
        let host_id = host.0 as isize;
        let (w, h) = rect_size(&rect);
        let builder = WebViewBuilder::new_with_web_context(opts.context)
            .with_bounds(Rect {
                position: dpi::PhysicalPosition::new(0, 0).into(),
                size: dpi::PhysicalSize::new(w.max(1) as u32, h.max(1) as u32).into(),
            })
            .with_background_color((0, 0, 0, 255))
            .with_autoplay(true)
            .with_devtools(opts.devtools)
            .with_focused(false)
            .with_on_page_load_handler(move |ev, url| {
                if matches!(ev, PageLoadEvent::Finished) && !url.starts_with("about:") {
                    crate::engine::post_player_event(host_id, EV_WEB_LOADED, 0);
                }
            });

        let webview = match builder.build_as_child(&HwndRef(host)) {
            Ok(v) => v,
            Err(e) => {
                host::destroy(host);
                return Err(format!("Không tạo được WebView2 (đã cài WebView2 Runtime chưa?): {e}"));
            }
        };

        let core = webview.webview();
        unsafe {
            if let Ok(c8) = core.cast::<ICoreWebView2_8>() {
                let _ = c8.SetIsMuted(opts.volume == 0);
            }
        }

        let map_folder = |dir: &Path| unsafe {
            if let Ok(c3) = core.cast::<ICoreWebView2_3>() {
                let h = wide(VIRTUAL_HOST);
                let d = wide(dir.as_os_str());
                if let Err(e) =
                    c3.SetVirtualHostNameToFolderMapping(h.pcwstr(), d.pcwstr(), COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_ALLOW)
                {
                    logln!("Không ánh xạ được thư mục {}: {e}", dir.display());
                }
            }
        };

        let nav = match &content {
            WebContent::Url(u) => webview.load_url(u),
            WebContent::Local { dir, file } => {
                map_folder(dir);
                webview.load_url(&format!("https://{VIRTUAL_HOST}/{}", encode_path(file)))
            }
            WebContent::Generated { dir, html } => {
                map_folder(dir);
                webview.load_html(html)
            }
        };
        if let Err(e) = nav {
            logln!("WebView2 không mở được trang: {e}");
        }

        Ok(WebPlayer { host, webview: Some(webview), properties: opts.properties, paused: None, loaded: false })
    }

    fn eval(&self, js: &str) {
        if let Some(w) = &self.webview {
            if let Err(e) = w.evaluate_script(js) {
                logln!("evaluate_script lỗi: {e}");
            }
        }
    }

    fn send_property(&self, name: &str, value: &serde_json::Value) {
        let js = format!(
            "try{{if(typeof livelyPropertyListener==='function')livelyPropertyListener({},{});}}catch(e){{console.error(e)}}",
            serde_json::Value::String(name.to_string()),
            value
        );
        self.eval(&js);
    }

    fn send_playback(&self, paused: bool) {
        let state = serde_json::json!({ "IsPaused": paused }).to_string();
        self.eval(&format!(
            "try{{if(typeof livelyWallpaperPlaybackChanged==='function')livelyWallpaperPlaybackChanged({});}}catch(e){{}}",
            serde_json::Value::String(state)
        ));
    }
}

const PAUSE_MEDIA_JS: &str = "document.querySelectorAll('video,audio').forEach(function(m){if(!m.paused){m.dataset.tachyonPaused='1';m.pause();}});";
const RESUME_MEDIA_JS: &str = "document.querySelectorAll('video,audio').forEach(function(m){if(m.dataset.tachyonPaused){delete m.dataset.tachyonPaused;m.play().catch(function(){});}});";

impl Player for WebPlayer {
    fn backend(&self) -> &'static str {
        "webview2"
    }

    fn host(&self) -> Option<HWND> {
        Some(self.host)
    }

    fn set_paused(&mut self, paused: bool, reason: PauseReason) {
        let want = paused.then_some(reason);
        if want == self.paused {
            return;
        }
        let Some(w) = &self.webview else { return };
        // Undo the previous state.
        if let Some(prev) = self.paused {
            if prev == PauseReason::Covered {
                let _ = w.set_visible(true);
                let _ = w.set_memory_usage_level(MemoryUsageLevel::Normal);
            }
            self.eval(RESUME_MEDIA_JS);
        }
        match want {
            Some(PauseReason::Covered) => {
                // Fully covered: hide it entirely so WebView2 stops rendering and uses less RAM.
                self.eval(PAUSE_MEDIA_JS);
                let _ = w.set_visible(false);
                let _ = w.set_memory_usage_level(MemoryUsageLevel::Low);
            }
            Some(PauseReason::System) => self.eval(PAUSE_MEDIA_JS),
            None => {}
        }
        self.send_playback(paused);
        self.paused = want;
    }

    fn set_volume(&mut self, volume: u8) {
        if let Some(w) = &self.webview {
            unsafe {
                if let Ok(c8) = w.webview().cast::<ICoreWebView2_8>() {
                    let _ = c8.SetIsMuted(volume == 0);
                }
            }
        }
        self.eval(&format!(
            "document.querySelectorAll('video,audio').forEach(function(m){{m.volume={};m.muted={};}});",
            volume as f32 / 100.0,
            volume == 0
        ));
    }

    fn resize(&mut self, rect: RECT) {
        host::move_to(self.host, rect);
        let (w, h) = rect_size(&rect);
        if let Some(v) = &self.webview {
            let _ = v.set_bounds(Rect {
                position: dpi::PhysicalPosition::new(0, 0).into(),
                size: dpi::PhysicalSize::new(w.max(1) as u32, h.max(1) as u32).into(),
            });
        }
    }

    fn apply_property(&mut self, name: &str, value: &serde_json::Value) {
        if let Some(p) = self.properties.iter_mut().find(|(n, _)| n == name) {
            p.1 = value.clone();
        } else {
            self.properties.push((name.to_string(), value.clone()));
        }
        if self.loaded {
            self.send_property(name, value);
        }
    }

    fn on_event(&mut self, code: u32, _param: isize) -> Result<(), String> {
        if code == EV_WEB_LOADED {
            self.loaded = true;
            for (n, v) in &self.properties {
                self.send_property(n, v);
            }
            if let Some(reason) = self.paused.take() {
                // The page just reloaded while paused: re-apply the paused state.
                self.set_paused(true, reason);
            }
        }
        Ok(())
    }

    fn open_devtools(&self) {
        if let Some(w) = &self.webview {
            w.open_devtools();
        }
    }

    fn close(&mut self) {
        self.webview = None;
        host::destroy(self.host);
    }
}

/// Encodes a relative path for use in a URL (keeping the '/' separators).
pub fn encode_path(rel: &str) -> String {
    let mut out = String::new();
    for b in rel.replace('\\', "/").bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// An HTML page that shows a video/GIF/image full screen (used when no suitable native player exists).
pub fn media_page(file_name: &str, video: bool, scaling: Scaling, volume: u8) -> String {
    let fit = match scaling {
        Scaling::Fill => "cover",
        Scaling::Fit => "contain",
        Scaling::Stretch => "fill",
    };
    let src = format!("https://{VIRTUAL_HOST}/{}", encode_path(file_name));
    let style = format!(
        "html,body{{margin:0;height:100%;background:#000;overflow:hidden}}#m{{width:100vw;height:100vh;object-fit:{fit};display:block}}"
    );
    let body = if video {
        format!(
            "<video id=\"m\" src=\"{src}\" autoplay loop playsinline {}></video><script>var v=document.getElementById('m');v.volume={};</script>",
            if volume == 0 { "muted" } else { "" },
            volume as f32 / 100.0
        )
    } else {
        format!("<img id=\"m\" src=\"{src}\" alt=\"\">")
    };
    format!("<!doctype html><html><head><meta charset=\"utf-8\"><style>{style}</style></head><body>{body}</body></html>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_relative_paths() {
        assert_eq!(encode_path(r"sub dir\a b.html"), "sub%20dir/a%20b.html");
        assert_eq!(encode_path("ảnh.gif"), "%E1%BA%A3nh.gif");
    }

    #[test]
    fn media_page_uses_virtual_host() {
        let html = media_page("clip.mp4", true, Scaling::Fill, 0);
        assert!(html.contains("object-fit:cover"));
        assert!(html.contains("muted"));
        assert!(html.contains(VIRTUAL_HOST));
    }
}
