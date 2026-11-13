// Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).
//! Wallpaper engine: a dedicated thread (COM STA) with a hidden window + message loop that manages the running wallpapers.
//!
//! Every event (timer, command from the app, player, monitor change, Explorer restart) goes through a single queue.
//! Some operations (creating WebView2...) pump nested messages themselves; thanks to the queue, events arriving in the meantime are handled
//! later instead of being re-entered into a busy `App`. Commands from other threads go into the shared pending queue and then `PostMessage(WM_DRAIN)`.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use image::RgbaImage;
use serde_json::Value;
use sg_core::explorer_identity::{ExplorerIdentity, IdentityTracker};
use sg_desktop::{DesktopLayer, Monitor, Rect};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
use windows::Win32::System::RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, MSG, PostMessageW, PostQuitMessage,
    RegisterClassExW, RegisterWindowMessageW, SetTimer, TranslateMessage, WM_APP, WM_DISPLAYCHANGE, WM_POWERBROADCAST, WM_TIMER,
    WM_WTSSESSION_CHANGE, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::w;

use crate::backup;
use crate::config::{Scaling, Target, TargetKind, VideoPlayer, WallpaperConfig, WallpaperPaths, plan_targets};
use crate::library::{self, Wallpaper, WallpaperType};
use crate::player::app::AppPlayer;
use crate::player::mf::MfPlayer;
use crate::player::mpv::{self, MpvPlayer};
use crate::player::picture::PicturePlayer;
use crate::player::web::{self, WebContent, WebOptions, WebPlayer};
use crate::player::{PauseReason, Player};
use crate::state::{EngineEvent, EngineState, RunningView};
use crate::{playback, properties, thumbs, util};

pub const WM_PLAYER_EVENT: u32 = WM_APP + 2;
const WM_DRAIN: u32 = WM_APP + 6;
const TIMER_TICK: usize = 1;
const WTS_SESSION_LOCK: usize = 0x7;
const WTS_SESSION_UNLOCK: usize = 0x8;
/// How long to wait at most for a command that has a reply.
const REPLY_TIMEOUT: Duration = Duration::from_secs(8);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_millis(1500);
const MAIN_CLASS: windows::core::PCWSTR = w!("ShunguangDecor.WallpaperMain");

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

/// Callable from any thread (Media Foundation and WebView2 callbacks).
pub fn post_player_event(host: isize, code: u32, param: i32) {
    let main = MAIN_HWND.load(Ordering::Relaxed);
    if main == 0 {
        return;
    }
    let lp = ((param as i64) << 8) | (code as i64 & 0xff);
    // SAFETY: PostMessage accepts an HWND that is no longer valid (returns an error, ignored).
    unsafe {
        let _ = PostMessageW(Some(HWND(main as *mut _)), WM_PLAYER_EVENT, WPARAM(host as usize), LPARAM(lp as isize));
    }
}

// ------------------------------------------------------------------ commands coming in from outside

type Reply<T> = Sender<T>;

enum Cmd {
    ApplyConfig(Box<WallpaperConfig>, Reply<()>),
    SetManualPause(bool, Reply<()>),
    TogglePause(Reply<bool>),
    SetProperty { id: String, name: String, value: Value, reply: Reply<Result<(), String>> },
    ResetProperties { id: String, reply: Reply<()> },
    /// (monitor, whether to capture the live wallpaper's host window; false = only the static image's frame)
    Snapshot(u32, bool, Reply<Option<RgbaImage>>),
    State(Reply<EngineState>),
    Shutdown(Reply<()>),
}

type CmdQueue = Arc<Mutex<VecDeque<Cmd>>>;

enum Event {
    Tick,
    Cmd(Cmd),
    Player { host: isize, code: u32, param: i32 },
    Display,
    Session { locked: bool },
    TaskbarCreated,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static QUEUE: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    static DRAINING: Cell<bool> = const { Cell::new(false) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    static CMDS: RefCell<Option<CmdQueue>> = const { RefCell::new(None) };
}

fn dispatch(ev: Event) {
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        // Coalesce the Ticks that piled up while busy.
        if matches!(ev, Event::Tick) && q.iter().any(|e| matches!(e, Event::Tick)) {
            return;
        }
        q.push_back(ev);
    });
    drain();
}

