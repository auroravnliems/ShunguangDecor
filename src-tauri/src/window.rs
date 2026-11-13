//! The UI window: created only when opened and destroyed when closed, so background running does not hold the WebView's RAM.

use crate::state::{AppState, lock};
use std::sync::atomic::Ordering;
use tauri::window::Color;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window, WindowEvent};

pub const MAIN: &str = "main";
pub const WEBVIEW2_URL: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";

/// Opens the window (creating it if it does not exist). Call on the main thread (tray event, single-instance, setup).
pub fn open_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    if tauri::webview_version().is_err() {
        show_webview2_missing(app);
        return;
    }
    let built = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("ShunguangDecor")
        .inner_size(1280.0, 760.0)
        .min_inner_size(1100.0, 650.0)
        .decorations(false)
        .shadow(true)
        .resizable(true)
        .center()
        .background_color(Color(0xf4, 0xf6, 0xf9, 0xff))
        .build();
    match built {
        Ok(w) => {
            square_corners(&w);
            let _ = w.set_focus();
            tracing::info!(target: "sg::app", "window opened");
        }
        Err(e) => tracing::error!(target: "sg::app", "cannot create window: {e}"),
    }
}

/// If the window is open, sends the event to it; otherwise does nothing.
pub fn emit_main<S: serde::Serialize + Clone>(app: &AppHandle, event: &str, payload: S) {
    if app.get_webview_window(MAIN).is_some() {
        let _ = app.emit_to(MAIN, event, payload);
    }
}

/// The first time the window is closed: ask the UI to show the "still running in the tray" notice. A second close click in the session
/// closes immediately, so even a hung UI cannot trap the window.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if window.label() != MAIN {
        return;
    }
    match event {
        WindowEvent::CloseRequested { api, .. } => {
            let state = window.state::<AppState>();
            if state.quitting.load(Ordering::SeqCst) {
                return;
            }
            let shown = lock(&state.config).tray_notice_shown;
            if !shown && !state.tray_notice_asked.swap(true, Ordering::SeqCst) {
                api.prevent_close();
                let _ = window.emit_to(MAIN, "show-tray-notice", ());
            }
        }
        WindowEvent::Destroyed => {
            // Closing the window fully stops the desktop capture (RAM returns to the background level).
            window.state::<AppState>().services.capture.stop_all();
            tracing::info!(target: "sg::app", "window closed (webview released)");
        }
        _ => {}
    }
}

fn show_webview2_missing(app: &AppHandle) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
    let lang = app.state::<AppState>().lang();
    let title = sg_core::i18n::t(lang, "dialog.webview2_missing.title", &[]);
    let body = sg_core::i18n::t(lang, "dialog.webview2_missing.body", &[("url", WEBVIEW2_URL)]);
    tracing::error!(target: "sg::app", "WebView2 runtime missing");
    app.dialog().message(body).title(title).kind(MessageDialogKind::Error).show(|_| {});
}

/// Windows 11 rounds the corners of borderless windows on its own; the ShunguangDecor UI is square.
fn square_corners(w: &WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DwmSetWindowAttribute};
    let Ok(h) = w.hwnd() else { return };
    // Tauri's HWND belongs to a different version of the `windows` crate: convert through a raw pointer.
    let hwnd = HWND(h.0);
    let pref = DWMWCP_DONOTROUND;
    // SAFETY: hwnd is the window just created; the buffer is a correctly sized DWM_WINDOW_CORNER_PREFERENCE.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const pref).cast::<core::ffi::c_void>(),
            std::mem::size_of_val(&pref) as u32,
        );
    }
}

