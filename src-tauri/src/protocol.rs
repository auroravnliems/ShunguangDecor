//! The `sg://` protocol for images: no base64 is sent through events.
//! WebView2 uses the form `http://sg.localhost/…`.

use crate::state::{AppState, ThumbCache, lock};
use sg_core::desktop;
use std::sync::Arc;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager, UriSchemeContext, UriSchemeResponder, Wry};

/// The wallpaper image on the Home page is at most 1280 px wide.
const WALLPAPER_WIDTH: u32 = 1280;

pub fn handle(ctx: UriSchemeContext<'_, Wry>, req: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let app = ctx.app_handle().clone();
    let path = req.uri().path().to_string();
    let range = req.headers().get(header::RANGE).and_then(|v| v.to_str().ok()).map(str::to_string);
    // Decoding the image can take ~100 ms: do it off the UI thread.
    tauri::async_runtime::spawn_blocking(move || {
        let resp = match path.as_str() {
            "/wallpaper/current" => current_wallpaper(&app),
            p if p.starts_with("/wallpaper/media/") => wallpaper_media(&app, p, range.as_deref()),
            p if p.starts_with("/cursor/") => cursor_image(&app, p),
            p if p.starts_with("/capture/") => capture_image(&app, p),
            p if p.starts_with("/wallpaper/thumb/") => wallpaper_thumb(&app, p),
            _ => status(StatusCode::NOT_FOUND),
        };
        responder.respond(resp);
    });
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder().status(code).body(Vec::new()).unwrap_or_default()
}

/// Cursor preview frame (`/cursor/<id>/<Role>/sheet.png` or `/<n>.png`).
fn cursor_image(app: &AppHandle, path: &str) -> Response<Vec<u8>> {
    let state = app.state::<AppState>();
    match crate::modules::cursor::serve(&state, path) {
        Some(png) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/png")
            .header(header::CACHE_CONTROL, "no-store")
            .body(png)
            .unwrap_or_default(),
        None => status(StatusCode::NOT_FOUND),
    }
}

/// Thumbnail of a wallpaper in the library (`/wallpaper/thumb/<id>.jpg`).
fn wallpaper_thumb(app: &AppHandle, path: &str) -> Response<Vec<u8>> {
    let state = app.state::<AppState>();
    match crate::modules::wallpaper::serve_thumb(&state, path) {
        Some(jpeg) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/jpeg")
            .header(header::CACHE_CONTROL, "no-cache")
            .body(jpeg)
            .unwrap_or_default(),
        None => status(StatusCode::NOT_FOUND),
    }
}

/// Video/GIF file of a wallpaper in the library (`/wallpaper/media/<id>`), with `Range` support (video needs it for seeking/looping):
/// with `Range` → 206, at most 4 MiB; without → 200 with the whole file; malformed/out of range → 416; unknown id → 404.
fn wallpaper_media(app: &AppHandle, path: &str, range: Option<&str>) -> Response<Vec<u8>> {
    use crate::modules::wallpaper::{MediaReply, serve_media};
    let state = app.state::<AppState>();
    match serve_media(&state, path, range) {
        MediaReply::NotFound => status(StatusCode::NOT_FOUND),
        MediaReply::TooLarge => status(StatusCode::PAYLOAD_TOO_LARGE),
        MediaReply::Unsatisfiable(total) => Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{total}"))
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Vec::new())
            .unwrap_or_default(),
        MediaReply::Data { body, content_type, total, range } => {
            let mut b = Response::builder()
                .header(header::CONTENT_TYPE, content_type)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CACHE_CONTROL, "no-cache");
            b = match range {
                Some((s, e)) => b.status(StatusCode::PARTIAL_CONTENT).header(header::CONTENT_RANGE, format!("bytes {s}-{e}/{total}")),
                None => b.status(StatusCode::OK),
            };
            b.body(body).unwrap_or_default()
        }
    }
}

/// The latest real desktop frame (`/capture/<monitor>.jpg`, or `-icons.png` with icons only); no frame yet → 404.
fn capture_image(app: &AppHandle, path: &str) -> Response<Vec<u8>> {
    let state = app.state::<AppState>();
    match crate::modules::capture::serve(&state, path) {
        Some((bytes, content_type)) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CACHE_CONTROL, "no-store")
            .body(bytes)
            .unwrap_or_default(),
        None => status(StatusCode::NOT_FOUND),
    }
}

fn current_wallpaper(app: &AppHandle) -> Response<Vec<u8>> {
    let Some(w) = desktop::current_wallpaper() else { return status(StatusCode::NOT_FOUND) };
    let state = app.state::<AppState>();
    let cached = lock(&state.thumb)
        .as_ref()
        .filter(|c| c.path == w.path && c.modified_unix == w.modified_unix)
        .map(|c| c.jpeg.clone());
    let jpeg = match cached {
        Some(j) => j,
        None => match desktop::thumbnail_jpeg(&w.path, WALLPAPER_WIDTH) {
            Ok(bytes) => {
                let j = Arc::new(bytes);
                *lock(&state.thumb) = Some(ThumbCache { path: w.path.clone(), modified_unix: w.modified_unix, jpeg: j.clone() });
                j
            }
            Err(e) => {
                tracing::warn!(target: "sg::app", "cannot read wallpaper image: {e}");
                return status(StatusCode::UNSUPPORTED_MEDIA_TYPE);
            }
        },
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "no-store")
        .body(jpeg.as_ref().clone())
        .unwrap_or_default()
}