fn drain() {
    if DRAINING.with(Cell::get) {
        return;
    }
    DRAINING.with(|d| d.set(true));
    loop {
        // Move commands from the shared pending queue into the event queue (preserving arrival order).
        CMDS.with(|c| {
            if let Some(q) = c.borrow().as_ref() {
                let mut g = q.lock().unwrap_or_else(|e| e.into_inner());
                while let Some(cmd) = g.pop_front() {
                    QUEUE.with(|q| q.borrow_mut().push_back(Event::Cmd(cmd)));
                }
            }
        });
        let Some(ev) = QUEUE.with(|q| q.borrow_mut().pop_front()) else { break };
        APP.with(|a| {
            if let Ok(mut guard) = a.try_borrow_mut()
                && let Some(app) = guard.as_mut()
            {
                app.handle(ev);
            }
        });
    }
    DRAINING.with(|d| d.set(false));
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wp.0 == TIMER_TICK => dispatch(Event::Tick),
        WM_DRAIN => drain(),
        WM_PLAYER_EVENT => {
            let code = (lp.0 & 0xff) as u32;
            let param = (lp.0 as i64 >> 8) as i32;
            dispatch(Event::Player { host: wp.0 as isize, code, param });
        }
        WM_DISPLAYCHANGE => dispatch(Event::Display),
        WM_WTSSESSION_CHANGE => match wp.0 {
            WTS_SESSION_LOCK => dispatch(Event::Session { locked: true }),
            WTS_SESSION_UNLOCK => dispatch(Event::Session { locked: false }),
            _ => {}
        },
        WM_POWERBROADCAST => dispatch(Event::Tick),
        m if m != 0 && m == TASKBAR_CREATED.with(Cell::get) => dispatch(Event::TaskbarCreated),
        _ => {}
    }
    // SAFETY: forwards the parameters unchanged to the default procedure.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

// ------------------------------------------------------------------ state

struct Active {
    target: Target,
    wallpaper: Wallpaper,
    player: Box<dyn Player>,
    paused: Option<PauseReason>,
}

struct App {
    hwnd: HWND,
    paths: WallpaperPaths,
    desktop: Option<DesktopLayer>,
    monitors: Vec<Monitor>,
    cfg: WallpaperConfig,
    active: Vec<Active>,
    web_ctx: wry::WebContext,
    locked: bool,
    manual_pause: bool,
    display_changed_at: Option<Instant>,
    trim_at: Option<Instant>,
    errors: BTreeMap<String, String>,
    last_error: Option<String>,
    last_state: Option<EngineState>,
    events: Box<dyn Fn(EngineEvent)>,
    down: bool,
    /// Static image already read + downscaled for the preview frame: (path, mtime, image).
    picture_cache: Option<(String, u64, RgbaImage)>,
    /// Explorer identity marker: rebuild only when `TaskbarCreated` comes with a genuinely changed Explorer (see `explorer_identity`).
    explorer: IdentityTracker,
}

/// A `TaskbarCreated` message simulated by the debug command (`simulate_taskbar_created`): always rebuild.
static FORCE_REBUILD: AtomicBool = AtomicBool::new(false);

/// The desktop's DefView + its owner pid (part of Explorer's identity).
fn desktop_identity() -> Option<(isize, u32)> {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let dv = DesktopLayer::locate()?.icon_view()?;
    let mut pid = 0u32;
    // SAFETY: dv is a live window.
    unsafe { GetWindowThreadProcessId(dv, Some(&mut pid)) };
    Some((dv.0 as isize, pid))
}

/// Does `TaskbarCreated` mean Explorer restarted? Any app can broadcast this message (the app's taskbar engine
/// does so every time it is turned on), so rebuild only when Explorer's identity changed, or when the message was simulated by the debug command.
/// Always updates the marker so the next one is not reported twice.
fn taskbar_created_means_restart(tracker: &mut IdentityTracker, now: ExplorerIdentity, forced: bool) -> bool {
    let changed = tracker.observe(now);
    forced || changed
}

fn to_rect(r: Rect) -> RECT {
    r.into()
}

impl App {
    fn handle(&mut self, ev: Event) {
        match ev {
            Event::Tick => self.tick(),
            Event::Cmd(c) => self.on_cmd(c),
            Event::Player { host, code, param } => self.on_player_event(host, code, param),
            Event::Display => self.display_changed_at = Some(Instant::now()),
            Event::Session { locked } => {
                self.locked = locked;
                self.apply_pause();
            }
            Event::TaskbarCreated => {
                let now = ExplorerIdentity::measure(desktop_identity());
                let forced = FORCE_REBUILD.swap(false, Ordering::SeqCst);
                if taskbar_created_means_restart(&mut self.explorer, now, forced) {
                    logln!("Explorer restarted — rebuilding wallpapers");
                    self.rebuild_all();
                } else {
                    logln!("TaskbarCreated ignored: the Explorer taskbar/desktop did not change (another app broadcast it)");
                }
            }
        }
    }

    fn on_cmd(&mut self, c: Cmd) {
        match c {
            Cmd::ApplyConfig(cfg, r) => {
                self.apply_config(*cfg);
                let _ = r.send(());
            }
            Cmd::SetManualPause(p, r) => {
                self.set_manual_pause(p);
                let _ = r.send(());
            }
            Cmd::TogglePause(r) => {
                let p = !self.manual_pause;
                self.set_manual_pause(p);
                let _ = r.send(p);
            }
            Cmd::SetProperty { id, name, value, reply } => {
                let _ = reply.send(self.set_property(&id, &name, value));
            }
            Cmd::ResetProperties { id, reply } => {
                self.reset_properties(&id);
                let _ = reply.send(());
            }
            Cmd::Snapshot(n, window, r) => {
                let _ = r.send(self.snapshot(n, window));
            }
            Cmd::State(r) => {
                let _ = r.send(self.state());
            }
            Cmd::Shutdown(r) => {
                self.shutdown();
                let _ = r.send(());
            }
        }
    }

    // -------------------------------------------------------------- state reported outward

    fn state(&self) -> EngineState {
        EngineState {
            running: self
                .active
                .iter()
                .map(|a| RunningView {
                    monitor: match &a.target.kind {
                        TargetKind::Monitor(m) => m.number,
                        TargetKind::Span => 0,
                    },
                    wallpaper_id: a.wallpaper.id.clone(),
                    backend: a.player.backend().to_string(),
                    paused: a.paused.is_some(),
                })
                .collect(),
            manual_pause: self.manual_pause,
            error: self.last_error.clone(),
            errors: self.errors.clone(),
        }
    }

    /// Reports the new state to the app, only when it changed.
    fn push_state(&mut self) {
        let s = self.state();
        if self.last_state.as_ref() != Some(&s) {
            self.last_state = Some(s.clone());
            (self.events)(EngineEvent::State(s));
        }
    }

    fn report_error(&mut self, id: Option<&str>, message: String) {
        logln!("wallpaper error ({}): {message}", id.unwrap_or("-"));
        if let Some(id) = id {
            self.errors.insert(id.to_string(), message.clone());
        }
        self.last_error = Some(message.clone());
        (self.events)(EngineEvent::Error { wallpaper_id: id.map(str::to_string), message });
    }

    // -------------------------------------------------------------- tick

    /// Schedules a working-set trim after `ms` (merges several requests close together into one).
    fn schedule_trim(&mut self, ms: u64) {
        if !self.cfg.trim_memory {
            return;
        }
        let at = Instant::now() + Duration::from_millis(ms);
        self.trim_at = Some(self.trim_at.map_or(at, |t| t.max(at)));
    }

    fn tick(&mut self) {
        if let Some(t) = self.trim_at
            && Instant::now() >= t
        {
            self.trim_at = None;
            util::trim_working_set();
        }
        if let Some(t) = self.display_changed_at
            && t.elapsed() > Duration::from_millis(1500)
        {
            self.display_changed_at = None;
            let now = sg_desktop::monitors();
            let same_devices = now.len() == self.monitors.len() && now.iter().zip(&self.monitors).all(|(a, b)| a.device == b.device);
            let moved = now.iter().zip(&self.monitors).any(|(a, b)| a.rect != b.rect);
            if !same_devices {
                logln!("monitors plugged/unplugged — rebuilding wallpapers");
                self.rebuild_all();
            } else if moved {
                logln!("monitor resolution changed — resizing wallpapers");
                self.monitors = now;
                self.resize_all();
            }
        }
        if let Some(d) = &self.desktop
            && !d.is_valid()
        {
            logln!("desktop layer lost — locating it again");
            self.rebuild_all();
        }
        let mut failed = Vec::new();
        for (i, a) in self.active.iter_mut().enumerate() {
            if let Err(e) = a.player.tick() {
                failed.push((i, e));
            }
        }
        for (i, e) in failed.into_iter().rev() {
            let mut a = self.active.remove(i);
            a.player.close();
            let id = a.wallpaper.id.clone();
            self.report_error(Some(&id), e);
        }
        self.apply_pause();
        self.push_state();
    }

    fn apply_pause(&mut self) {
        if self.active.is_empty() {
            return;
        }
        let snap = playback::check(&self.cfg, &self.monitors, self.desktop.as_ref(), self.locked);
        let manual = self.manual_pause;
        let mut newly_paused = false;
        for a in self.active.iter_mut() {
            let want = if manual || snap.system_pause {
                Some(PauseReason::System)
            } else {
                let covered = match &a.target.kind {
                    TargetKind::Monitor(m) => snap.covered.contains(&m.device),
                    TargetKind::Span => !self.monitors.is_empty() && self.monitors.iter().all(|m| snap.covered.contains(&m.device)),
                };
                covered.then_some(PauseReason::Covered)
            };
            if want != a.paused {
                a.player.set_paused(want.is_some(), want.unwrap_or(PauseReason::System));
                if want.is_some() {
                    newly_paused = true;
                }
                a.paused = want;
            }
        }
        if newly_paused {
            self.schedule_trim(1500);
        }
    }

    // -------------------------------------------------------------- building wallpapers

    fn ensure_desktop(&mut self) -> Result<DesktopLayer, String> {
        if let Some(d) = self.desktop
            && d.is_valid()
        {
            return Ok(d);
        }
        self.desktop = DesktopLayer::locate();
        self.desktop.ok_or_else(|| "Explorer desktop layer not found (is Explorer running?)".to_string())
    }

    fn close_all(&mut self) {
        for mut a in self.active.drain(..) {
            a.player.close();
        }
    }

    fn resize_all(&mut self) {
        let Some(desktop) = self.desktop else { return };
        for a in self.active.iter_mut() {
            if let TargetKind::Monitor(m) = &mut a.target.kind
                && let Some(fresh) = self.monitors.iter().find(|x| x.device == m.device)
            {
                *m = fresh.clone();
            }
            let screen = match &a.target.kind {
                TargetKind::Monitor(m) => m.rect,
                TargetKind::Span => sg_desktop::virtual_screen(),
            };
            a.player.resize(to_rect(desktop.screen_to_parent(screen)));
        }
    }

    /// Closes everything and rebuilds from the configuration (Explorer restarted, monitors changed).
    fn rebuild_all(&mut self) {
        self.close_all();
        self.desktop = DesktopLayer::locate();
        self.monitors = sg_desktop::monitors();
        self.start_missing();
        self.finish_change();
    }

    fn finish_change(&mut self) {
        self.restore_picture_if_unused();
        self.apply_pause();
        self.push_state();
        self.schedule_trim(5000);
    }

    /// No static-image player is left but Windows still holds our image → restore the old wallpaper.
    fn restore_picture_if_unused(&mut self) {
        let pictures = self.active.iter().any(|a| a.player.backend() == "windows-wallpaper");
        if !pictures && self.paths.backup_file.is_file() {
            if let Err(e) = backup::restore_and_clear(&self.paths.backup_file) {
                logln!("cannot restore the Windows wallpaper: {e}");
            }
        }
    }

    fn is_active(&self, t: &Target) -> bool {
        self.active.iter().any(|a| a.target.key() == t.key() && a.wallpaper.id == t.wallpaper)
    }

    /// Starts the targets in the configuration that are not running yet.
    fn start_missing(&mut self) {
        let targets = plan_targets(&self.cfg, &self.monitors);
        for t in targets {
            if self.is_active(&t) {
                continue;
            }
            let Some(w) = library::find(&self.paths, &t.wallpaper) else {
                self.report_error(Some(&t.wallpaper), format!("Wallpaper {} is no longer in the library", t.wallpaper));
                continue;
            };
            match self.start(&w, t) {
                Ok(()) => {
                    self.errors.remove(&w.id);
                }
                Err(e) => self.report_error(Some(&w.id), e),
            }
        }
    }

    /// Applies the new configuration: rebuilds only the monitors that changed.
    fn apply_config(&mut self, new: WallpaperConfig) {
        let old = std::mem::replace(&mut self.cfg, new);
        let c = &self.cfg;
        let global = old.scaling != c.scaling
            || old.video_player != c.video_player
            || old.mpv_path != c.mpv_path
            || old.hardware_decoding != c.hardware_decoding
            || old.arrangement != c.arrangement;
        self.monitors = sg_desktop::monitors();
        if self.ensure_desktop().is_err() {
            logln!("no desktop layer yet — will retry on the next tick");
        }
        let targets = plan_targets(&self.cfg, &self.monitors);
        // Close what is no longer in the plan (or everything when the shared parameters changed).
        let mut i = 0;
        while i < self.active.len() {
            let a = &self.active[i];
            let keep = !global && targets.iter().any(|t| t.key() == a.target.key() && t.wallpaper == a.wallpaper.id);
            if keep {
                i += 1;
            } else {
                let mut a = self.active.remove(i);
                a.player.close();
            }
        }
        // Drop errors of wallpapers that are no longer in the layout (deleted/removed).
        self.errors.retain(|id, _| targets.iter().any(|t| &t.wallpaper == id));
        self.start_missing();
        if self.errors.is_empty() {
            self.last_error = None;
        }
        if old.volume != self.cfg.volume {
            for a in self.active.iter_mut() {
                a.player.set_volume(self.cfg.volume);
            }
        }
        if old.poll_interval_ms != self.cfg.poll_interval_ms {
            // SAFETY: same window and timer id, SetTimer replaces the old timer.
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_TICK, self.cfg.poll_interval_ms, None);
            }
        }
        self.finish_change();
    }

    fn start(&mut self, w: &Wallpaper, target: Target) -> Result<(), String> {
        library::check_source(w)?;
        let player = self.spawn(w, &target.kind, 0)?;
        logln!("started '{}' [{}] with {}", w.title(), w.kind().as_str(), player.backend());
        let mut player = player;
        for (n, v) in properties::startup_values(&self.paths, w) {
            // The web player receives it when the page finishes loading; other players ignore it.
            player.apply_property(&n, &v);
        }
        // Thumbnails for the library (cheap, done once).
        let _ = thumbs::ensure_thumb(&self.paths, w);
        self.active.push(Active { target, wallpaper: w.clone(), player, paused: None });
        Ok(())
    }

    fn target_rect(&self, desktop: &DesktopLayer, kind: &TargetKind) -> RECT {
        let screen = match kind {
            TargetKind::Monitor(m) => m.rect,
            TargetKind::Span => sg_desktop::virtual_screen(),
        };
        to_rect(desktop.screen_to_parent(screen))
    }

    /// Creates a player. `stage` > 0 when trying a fallback player for video.
    fn spawn(&mut self, w: &Wallpaper, kind_t: &TargetKind, stage: u8) -> Result<Box<dyn Player>, String> {
        let source = w.source();
        let kind = w.kind();
        let cfg = self.cfg.clone();

        if kind == WallpaperType::Picture && stage == 0 {
            let rect = match kind_t {
                TargetKind::Monitor(m) => Some(to_rect(m.rect)),
                TargetKind::Span => None,
            };
            match PicturePlayer::new(&source, rect, cfg.scaling, &self.paths.backup_file) {
                Ok(p) => return Ok(Box::new(p)),
                Err(e) => logln!("{e} — falling back to WebView2"),
            }
        }

        let desktop = self.ensure_desktop()?;
        let rect = self.target_rect(&desktop, kind_t);
        let mpv_exe = mpv::find_mpv(&cfg);
        let webview_dir = self.paths.webview.clone();
        let _ = std::fs::create_dir_all(&webview_dir);

        let web_media = |app: &mut App, video: bool| -> Result<Box<dyn Player>, String> {
            let p = std::path::Path::new(&source);
            let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
            let file = p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
            let html = web::media_page(&file, video, cfg.scaling, cfg.volume);
            let opts = WebOptions { context: &mut app.web_ctx, volume: cfg.volume, devtools: false, properties: Vec::new() };
            Ok(Box::new(WebPlayer::new(&desktop, rect, WebContent::Generated { dir, html }, opts)?))
        };

        match kind {
            WallpaperType::Video => {
                // Order to try: Media Foundation → mpv → WebView2 <video>.
                let order: &[&str] = match cfg.video_player {
                    VideoPlayer::Auto => &["mf", "mpv", "web"],
                    VideoPlayer::MediaFoundation => &["mf", "web"],
                    VideoPlayer::Mpv => &["mpv", "mf", "web"],
                };
                let mut last_err = String::from("No video player is available");
                for backend in order.iter().skip(stage as usize) {
                    let r: Result<Box<dyn Player>, String> = match *backend {
                        "mf" => MfPlayer::new(&desktop, rect, &source, cfg.volume, cfg.scaling).map(|p| Box::new(p) as Box<dyn Player>),
                        "mpv" => match &mpv_exe {
                            Some(exe) => MpvPlayer::new(&desktop, rect, exe, &source, &cfg, false).map(|p| Box::new(p) as Box<dyn Player>),
                            None => Err("mpv is not installed".into()),
                        },
                        _ => web_media(self, true),
                    };
                    match r {
                        Ok(p) => return Ok(p),
                        Err(e) => {
                            logln!("player {backend} failed: {e}");
                            last_err = e;
                        }
                    }
                }
                Err(last_err)
            }
            WallpaperType::VideoStream => match &mpv_exe {
                Some(exe) => Ok(Box::new(MpvPlayer::new(&desktop, rect, exe, &source, &cfg, false)?)),
                None => {
                    let opts = WebOptions { context: &mut self.web_ctx, volume: cfg.volume, devtools: false, properties: Vec::new() };
                    Ok(Box::new(WebPlayer::new(&desktop, rect, WebContent::Url(source.clone()), opts)?))
                }
            },
            WallpaperType::Gif => match (&mpv_exe, stage) {
                (Some(exe), 0) => Ok(Box::new(MpvPlayer::new(&desktop, rect, exe, &source, &cfg, true)?)),
                _ => web_media(self, false),
            },
            WallpaperType::Picture => web_media(self, false),
            WallpaperType::Web | WallpaperType::WebAudio => {
                let (dir, file) = if w.info.is_absolute_path {
                    let p = std::path::Path::new(&source);
                    (
                        p.parent().map(|d| d.to_path_buf()).unwrap_or_default(),
                        p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
                    )
                } else {
                    (w.dir.clone(), w.info.file_name.clone().unwrap_or_else(|| "index.html".into()))
                };
                let opts = WebOptions { context: &mut self.web_ctx, volume: cfg.volume, devtools: false, properties: Vec::new() };
                Ok(Box::new(WebPlayer::new(&desktop, rect, WebContent::Local { dir, file }, opts)?))
            }
            WallpaperType::Url => {
                let opts = WebOptions { context: &mut self.web_ctx, volume: cfg.volume, devtools: false, properties: Vec::new() };
                Ok(Box::new(WebPlayer::new(&desktop, rect, WebContent::Url(source.clone()), opts)?))
            }
            _ => Ok(Box::new(AppPlayer::new(&desktop, rect, &source, w.info.arguments.as_deref())?)),
        }
    }

    fn on_player_event(&mut self, host: isize, code: u32, param: i32) {
        let Some(i) = self.active.iter().position(|a| a.player.host().map(|h| h.0 as isize) == Some(host)) else {
            return;
        };
        let result = self.active[i].player.on_event(code, param as isize);
        if let Err(e) = result {
            let mut a = self.active.remove(i);
            let backend = a.player.backend();
            logln!("'{}' failed ({backend}): {e}", a.wallpaper.title());
            a.player.close();
            // Video failed in Media Foundation → try the next player.
            if a.wallpaper.kind() == WallpaperType::Video && backend == "media-foundation" {
                let stage = match self.cfg.video_player {
                    VideoPlayer::Mpv => 2,
                    _ => 1,
                };
                match self.spawn(&a.wallpaper, &a.target.kind, stage) {
                    Ok(p) => {
                        logln!("switched to {}", p.backend());
                        a.player = p;
                        a.paused = None;
                        self.active.push(a);
                        self.apply_pause();
                        self.push_state();
                        return;
                    }
                    Err(e2) => {
                        let id = a.wallpaper.id.clone();
                        self.report_error(Some(&id), format!("{e} / {e2}"));
                    }
                }
            } else {
                let id = a.wallpaper.id.clone();
                self.report_error(Some(&id), e);
            }
            self.push_state();
        }
    }

    // -------------------------------------------------------------- commands

    fn set_manual_pause(&mut self, paused: bool) {
        self.manual_pause = paused;
        self.apply_pause();
        self.push_state();
    }

    fn set_property(&mut self, id: &str, name: &str, value: Value) -> Result<(), String> {
        let w = library::find(&self.paths, id).ok_or("Wallpaper not found")?;
        if let Some(eff) = properties::set_value(&self.paths, &w, name, value)? {
            for a in self.active.iter_mut().filter(|a| a.wallpaper.id == id) {
                a.player.apply_property(name, &eff);
            }
        }
        Ok(())
    }

    fn reset_properties(&mut self, id: &str) {
        let Some(w) = library::find(&self.paths, id) else { return };
        properties::reset(&self.paths, id);
        let vals = properties::startup_values(&self.paths, &w);
        for a in self.active.iter_mut().filter(|a| a.wallpaper.id == id) {
            for (n, v) in &vals {
                a.player.apply_property(n, v);
            }
        }
    }

    /// The currently running frame of a monitor (for the preview).
    /// Static image: built from the image file itself using the fill style in use (Progman repaints slowly, so printing it may still show the old image).
    /// Live wallpaper: only when `window` — captures the host window (slower; used when printing Progman yields no image).
    fn snapshot(&mut self, monitor: u32, window: bool) -> Option<RgbaImage> {
        let m = self.monitors.iter().find(|m| m.number == monitor)?.clone();
        let a = self.active.iter().find(|a| match &a.target.kind {
            TargetKind::Monitor(t) => t.number == monitor,
            TargetKind::Span => true,
        })?;
        let span = matches!(a.target.kind, TargetKind::Span);
        if a.player.backend() == "windows-wallpaper" {
            if span {
                return None;
            }
            let source = a.wallpaper.source();
            let path = std::path::PathBuf::from(&source);
            let mtime = std::fs::metadata(&path)
                .and_then(|x| x.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let fresh = !matches!(&self.picture_cache, Some((p, t, _)) if *p == source && *t == mtime);
            if fresh {
                let img = crate::frame::read_picture(&path)?;
                let img = crate::frame::fit_picture(&img, img.dimensions(), Scaling::Stretch, 1920);
                self.picture_cache = Some((source, mtime, img));
            }
            let (_, _, img) = self.picture_cache.as_ref()?;
            return Some(crate::frame::fit_picture(img, (m.rect.width() as u32, m.rect.height() as u32), self.cfg.scaling, 960));
        }
        if !window {
            return None;
        }
        let host = a.player.host()?;
        let img = thumbs::snapshot_window(host).ok()?;
        if span {
            let vs = sg_desktop::virtual_screen();
            let (x, y) = ((m.rect.left - vs.left).max(0) as u32, (m.rect.top - vs.top).max(0) as u32);
            let (w, h) = (m.rect.width() as u32, m.rect.height() as u32);
            if x + w <= img.width() && y + h <= img.height() {
                return Some(image::imageops::crop_imm(&img, x, y, w, h).to_image());
            }
            return None;
        }
        Some(img)
    }

    fn shutdown(&mut self) {
        if self.down {
            return;
        }
        self.down = true;
        logln!("engine shutting down");
        self.close_all();
        if let Err(e) = backup::restore_and_clear(&self.paths.backup_file) {
            logln!("cannot restore the Windows wallpaper: {e}");
        }
        self.push_state();
        // SAFETY: the window and timer belong to this thread.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_TICK);
            PostQuitMessage(0);
        }
    }
}

// ------------------------------------------------------------------ control handle

/// Handle the app uses to control the engine from any thread. Every command goes through the pending queue + `WM_DRAIN`.
pub struct EngineHandle {
    hwnd: isize,
    cmds: CmdQueue,
    thread: Mutex<Option<JoinHandle<()>>>,
    dead: AtomicBool,
}

impl EngineHandle {
    /// Builds the engine thread with the initial configuration (no wallpaper runs until `apply_config`).
    pub fn spawn(
        paths: WallpaperPaths,
        cfg: WallpaperConfig,
        events: impl Fn(EngineEvent) + Send + 'static,
    ) -> Result<EngineHandle, String> {
        paths.ensure();
        let cmds: CmdQueue = Arc::new(Mutex::new(VecDeque::new()));
        let (ready_tx, ready_rx) = channel::<Result<isize, String>>();
        let q = cmds.clone();
        let thread = std::thread::Builder::new()
            .name("sg-wallpaper".into())
            .spawn(move || run(paths, cfg, q, Box::new(events), ready_tx))
            .map_err(|e| format!("cannot start the wallpaper thread: {e}"))?;
        match ready_rx.recv_timeout(REPLY_TIMEOUT) {
            Ok(Ok(hwnd)) => Ok(EngineHandle { hwnd, cmds, thread: Mutex::new(Some(thread)), dead: AtomicBool::new(false) }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("the wallpaper engine did not start in time".into()),
        }
    }

    fn post(&self, cmd: Cmd) -> Result<(), String> {
        if self.dead.load(Ordering::SeqCst) {
            return Err("the wallpaper engine is not running".into());
        }
        self.cmds.lock().unwrap_or_else(|e| e.into_inner()).push_back(cmd);
        // SAFETY: PostMessage accepts a dead HWND (returns an error).
        unsafe { PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_DRAIN, WPARAM(0), LPARAM(0)) }
            .map_err(|e| format!("cannot reach the wallpaper engine: {e}"))
    }

    fn ask<T>(&self, make: impl FnOnce(Reply<T>) -> Cmd, timeout: Duration) -> Result<T, String> {
        let (tx, rx) = channel();
        self.post(make(tx))?;
        rx.recv_timeout(timeout).map_err(|_| "the wallpaper engine did not answer in time".to_string())
    }

    /// Applies the new configuration: rebuilds exactly the monitors that changed.
    pub fn apply_config(&self, cfg: WallpaperConfig) -> Result<(), String> {
        self.ask(|r| Cmd::ApplyConfig(Box::new(cfg), r), REPLY_TIMEOUT)
    }

    pub fn set_manual_pause(&self, paused: bool) -> Result<(), String> {
        self.ask(|r| Cmd::SetManualPause(paused, r), REPLY_TIMEOUT)
    }

    /// Toggles the paused state; returns the new state.
    pub fn toggle_pause(&self) -> Result<bool, String> {
        self.ask(Cmd::TogglePause, REPLY_TIMEOUT)
    }

    pub fn set_property(&self, id: &str, name: &str, value: Value) -> Result<(), String> {
        self.ask(|reply| Cmd::SetProperty { id: id.into(), name: name.into(), value, reply }, REPLY_TIMEOUT)?
    }

    pub fn reset_properties(&self, id: &str) -> Result<(), String> {
        self.ask(|reply| Cmd::ResetProperties { id: id.into(), reply }, REPLY_TIMEOUT)
    }

    /// The currently running frame of a monitor (see `App::snapshot`).
    pub fn snapshot(&self, monitor: u32, window: bool) -> Option<RgbaImage> {
        self.ask(|r| Cmd::Snapshot(monitor, window, r), SNAPSHOT_TIMEOUT).ok().flatten()
    }

    pub fn state(&self) -> Result<EngineState, String> {
        self.ask(Cmd::State, REPLY_TIMEOUT)
    }

    /// For manual testing: simulates an Explorer restart (the registered `TaskbarCreated` message) without killing the real Explorer.
    pub fn simulate_taskbar_created(&self) {
        FORCE_REBUILD.store(true, Ordering::SeqCst);
        // SAFETY: registering/sending a window message has no preconditions.
        unsafe {
            let m = RegisterWindowMessageW(w!("TaskbarCreated"));
            let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), m, WPARAM(0), LPARAM(0));
        }
    }

    /// Closes every player, restores the old wallpaper, exits the thread. A second call does not error.
    pub fn shutdown(&self) {
        if self.dead.swap(true, Ordering::SeqCst) {
            return;
        }
        let (tx, rx) = channel();
        // Push the command directly (the `dead` flag is already set, so it does not go through `post`).
        self.cmds.lock().unwrap_or_else(|e| e.into_inner()).push_back(Cmd::Shutdown(tx));
        // SAFETY: same as `post`.
        let posted = unsafe { PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_DRAIN, WPARAM(0), LPARAM(0)) }.is_ok();
        let answered = posted && rx.recv_timeout(REPLY_TIMEOUT).is_ok();
        if let Some(t) = self.thread.lock().unwrap_or_else(|e| e.into_inner()).take() {
            if answered {
                let _ = t.join();
            } else {
                logln!("the wallpaper engine did not shut down in time; detaching its thread");
            }
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ------------------------------------------------------------------ engine thread

fn run(
    paths: WallpaperPaths,
    cfg: WallpaperConfig,
    cmds: CmdQueue,
    events: Box<dyn Fn(EngineEvent)>,
    ready: Sender<Result<isize, String>>,
) {
    // SAFETY: dedicated thread; every window/COM object belongs to this thread and is cleaned up before it exits.
    unsafe {
        let _ = OleInitialize(None);
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(main_proc),
            hInstance: hinst.into(),
            lpszClassName: MAIN_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        // A hidden top-level window (not message-only) to receive WM_DISPLAYCHANGE and "TaskbarCreated".
        let hwnd = match CreateWindowExW(
            WS_EX_TOOLWINDOW,
            MAIN_CLASS,
            w!("ShunguangDecor Wallpaper"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst.into()),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(format!("cannot create the engine window: {e}")));
                OleUninitialize();
                return;
            }
        };
        MAIN_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        TASKBAR_CREATED.with(|c| c.set(RegisterWindowMessageW(w!("TaskbarCreated"))));
        let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
        CMDS.with(|c| *c.borrow_mut() = Some(cmds));

        let interval = cfg.poll_interval_ms;
        let app = App {
            hwnd,
            paths: paths.clone(),
            desktop: DesktopLayer::locate(),
            monitors: sg_desktop::monitors(),
            cfg,
            active: Vec::new(),
            web_ctx: wry::WebContext::new(Some(paths.webview.clone())),
            locked: false,
            manual_pause: false,
            display_changed_at: None,
            trim_at: None,
            errors: BTreeMap::new(),
            last_error: None,
            last_state: None,
            events,
            down: false,
            picture_cache: None,
            explorer: {
                let mut t = IdentityTracker::new();
                t.seed(ExplorerIdentity::measure(desktop_identity()));
                t
            },
        };
        logln!("engine started: {} monitors, desktop {}", app.monitors.len(), if app.desktop.is_some() { "OK" } else { "NOT FOUND" });
        APP.with(|a| *a.borrow_mut() = Some(app));
        SetTimer(Some(hwnd), TIMER_TICK, interval, None);
        let _ = ready.send(Ok(hwnd.0 as isize));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        APP.with(|a| {
            if let Some(mut app) = a.borrow_mut().take() {
                app.close_all();
            }
        });
        // Commands left in the pending queue: drop them so callers do not wait in vain (the Sender is dropped → recv errors immediately).
        CMDS.with(|c| *c.borrow_mut() = None);
        let _ = WTSUnRegisterSessionNotification(hwnd);
        let _ = DestroyWindow(hwnd);
        if MAIN_HWND.load(Ordering::Relaxed) == hwnd.0 as isize {
            MAIN_HWND.store(0, Ordering::Relaxed);
        }
        OleUninitialize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(dv: (isize, u32), bars: &[isize]) -> ExplorerIdentity {
        ExplorerIdentity { desktop: Some(dv), taskbars: bars.to_vec() }
    }

    #[test]
    fn taskbar_created_from_another_app_does_not_rebuild() {
        let mut t = IdentityTracker::new();
        t.seed(ident((100, 7), &[200]));
        // The app's taskbar engine broadcasts TaskbarCreated: Explorer is unchanged → do not rebuild.
        assert!(!taskbar_created_means_restart(&mut t, ident((100, 7), &[200]), false));
    }

    #[test]
    fn taskbar_created_with_new_explorer_rebuilds_once() {
        let mut t = IdentityTracker::new();
        t.seed(ident((100, 7), &[200]));
        assert!(taskbar_created_means_restart(&mut t, ident((150, 9), &[300]), false));
        // The marker has moved to the new Explorer: the next broadcast no longer rebuilds.
        assert!(!taskbar_created_means_restart(&mut t, ident((150, 9), &[300]), false));
    }

    #[test]
    fn simulated_taskbar_created_always_rebuilds() {
        let mut t = IdentityTracker::new();
        t.seed(ident((100, 7), &[200]));
        assert!(taskbar_created_means_restart(&mut t, ident((100, 7), &[200]), true));
    }

    /// Manual run (needs a real desktop): spawn with an empty configuration then shut down, must finish within 2 seconds; a second shutdown does not error.
    #[test]
    #[ignore]
    fn engine_spawn_shutdown_real() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let paths = WallpaperPaths {
            library: root.join("lib"),
            properties: root.join("props"),
            thumbs: root.join("thumbs"),
            webview: root.join("web"),
            backup_file: root.join("backup.toml"),
        };
        let h = EngineHandle::spawn(paths, WallpaperConfig::default(), |_| {}).expect("spawn");
        h.apply_config(WallpaperConfig::default()).unwrap();
        let st = h.state().unwrap();
        assert!(st.running.is_empty() && !st.manual_pause);
        assert!(h.toggle_pause().unwrap());
        let t = Instant::now();
        h.shutdown();
        assert!(t.elapsed() < Duration::from_secs(2), "shutdown mất {:?}", t.elapsed());
        h.shutdown();
        assert!(h.state().is_err(), "sau shutdown mọi lệnh báo lỗi, không treo");
    }
}
