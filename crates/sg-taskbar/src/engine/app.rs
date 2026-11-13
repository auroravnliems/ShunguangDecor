//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! Application: builds the bar on each monitor from the config, forwards events to widgets,
//! manages popups (one popup at a time, like `PopupWidget`) and tooltips. The config + stylesheet are supplied by the
//! library (`new` / `apply`); it reads no files and has no tray of its own.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{DWMWCP_ROUND, DWMWCP_ROUNDSMALL};

use sg_core::i18n::Lang;

use crate::engine::TaskbarEvent;
use crate::engine::heal;
use crate::engine::config::{BarConfig, RootConfig};
use crate::engine::css::{Color, Style, Stylesheet};
use crate::engine::gfx::Gfx;
use crate::engine::layer::{self, Fade, Layer};
use crate::engine::sys;
use crate::engine::thumb::Thumb;
use crate::engine::ui::tree::*;
use crate::engine::widgets::{self, Cx, Ev, MenuEntry, PopupSpec, Req, SysEv, Widget};
use crate::engine::win::{self, MouseBtn, WinEvent};
use crate::reveal::{self, AutoHideInput, Rect, RevealState};

thread_local! {
    static EXIT: Cell<bool> = const { Cell::new(false) };
}

pub fn request_exit() {
    EXIT.with(|e| e.set(true));
}

const T_ANIM: usize = 1;
const T_TOOLTIP: usize = 2;
const T_DISPLAY: usize = 4;
const T_HOVER: usize = 5;
/// self-check that the AppBar still holds the correct work area (see `check_appbars`)
const T_APPBAR: usize = 6;
/// first check after building (Explorer needs a moment to update the work area) and the period of later checks (ms)
const APPBAR_FIRST_MS: u32 = 1500;
/// SPI_SETWORKAREA / ABN_POSCHANGED only schedule a single check after this many ms (a new event resets the schedule)
const T_APPBAR_DEBOUNCE: usize = 7;
const APPBAR_DEBOUNCE_MS: u32 = 400;
/// after our own AppBar operation, skip checks for this many ms
const APPBAR_GRACE_MS: u64 = 1000;
const APPBAR_EVERY_MS: u32 = 3000;
/// pointer polling for the auto-hide bar (only runs when some bar has `auto_hide` on)
const T_AUTOHIDE: usize = 8;
const AUTOHIDE_POLL_MS: u32 = 40;
/// re-check "is the foreground window fullscreen" (only runs when some bar has `hide_on_fullscreen` on); foreground-change
/// events also trigger an immediate check, this timer catches windows that get resized after already being foreground
const T_FULLSCREEN: usize = 9;
const FULLSCREEN_POLL_MS: u32 = 500;
/// reposition the AppBar once more after `replace_appbars`: Explorer updates the native taskbar's reserved area slightly late after it changes mode
const T_REPLACE: usize = 10;
const REPLACE_AGAIN_MS: u32 = 500;
/// how long the mouse must hover still before the thumbnail shows (ms)
const HOVER_MS: u32 = 500;
const T_WIDGET: usize = 1000;

/// owner of the right-click menu on the bar (belongs to no widget)
const BAR_OWNER: usize = usize::MAX - 1;
const BM_TASKMGR: u32 = 1;
const BM_OPEN_APP: u32 = 2;
const BM_TURN_OFF: u32 = 3;

pub const N_AUDIO: usize = 1;
pub const N_AUDIO_DEVICES: usize = 2;
pub const N_NOTIFICATIONS: usize = 3;
/// a command from the library (reload/shutdown) is already in the engine thread's command queue
pub const N_CMD: usize = 4;
pub const N_SYSTRAY: usize = 5;

struct Slot {
    w: Box<dyn Widget>,
    bar: usize,
}

struct Bar {
    name: String,
    cfg: BarConfig,
    mon: win::Monitor,
    layer: Layer,
    sections: [Vec<usize>; 3],
    appbar: bool,
    /// the registered AppBar geometry (if it changes → must re-register, not just ABM_SETPOS)
    geom: heal::AppBarGeometry,
    /// the rectangle returned by the latest `ABM_SETPOS` (where the system actually placed the AppBar); `None` when not an AppBar
    appbar_rc: Option<heal::Area>,
    /// the geometry that was repositioned because the bar overlapped the native taskbar; the same geometry still overlapping isn't redone (avoids endless loops)
    overlap_fixed: Option<heal::AppBarGeometry>,
    size_logical: (f32, f32),
    /// currently hidden because a fullscreen foreground window is on this monitor (`hide_on_fullscreen`)
    fs_hidden: bool,
    /// auto-hide bar state (`auto_hide`): `visible` = currently shown
    auto: RevealState,
}

struct Popup {
    owner: usize,
    bar: usize,
    spec: PopupSpec,
    menu: Option<Vec<MenuEntry>>,
    at: Option<(i32, i32)>,
    layer: Layer,
    anchor: (i32, i32, i32, i32),
    closing: bool,
}

/// A node with `on_hover` currently under the mouse (on a bar).
struct Hover {
    hwnd: isize,
    key: u64,
    owner: usize,
    id: u32,
    bar: usize,
    /// the node's on-screen rectangle when hovering started
    anchor: (i32, i32, i32, i32),
    fired: bool,
}

/// A node with `on_hover` found under the mouse.
struct HoverNode {
    key: u64,
    owner: usize,
    id: u32,
    anchor: (i32, i32, i32, i32),
}

struct Tip {
    layer: Layer,
    text: String,
    /// (window, node key) waiting to show / being shown
    target: Option<(isize, u64)>,
    pending: Option<(isize, u64, String)>,
    shown: bool,
    /// after a click, don't show that node's tooltip again until the mouse leaves (like Qt)
    blocked: Option<(isize, u64)>,
}

pub struct App {
    cfg: RootConfig,
    sheet: Stylesheet,
    gfx: Gfx,
    root_style: Style,
    bars: Vec<Bar>,
    slots: Vec<Slot>,
    popup: Option<Popup>,
    tip: Tip,
    lang: Lang,
    events: Rc<dyn Fn(TaskbarEvent)>,
    /// bars from the previous build, waiting to be reused (same name + monitor) on `apply`
    reuse: Vec<Bar>,
    /// popup closed by clicking its owner widget → ignore that widget's next mouse release
    swallow_click: Option<usize>,
    anim_on: bool,
    menu_bar: usize,
    hover: Option<Hover>,
    /// thumbnail popup currently shown (unregisters + closes the window when dropped)
    thumb: Option<Thumb>,
    /// Explorer just restarted: the next build re-registers the AppBar (ABM_REMOVE then ABM_NEW) even if the window is reused.
    force_appbar: bool,
    /// guards against repeated AppBar self-healing: at most 4 times per minute
    heal_limiter: heal::HealLimiter,
    heal_gate: heal::HealGate,
    epoch: Instant,
    /// the monitor that has a fullscreen foreground window (computed while building bars, see `fullscreen_monitor_now`)
    fs_monitor: Option<isize>,
}

/// Text in the bar's right-click menu by app language (languages without a translation use English).
pub(crate) fn menu_text(lang: Lang, key: u32) -> &'static str {
    let vi = lang == Lang::Vi;
    match key {
        BM_TASKMGR => if vi { "Trình quản lý tác vụ" } else { "Task Manager" },
        BM_OPEN_APP => if vi { "Mở ShunguangDecor" } else { "Open ShunguangDecor" },
        _ => if vi { "Tắt thanh này" } else { "Turn this bar off" },
    }
}

impl App {
    pub fn new(lang: Lang, events: Rc<dyn Fn(TaskbarEvent)>) -> Result<App, String> {
        let gfx = Gfx::new().map_err(|e| format!("không khởi tạo được Direct2D/DirectWrite: {e}"))?;
        let tip_hwnd = win::create_layered(win::Kind::Tooltip, "YasbTooltip");
        let mut app = App {
            cfg: RootConfig::default(),
            sheet: Stylesheet::default(),
            gfx,
            root_style: Style::default(),
            bars: Vec::new(),
            slots: Vec::new(),
            popup: None,
            tip: Tip { layer: Layer::new(tip_hwnd, 1.0), text: String::new(), target: None, pending: None, shown: false, blocked: None },
            lang,
            events,
            reuse: Vec::new(),
            swallow_click: None,
            anim_on: false,
            menu_bar: 0,
            hover: None,
            thumb: None,
            force_appbar: false,
            heal_limiter: heal::HealLimiter::new(4, 60_000),
            heal_gate: heal::HealGate::new(APPBAR_GRACE_MS),
            epoch: Instant::now(),
            fs_monitor: None,
        };
        app.refresh_theme();
        Ok(app)
    }

    fn refresh_theme(&mut self) {
        self.gfx.dark = win::apps_use_dark();
        if let Some(c) = win::accent_color(self.gfx.dark) {
            self.gfx.accent = c;
        }
        self.root_style = Style::default();
        self.root_style.color = if self.gfx.dark { Color::WHITE } else { Color::BLACK };
    }

    // ------------------------------------------------------------------ load / build

    /// Rebuild the bars from the new config + stylesheet. Bar windows with the same name and monitor are reused
    /// (keeping their AppBar registration) so the monitor's work area doesn't flicker.
    /// Like `apply` but re-registers the AppBar of every bar (an Explorer restart loses the work area that was reserved).
    pub fn apply_reregistering_appbar(&mut self, cfg: RootConfig, sheet: Stylesheet) {
        self.force_appbar = true;
        self.apply(cfg, sheet);
        self.force_appbar = false;
    }

    pub fn apply(&mut self, cfg: RootConfig, sheet: Stylesheet) {
        crate::engine::log::info(&format!("Nạp cấu hình thanh ({} luật CSS)", sheet.rule_count()));
        self.sheet = sheet;
        self.close_popup(false);
        self.hide_tooltip(true);
        self.clear_hover();
        for (i, _) in self.slots.iter().enumerate() {
            win::kill_timer(T_WIDGET + i);
        }
        self.slots.clear();
        self.reuse = std::mem::take(&mut self.bars);
        self.cfg = cfg;
        self.build_bars();
        for b in self.reuse.drain(..) {
            if b.appbar {
                win::appbar_remove(b.layer.hwnd);
            }
            win::destroy(b.layer.hwnd);
        }
        for b in &mut self.bars {
            b.layer.dirty = true;
        }
    }

    /// Self-check and self-heal: the AppBar is valid while the monitor's work area reserves exactly the bar's share. If not
    /// (Explorer recomputed the work area, ABM_NEW was rejected during build...), re-register + reposition.
    /// Called after every build (first timer), on `SPI_SETWORKAREA`, `ABN_POSCHANGED`, and on the slow timer.
    fn check_appbars(&mut self, why: &str) {
        let now = self.epoch.elapsed().as_millis() as u64;
        if !self.heal_gate.may_check(now) {
            return;
        }
        // The native taskbar RESERVES space (not auto-hidden) yet overlaps the bar: our AppBar was placed wrongly (usually because it was placed while the native taskbar
        // was still auto-hidden) even though the work area is still "enough" since the native taskbar itself reserves it. A native taskbar that auto-hides (including when we turn it on in
        // Hide/reveal mode) reserves no space so it doesn't count, otherwise the bar would be repositioned every time the native taskbar appears.
        let natives_all: Vec<heal::Area> = if self.bars.iter().any(|b| b.appbar) && sg_core::platform::taskbar_autohide() == Some(false) {
            sg_core::platform::native_taskbars()
                .into_iter()
                .filter(|t| win::is_visible(HWND(t.hwnd as *mut _)))
                .map(|t| heal::Area::new(t.rect.0, t.rect.1, t.rect.2, t.rect.3))
                .collect()
        } else {
            Vec::new()
        };
        for i in 0..self.bars.len() {
            let b = &self.bars[i];
            if !b.appbar {
                continue;
            }
            let Some(work) = win::work_area_of(b.mon.rect) else { continue };
            let g = b.geom;
            // "in the right place" = the work area reserves all the way up to the rectangle ABM_SETPOS returned (keep mode: sits above the
            // native taskbar), not just the raw `reserve`, because the native taskbar also reserves a strip
            let reserve = b.appbar_rc.map_or(g.reserve, |rc| heal::effective_reserve(g.monitor, rc, g.top_edge));
            let win_area = heal::Area::new(b.layer.pos.0, b.layer.pos.1, b.layer.pos.0 + b.layer.size.0, b.layer.pos.1 + b.layer.size.1);
            // only a native taskbar on the same monitor + the same horizontal edge can "overlap" the bar
            let natives = heal::natives_on_same_edge(&natives_all, g.monitor, g.top_edge);
            let lost_work_area = heal::needs_heal(g.monitor, area_of(work), g.top_edge, reserve);
            let overlapping = heal::overlaps_any(win_area, &natives);
            if !lost_work_area && !overlapping {
                self.bars[i].overlap_fixed = None;
                continue;
            }
            if !lost_work_area {
                // overlap only: reposition ONCE per geometry, without re-registering (the work area isn't touched);
                // if it still overlaps after repositioning, stop, don't loop
                if self.bars[i].overlap_fixed == Some(g) {
                    continue;
                }
                self.bars[i].overlap_fixed = Some(g);
                crate::engine::log::warn(&format!("Thanh '{}' đè taskbar gốc ({why}) → đặt lại vị trí (không đăng ký lại)", self.bars[i].name));
                self.place_appbar(i);
                self.heal_gate.note_own_operation(self.epoch.elapsed().as_millis() as u64);
                continue;
            }
            if !self.heal_limiter.allow(now) {
                crate::engine::log::warn(&format!("AppBar '{}' mất vùng làm việc ({why}) nhưng đã chữa quá nhiều lần trong 1 phút, tạm bỏ qua", b.name));
                continue;
            }
            let hwnd = b.layer.hwnd;
            crate::engine::log::warn(&format!(
                "AppBar '{}' mất vùng làm việc ({why}): màn hình {:?}, rcWork bottom={} top={}, cần chừa {} px → đăng ký lại",
                b.name, g.monitor, work.bottom, work.top, reserve
            ));
            if !win::appbar_reregister(hwnd) {
                crate::engine::log::warn("không đăng ký lại được AppBar (ABM_NEW bị từ chối)");
            }
            self.place_appbar(i);
            self.heal_gate.note_own_operation(self.epoch.elapsed().as_millis() as u64);
        }
    }

    /// Reposition the AppBar of bar `bi` (`ABM_QUERYPOS` + `ABM_SETPOS`) then move the window to EXACTLY the rectangle the system
    /// returned, remembering it for `check_appbars`. Native taskbar "keep" mode: the system places the bar above the native taskbar so the window
    /// must sit there too (it used to always be `mon.bottom - h`, hidden behind the native taskbar).
    fn place_appbar(&mut self, bi: usize) {
        let Some(b) = self.bars.get_mut(bi) else { return };
        let hwnd = b.layer.hwnd;
        let rc = win::appbar_set_pos(hwnd, b.geom.top_edge, b.mon.rect, b.geom.reserve);
        let area = area_of(rc);
        let (pad_top, pad_bottom) = pad_px(&b.cfg, b.mon.scale);
        b.appbar_rc = Some(area);
        b.layer.pos.1 = heal::bar_y(b.geom.top_edge, area, pad_top, pad_bottom, b.layer.size.1);
        b.layer.dirty = true;
        win::move_window(hwnd, b.layer.pos.0, b.layer.pos.1, b.layer.size.0, b.layer.size.1, bar_topmost(&b.cfg));
        win::appbar_window_pos_changed(hwnd);
    }

    // ------------------------------------------------------------------ hide on fullscreen / auto-hide

    /// Should the bar be shown: not covered by a fullscreen window and (if auto-hide) the mouse has summoned it.
    fn want_visible(b: &Bar) -> bool {
        !b.fs_hidden && (!b.cfg.window_flags.auto_hide || b.auto.visible)
    }

    /// Bring bar `bi`'s window to the desired shown/hidden state. When hiding, close its popup/tooltip.
    fn sync_visibility(&mut self, bi: usize) {
        let Some(b) = self.bars.get(bi) else { return };
        let want = Self::want_visible(b);
        if want == b.layer.visible {
            return;
        }
        if !want {
            if self.popup.as_ref().is_some_and(|p| p.bar == bi) {
                self.close_popup(false);
            }
            self.hide_tooltip(true);
            self.clear_hover();
        }
        let b = &mut self.bars[bi];
        if want {
            // an auto-hide bar is always topmost when shown (like Windows' auto-hide taskbar)
            win::move_window(b.layer.hwnd, b.layer.pos.0, b.layer.pos.1, b.layer.size.0, b.layer.size.1, bar_topmost(&b.cfg));
            win::show_noactivate(b.layer.hwnd);
            b.layer.visible = true;
            b.layer.dirty = true;
        } else {
            win::hide(b.layer.hwnd);
            b.layer.visible = false;
            b.layer.mouse = None;
        }
    }

    /// The monitor that has a fullscreen foreground window (only reads the foreground window, never touches the game process).
    fn fullscreen_monitor_now() -> Option<isize> {
        let (mon, fg) = win::foreground_window()?;
        // QUNS is only queried when the window could be truly fullscreen (not excluded, covers the screen; maximized windows are queried too
        // but only D3D/presentation counts, see `is_fullscreen_foreground`)
        let none = reveal::NotifyState::None;
        let full = reveal::is_fullscreen_foreground(Some(&fg), none)
            || (reveal::should_query_quns(Some(&fg)) && reveal::is_fullscreen_foreground(Some(&fg), win::notification_state()));
        full.then_some(mon)
    }

    /// Re-check fullscreen: hide the bar (`hide_on_fullscreen`) on a monitor with a fullscreen window, show it again when it ends.
    /// The AppBar stays as is (the work area isn't released) so leaving fullscreen only needs to show it again, with no layout flicker.
    fn refresh_fullscreen(&mut self) {
        if !self.bars.iter().any(|b| b.cfg.window_flags.hide_on_fullscreen) {
            return;
        }
        self.fs_monitor = Self::fullscreen_monitor_now();
        for bi in 0..self.bars.len() {
            let b = &mut self.bars[bi];
            let hide = b.cfg.window_flags.hide_on_fullscreen && self.fs_monitor == Some(b.mon.handle);
            if b.fs_hidden != hide {
                b.fs_hidden = hide;
                crate::engine::log::info(&format!("Thanh '{}': {} (cửa sổ toàn màn hình)", b.name, if hide { "ẩn" } else { "hiện lại" }));
                self.sync_visibility(bi);
            }
        }
    }

    /// One pointer-polling tick for the auto-hide bars.
    fn tick_auto_hide(&mut self) {
        let cursor = win::cursor_pos();
        let now = self.epoch.elapsed().as_millis() as u64;
        // Start/Search opened from the bar's button is a shell flyout, not one of the bar's popups: still counts as "popup open"
        // so the bar doesn't vanish from under the user's hands
        let shell_flyout = self.bars.iter().any(|b| b.cfg.window_flags.auto_hide) && win::foreground_is_shell_flyout();
        let thumb_open = self.thumb.is_some() || shell_flyout;
        for bi in 0..self.bars.len() {
            let b = &self.bars[bi];
            if !b.cfg.window_flags.auto_hide {
                continue;
            }
            let next = if b.fs_hidden {
                RevealState::HIDDEN
            } else {
                let top = b.geom.top_edge;
                let bar = Rect { left: b.layer.pos.0, top: b.layer.pos.1, right: b.layer.pos.0 + b.layer.size.0, bottom: b.layer.pos.1 + b.layer.size.1 };
                let mon = Rect { left: b.mon.rect.left, top: b.mon.rect.top, right: b.mon.rect.right, bottom: b.mon.rect.bottom };
                let input = AutoHideInput {
                    cursor_on_edge: reveal::cursor_on_bar_edge(cursor, bar, mon, top),
                    cursor_over_bar: reveal::auto_hide_zone(bar, mon, top).contains(cursor.0, cursor.1),
                    popup_open: thumb_open || self.popup.as_ref().is_some_and(|p| p.bar == bi),
                    now_ms: now,
                };
                reveal::auto_hide_step(b.auto, input)
            };
            if next != b.auto {
                self.bars[bi].auto = next;
                self.sync_visibility(bi);
            }
        }
    }

    /// Turn the two timers on/off depending on whether any bar needs them (if none does, they cost nothing).
    fn update_visibility_timers(&mut self) {
        let any = |f: fn(&BarConfig) -> bool| self.bars.iter().any(|b| f(&b.cfg));
        if any(|c| c.window_flags.auto_hide) {
            win::set_timer(T_AUTOHIDE, AUTOHIDE_POLL_MS);
        } else {
            win::kill_timer(T_AUTOHIDE);
        }
        if any(|c| c.window_flags.hide_on_fullscreen) {
            win::set_timer(T_FULLSCREEN, FULLSCREEN_POLL_MS);
        } else {
            win::kill_timer(T_FULLSCREEN);
        }
    }

    /// Reposition every AppBar (the native taskbar just changed mode). Do it now, then again after `REPLACE_AGAIN_MS` because the native
    /// taskbar's reserved area may not have updated yet the first time.
    pub fn replace_appbars(&mut self) {
        self.replace_all_appbars();
        if self.bars.iter().any(|b| b.appbar) {
            win::set_timer(T_REPLACE, REPLACE_AGAIN_MS);
        }
    }

    fn replace_all_appbars(&mut self) {
        for bi in 0..self.bars.len() {
            if self.bars[bi].appbar {
                self.place_appbar(bi);
            }
        }
        self.heal_gate.note_own_operation(self.epoch.elapsed().as_millis() as u64);
    }

    fn teardown(&mut self) {
        win::kill_timer(T_REPLACE);
        win::kill_timer(T_AUTOHIDE);
        win::kill_timer(T_FULLSCREEN);
        self.close_popup(false);
        self.hide_tooltip(true);
        self.clear_hover();
        for (i, _) in self.slots.iter().enumerate() {
            win::kill_timer(T_WIDGET + i);
        }
        win::kill_timer(T_APPBAR);
        win::kill_timer(T_APPBAR_DEBOUNCE);
        for b in self.bars.drain(..) {
            if b.appbar {
                win::appbar_remove(b.layer.hwnd);
            }
            win::destroy(b.layer.hwnd);
        }
        self.slots.clear();
    }

    fn build_bars(&mut self) {
        let mons = win::monitors();
        self.fs_monitor = Self::fullscreen_monitor_now();
        let order = self.cfg.bar_order.clone();
        for name in order {
            let Some(bc) = self.cfg.bars.get(&name).cloned() else { continue };
            if !bc.enabled {
                continue;
            }
            for m in &mons {
                let wanted = bc.screens.iter().any(|s| {
                    let s = s.trim();
                    s == "*" || s.eq_ignore_ascii_case(&m.device) || (s.eq_ignore_ascii_case("primary") && m.primary)
                });
                if !wanted {
                    continue;
                }
                self.create_bar(&name, bc.clone(), m.clone());
            }
        }
        if self.bars.is_empty() {
            crate::engine::log::warn("Không có thanh bar nào được bật cho màn hình hiện tại.");
        }
        if self.bars.iter().any(|b| b.appbar) {
            win::set_timer(T_APPBAR, APPBAR_FIRST_MS);
        } else {
            win::kill_timer(T_APPBAR);
        }
        self.update_visibility_timers();
        sys::notifications::ensure_started(self.has_notifications_widget());
        for i in 0..self.slots.len() {
            self.dispatch(i, Ev::Init);
            let iv = self.slots[i].w.interval();
            if iv > 0 {
                win::set_timer(T_WIDGET + i, iv);
            }
        }
    }

    fn has_notifications_widget(&self) -> bool {
        self.cfg.widgets.values().any(|w| w.type_name.ends_with("NotificationsWidget"))
    }

    fn create_bar(&mut self, name: &str, bc: BarConfig, mon: win::Monitor) {
        let topmost = bar_topmost(&bc);
        let auto_hide = bc.window_flags.auto_hide;
        let top = bc.alignment.position != "bottom";
        let reused = self
            .reuse
            .iter()
            // Changing edge still reuses the window: `moved` below unregisters + re-registers the AppBar (experiment round 4: the same HWND switching top ↔
            // bottom still gets the correct work area reserved by Explorer; the earlier measured bug was caused by the fake `Shell_TrayWnd`, see appbar.rs)
            .position(|b| b.name == name && b.mon.handle == mon.handle && bar_topmost(&b.cfg) == topmost)
            .map(|i| self.reuse.remove(i));
        let reused_window = reused.is_some();
        let (hwnd, had_appbar) = match &reused {
            Some(b) => (b.layer.hwnd, b.appbar),
            None => (win::create_layered(win::Kind::Bar { topmost }, win::APP_TITLE), false),
        };
        let bar_idx = self.bars.len();
        let mut sections: [Vec<usize>; 3] = Default::default();
        for (si, list) in [&bc.widgets.left, &bc.widgets.center, &bc.widgets.right].into_iter().enumerate() {
            for wname in list {
                let Some(def) = self.cfg.widgets.get(wname) else {
                    crate::engine::log::warn(&format!("Thanh '{name}': không có widget '{wname}' trong mục widgets."));
                    continue;
                };
                if let Some(w) = widgets::create(wname, &def.type_name, &def.options) {
                    sections[si].push(self.slots.len());
                    self.slots.push(Slot { w, bar: bar_idx });
                }
            }
        }
        let mut layer = Layer::new(hwnd, mon.scale);
        let s = mon.scale;
        let mr = mon.rect;
        let sw = mr.right - mr.left;
        let pad = &bc.padding;
        let h = (bc.dimensions.height as f32 * s).round() as i32;
        let avail = sw - ((pad.left + pad.right) as f32 * s) as i32;
        let w = bc.width_px((sw as f32 / s) as i32).map(|w| (w as f32 * s) as i32).unwrap_or(avail).min(avail);
        let x = match bc.alignment.align.as_str() {
            "left" => mr.left + (pad.left as f32 * s) as i32,
            "right" => (mr.right - w - (pad.right as f32 * s) as i32).max(mr.left + (pad.left as f32 * s) as i32),
            _ => mr.left + (pad.left as f32 * s) as i32 + (avail - w) / 2,
        };
        let (pad_top, pad_bottom) = pad_px(&bc, s);
        let mut y = if top { mr.top + pad_top } else { mr.bottom - h - pad_bottom };
        layer.size = (w, h);
        let mut appbar = false;
        let mut appbar_rc = None;
        let reserve = h + ((pad.top + pad.bottom) as f32 * s) as i32;
        let geom = heal::AppBarGeometry { monitor: area_of(mr), top_edge: top, reserve };
        // an auto-hide bar reserves no space (the work area isn't reduced): it only shows when the mouse touches the edge
        if bc.window_flags.windows_app_bar && !auto_hide {
            // When a reused window's edge/height/monitor changes, ABM_SETPOS alone isn't reliable enough (measured: the
            // bar was drawn at 48 px but the work area stayed at the 36 px level) → re-register from scratch.
            let moved = reused.as_ref().is_some_and(|b| b.appbar && b.geom != geom);
            if had_appbar && (self.force_appbar || moved) {
                win::appbar_remove(hwnd);
            }
            if (!had_appbar || self.force_appbar || moved) && !win::appbar_reregister(hwnd) {
                crate::engine::log::warn(&format!("Thanh '{name}': Explorer từ chối đăng ký AppBar (ABM_NEW)"));
            }
            // Move the window to exactly the rectangle the system returned (keep mode: above the native taskbar), don't compute y ourselves.
            let rc = area_of(win::appbar_set_pos(hwnd, top, mr, reserve));
            y = heal::bar_y(top, rc, pad_top, pad_bottom, h);
            appbar_rc = Some(rc);
            appbar = true;
            self.heal_gate.note_own_operation(self.epoch.elapsed().as_millis() as u64);
        } else if had_appbar {
            win::appbar_remove(hwnd);
        }
        layer.pos = (x, y);
        win::move_window(hwnd, x, y, w, h, topmost);
        if appbar {
            win::appbar_window_pos_changed(hwnd);
        }
        let fs_hidden = bc.window_flags.hide_on_fullscreen && self.fs_monitor == Some(mon.handle);
        let anim = bc.animation.enabled && reused.is_none() && !auto_hide;
        layer.alpha = if anim { 0.0 } else { 1.0 };
        let size_logical = (w as f32 / s, h as f32 / s);
        self.bars.push(Bar { name: name.to_string(), cfg: bc, mon, layer, sections, appbar, geom, appbar_rc, overlap_fixed: None, size_logical, fs_hidden, auto: RevealState::HIDDEN });
        self.render_bar(bar_idx);
        let b = &mut self.bars[bar_idx];
        if b.cfg.blur_effect.enabled {
            let be = &b.cfg.blur_effect;
            // styles are only computed after `render_bar`: the bar's background color is used as the acrylic tint
            let tint = be.acrylic.then(|| win::acrylic_tint(b.layer.tree.nodes.iter().find(|n| n.classes.starts_with("bar ")).and_then(|n| n.style.background)));
            win::enable_blur(b.layer.hwnd, be.dark_mode, be.round_corners, be.round_corners_type == "small", &be.border_color, tint);
        }
        // an auto-hide bar starts hidden; so does a bar currently covered by a fullscreen game
        match heal::initial_window_action(Self::want_visible(b), reused_window) {
            heal::WindowAction::Show => {
                win::show_noactivate(b.layer.hwnd);
                b.layer.visible = true;
            }
            // a reused window may be showing on a monitor where the new config wants it hidden
            heal::WindowAction::Hide => win::hide(b.layer.hwnd),
            heal::WindowAction::Nothing => {}
        }
        if anim {
            // the first show uses a fade (YASB avoids DWM blur flicker while sliding)
            // slide in from the screen edge together with a fade-in
            let dy = if b.cfg.alignment.position == "bottom" { 14.0 } else { -14.0 } * b.mon.scale;
            b.layer.slide = dy;
            b.layer.fade = Some(Fade { from: 0.0, to: 1.0, start: Instant::now(), ms: b.cfg.animation.duration as f32, ease_out: true, slide: Some((dy, 0.0)) });
            self.ensure_anim();
        }
    }

    fn build_bar_tree(slots: &[Slot], bar: &Bar, tree: &mut Tree) {
        let mut ui = Ui::new(tree, usize::MAX);
        let root = Node::new("Widget", "");
        let class = format!("bar {}", bar.cfg.class_name);
        let lay = [&bar.cfg.layouts.left, &bar.cfg.layouts.center, &bar.cfg.layouts.right];
        ui.with(root, |ui| {
            ui.row(&class, |ui| {
                for (si, name) in ["left", "center", "right"].iter().enumerate() {
                    let mut c = Node::new("QFrame", &format!("container container-{name}"));
                    c.dir = Dir::Row;
                    c.hpolicy = Policy::Expanding;
                    let l = lay[si];
                    ui.with(c, |ui| {
                        if l.stretch && (l.alignment == "right" || l.alignment == "center") {
                            ui.stretch();
                        }
                        for &wi in &bar.sections[si] {
                            ui.owner = wi;
                            slots[wi].w.view(ui);
                        }
                        ui.owner = usize::MAX;
                        if l.stretch && (l.alignment == "left" || l.alignment == "center") {
                            ui.stretch();
                        }
                    });
                }
            });
        });
    }

    fn render_bar(&mut self, bi: usize) {
        let App { bars, slots, gfx, sheet, root_style, .. } = self;
        let bar = &mut bars[bi];
        let mut tree = std::mem::take(&mut bar.layer.tree);
        Self::build_bar_tree(slots, bar, &mut tree);
        bar.layer.tree = tree;
        let size = bar.size_logical;
        bar.layer.layout(gfx, sheet, root_style, Some(size));
        bar.layer.present(gfx);
    }

    fn render_popup(&mut self) {
        let App { popup, slots, gfx, sheet, root_style, .. } = self;
        let Some(p) = popup.as_mut() else { return };
        let mut tree = std::mem::take(&mut p.layer.tree);
        {
            let mut ui = Ui::new(&mut tree, p.owner);
            if let Some(entries) = &p.menu {
                build_menu(&mut ui, &p.spec.class, entries);
            } else {
                let mut root = Node::new("PopupWidget", &p.spec.class);
                root.dir = p.spec.dir;
                root.layout_margins = p.spec.margins;
                ui.with(root, |ui| slots[p.owner].w.popup_view(p.spec.kind, ui));
            }
        }
        p.layer.tree = tree;
        let (w, h) = p.layer.layout(gfx, sheet, root_style, None);
        let s = p.layer.scale;
        let size = ((w * s).ceil() as i32, (h * s).ceil() as i32);
        if size != p.layer.size || !p.layer.visible {
            p.layer.size = size;
            p.layer.pos = match p.at {
                Some(at) => Self::menu_position(at, size),
                None => Self::popup_position(&p.spec, p.anchor, size, s),
            };
            win::move_window(p.layer.hwnd, p.layer.pos.0, p.layer.pos.1, size.0, size.1, true);
        }
        p.layer.present(gfx);
    }

    /// `PopupWidget.setPosition`: place below/above the owner widget, aligned left/right/center, clamped to the screen.
    fn popup_position(spec: &PopupSpec, anchor: (i32, i32, i32, i32), size: (i32, i32), s: f32) -> (i32, i32) {
        let (ax, ay, aw, ah) = anchor;
        let ol = (spec.opts.offset_left as f32 * s).round() as i32;
        let ot = (spec.opts.offset_top as f32 * s).round() as i32;
        let (mut x, mut y) = (ax + ol, ay + ah + ot);
        if spec.opts.direction == "up" {
            y = ay - size.1 - ot;
            x = ax + ol;
        }
        x = match spec.opts.alignment.as_str() {
            "right" => x + aw - size.0,
            "center" => x + (aw - size.0) / 2,
            _ => x,
        };
        let mon = win::monitors().into_iter().find(|m| {
            let cx = ax + aw / 2;
            let cy = ay + ah / 2;
            cx >= m.rect.left && cx < m.rect.right && cy >= m.rect.top && cy < m.rect.bottom
        });
        if let Some(m) = mon {
            let r = m.rect;
            x = x.clamp(r.left, (r.right - size.0).max(r.left));
            y = y.clamp(r.top, (r.bottom - size.1).max(r.top));
        }
        (x, y)
    }

    /// QMenu opens at the pointer; if it overflows the screen it flips to the left/upwards.
    fn menu_position(at: (i32, i32), size: (i32, i32)) -> (i32, i32) {
        let (mut x, mut y) = at;
        if let Some(m) = win::monitors().into_iter().find(|m| at.0 >= m.rect.left && at.0 < m.rect.right && at.1 >= m.rect.top && at.1 < m.rect.bottom) {
            let r = m.rect;
            if x + size.0 > r.right {
                x = (x - size.0).max(r.left);
            }
            if y + size.1 > r.bottom {
                y = (y - size.1).max(r.top);
            }
        }
        (x, y)
    }

    fn render_tooltip(&mut self) {
        let App { tip, gfx, sheet, root_style, .. } = self;
        let mut tree = std::mem::take(&mut tip.layer.tree);
        {
            let mut ui = Ui::new(&mut tree, usize::MAX);
            let root = Node::new("Widget", "");
            let class = if gfx.dark { "tooltip dark" } else { "tooltip" };
            let text = tip.text.clone();
            ui.with(root, |ui| {
                ui.label(class, text);
            });
        }
        tip.layer.tree = tree;
        let (w, h) = tip.layer.layout(gfx, sheet, root_style, None);
        let s = tip.layer.scale;
        tip.layer.size = ((w * s).ceil() as i32, (h * s).ceil() as i32);
        tip.layer.present(gfx);
    }

    /// Redraw every window marked as changed.
    pub fn flush(&mut self) {
        for i in 0..self.bars.len() {
            if self.bars[i].layer.dirty {
                self.render_bar(i);
            }
        }
        if self.popup.as_ref().is_some_and(|p| p.layer.dirty) {
            self.render_popup();
        }
        if self.tip.shown && self.tip.layer.dirty {
            self.render_tooltip();
        }
        // Safety net: if the state says the tooltip is hidden, the window must not remain WS_VISIBLE (a transparent window would overlap the bar).
        if layer::tooltip_window_stale(self.tip.shown, win::is_visible(self.tip.layer.hwnd)) {
            win::hide(self.tip.layer.hwnd);
            self.tip.layer.visible = false;
        }
        if self.bars.iter().any(|b| b.layer.animating || b.layer.fade.is_some())
            || self.popup.as_ref().is_some_and(|p| p.layer.animating || p.layer.fade.is_some())
            || self.tip.layer.fade.is_some()
            || self.tip.layer.animating
        {
            self.ensure_anim();
        }
    }

    fn ensure_anim(&mut self) {
        if !self.anim_on {
            self.anim_on = true;
            win::set_timer(T_ANIM, 15);
        }
    }

    fn tick_anim(&mut self) {
        let now = Instant::now();
        let mut any = false;
        for b in &mut self.bars {
            if b.layer.fade.is_some() {
                any |= b.layer.update_fade(now);
                b.layer.present_alpha_only(&self.gfx);
            }
            if b.layer.animating {
                b.layer.dirty = true;
                any = true;
            }
        }
        let mut destroy_popup = false;
        if let Some(p) = &mut self.popup {
            if p.layer.fade.is_some() {
                let running = p.layer.update_fade(now);
                any |= running;
                p.layer.present_alpha_only(&self.gfx);
                if !running && p.closing {
                    destroy_popup = true;
                }
            }
            if p.layer.animating {
                p.layer.dirty = true;
                any = true;
            }
        }
        if destroy_popup {
            self.destroy_popup();
        }
        if let Some(target) = self.tip.layer.fade.as_ref().map(|f| f.to) {
            let running = self.tip.layer.update_fade(now);
            any |= running;
            self.tip.layer.present(&self.gfx);
            if layer::fade_finished_hidden(target, running) {
                win::hide(self.tip.layer.hwnd);
                self.tip.layer.visible = false;
                self.tip.shown = false;
            }
        }
        if self.tip.layer.animating {
            self.tip.layer.dirty = true;
            any = true;
        }
        if !any {
            win::kill_timer(T_ANIM);
            self.anim_on = false;
        }
    }

    // ------------------------------------------------------------------ widget

    fn dispatch(&mut self, idx: usize, ev: Ev) {
        if idx == BAR_OWNER {
            if let Ev::Click { id, .. } = ev {
                self.close_popup(true);
                self.bar_menu_action(id);
            }
            return;
        }
        if idx >= self.slots.len() {
            return;
        }
        let bar = self.slots[idx].bar;
        let popup_open = self.popup.as_ref().filter(|p| p.owner == idx && !p.closing).map(|p| p.spec.kind);
        let (monitor, scale) = self.bars.get(bar).map(|b| (b.mon.handle, b.mon.scale)).unwrap_or((0, 1.0));
        let mut cx = Cx { dirty: false, requests: Vec::new(), popup_open, monitor, scale };
        self.slots[idx].w.event(ev, &mut cx);
        if cx.dirty {
            if let Some(b) = self.bars.get_mut(bar) {
                b.layer.dirty = true;
            }
            if let Some(p) = self.popup.as_mut().filter(|p| p.owner == idx) {
                p.layer.dirty = true;
            }
        }
        for r in cx.requests {
            match r {
                Req::OpenPopup(spec) => self.open_popup(idx, spec, None),
                Req::ClosePopup => self.close_popup(true),
                Req::ContextMenu(entries) => self.open_menu(idx, bar, entries),
                Req::Thumbnail(hwnd) => self.open_thumb(idx, hwnd),
            }
        }
    }

    fn broadcast(&mut self, ev: SysEv) {
        for i in 0..self.slots.len() {
            self.dispatch(i, Ev::Sys(ev.clone()));
        }
    }

    // ------------------------------------------------------------------ popup

    fn open_menu(&mut self, owner: usize, bar: usize, entries: Vec<MenuEntry>) {
        let class = if self.gfx.dark { "context-menu dark" } else { "context-menu" };
        let spec = PopupSpec {
            kind: 0x4d45_4e55,
            class: class.into(),
            opts: crate::engine::config::PopupOptions { blur: true, round_corners: true, border_color: "None".into(), ..Default::default() },
            margins: crate::engine::css::Edges::default(),
            dir: Dir::Col,
        };
        self.close_popup(false);
        self.menu_bar = bar;
        self.open_popup(owner, spec, Some(entries));
    }

    fn bar_menu_action(&mut self, id: u32) {
        match id {
            BM_TASKMGR => sys::shell::shell_open("taskmgr", None),
            BM_OPEN_APP => (self.events)(TaskbarEvent::OpenApp),
            BM_TURN_OFF => (self.events)(TaskbarEvent::TurnOff),
            _ => {}
        }
    }

    fn open_popup(&mut self, owner: usize, spec: PopupSpec, menu: Option<Vec<MenuEntry>>) {
        crate::engine::log::debug(&format!("popup: mở '{}' cho widget {owner}", spec.class));
        if let Some(p) = &self.popup {
            if p.owner == owner && p.spec.kind == spec.kind && !p.closing {
                self.close_popup(true);
                return;
            }
            self.close_popup(false);
        }
        let bar = if owner < self.slots.len() { self.slots[owner].bar } else { self.menu_bar };
        let Some(b) = self.bars.get(bar) else { return };
        // anchor rect = the owner widget's root node on the bar (a context menu anchors at the pointer)
        let at = menu.as_ref().map(|_| win::cursor_pos());
        let anchor = match b.layer.tree.nodes.iter().position(|n| n.owner == owner && n.tag == "Widget") {
            Some(root) => b.layer.screen_rect(root),
            None if at.is_some() => (b.layer.pos.0, b.layer.pos.1, b.layer.size.0, b.layer.size.1),
            None => return,
        };
        let hwnd = win::create_layered(win::Kind::Popup, "YasbPopup");
        let mut layer = Layer::new(hwnd, b.mon.scale);
        layer.alpha = 0.0;
        self.popup = Some(Popup { owner, bar, spec: spec.clone(), menu, at, layer, anchor, closing: false });
        self.hide_tooltip(true);
        self.render_popup();
        if spec.opts.blur {
            let small = spec.opts.round_corners_type == "small";
            win::enable_blur(hwnd, false, spec.opts.round_corners && win::windows_build() >= 22000, small, &spec.opts.border_color, None);
        } else if spec.opts.round_corners {
            win::set_corners(hwnd, if spec.opts.round_corners_type == "small" { DWMWCP_ROUNDSMALL } else { DWMWCP_ROUND }, &spec.opts.border_color);
        }
        if let Some(p) = &mut self.popup {
            win::activate(p.layer.hwnd);
            p.layer.visible = true;
            // fade in + a slight slide out from the bar side (8px)
            let dy = if p.spec.opts.direction == "up" { 8.0 } else { -8.0 } * p.layer.scale;
            p.layer.slide = dy;
            p.layer.fade = Some(Fade { from: 0.0, to: 1.0, start: Instant::now(), ms: 170.0, ease_out: true, slide: Some((dy, 0.0)) });
        }
        win::set_mouse_hook(true);
        self.bars[bar].layer.dirty = true;
        self.ensure_anim();
    }

    fn close_popup(&mut self, animated: bool) {
        let Some(p) = &mut self.popup else { return };
        crate::engine::log::debug(&format!("popup: đóng '{}' (animated={animated}, closing={})", p.spec.class, p.closing));
        if animated {
            if p.closing {
                return;
            }
            p.closing = true;
            let from = p.layer.alpha;
            let dy = if p.spec.opts.direction == "up" { 5.0 } else { -5.0 } * p.layer.scale;
            p.layer.fade = Some(Fade { from, to: 0.0, start: Instant::now(), ms: 110.0, ease_out: false, slide: Some((p.layer.slide, dy)) });
            win::set_mouse_hook(false);
            self.ensure_anim();
        } else {
            self.destroy_popup();
        }
    }

    fn destroy_popup(&mut self) {
        if let Some(p) = self.popup.take() {
            win::destroy(p.layer.hwnd);
            win::set_mouse_hook(false);
            let owner = p.owner;
            if let Some(b) = self.bars.get_mut(p.bar) {
                b.layer.dirty = true;
            }
            self.dispatch(owner, Ev::PopupClosed);
        }
    }

    // ------------------------------------------------------------------ tooltip

    fn update_tooltip_target(&mut self, hwnd: isize, layer_tree: Option<(u64, String)>) {
        let key_now = layer_tree.as_ref().map(|(k, _)| (hwnd, *k));
        if self.tip.blocked.is_some() {
            if self.tip.blocked == key_now {
                return;
            }
            self.tip.blocked = None;
        }
        match layer_tree {
            Some((key, text)) => {
                if self.tip.target == Some((hwnd, key)) && self.tip.shown {
                    if self.tip.text != text {
                        self.tip.text = text;
                        self.tip.layer.dirty = true;
                    }
                    return;
                }
                if self.tip.pending.as_ref().is_some_and(|(h, k, _)| *h == hwnd && *k == key) {
                    return;
                }
                if self.tip.shown {
                    self.hide_tooltip(false);
                }
                self.tip.pending = Some((hwnd, key, text));
                win::set_timer(T_TOOLTIP, 400);
            }
            None => {
                self.tip.pending = None;
                win::kill_timer(T_TOOLTIP);
                if self.tip.shown {
                    self.hide_tooltip(false);
                }
            }
        }
    }

    fn show_tooltip(&mut self) {
        win::kill_timer(T_TOOLTIP);
        let Some((hwnd, key, text)) = self.tip.pending.take() else { return };
        // find the node + the window containing it
        let (rect, top_rect, scale) = {
            let layer = if let Some(b) = self.bars.iter().find(|b| b.layer.hwnd.0 as isize == hwnd) {
                &b.layer
            } else if let Some(p) = self.popup.as_ref().filter(|p| p.layer.hwnd.0 as isize == hwnd) {
                &p.layer
            } else {
                return;
            };
            let Some(id) = layer.node_by_key(key) else { return };
            (layer.screen_rect(id), (layer.pos.0, layer.pos.1, layer.size.0, layer.size.1), layer.scale)
        };
        self.tip.text = text;
        self.tip.target = Some((hwnd, key));
        self.tip.layer.scale = scale;
        self.render_tooltip();
        let (tw, th) = self.tip.layer.size;
        let off = (self.cfg.tooltip.offset as f32 * scale) as i32;
        let x = rect.0 + rect.2 / 2 - tw / 2;
        let below = top_rect.1 + top_rect.3 + off;
        let mon = win::monitors().into_iter().find(|m| rect.0 >= m.rect.left && rect.0 < m.rect.right).map(|m| m.rect);
        let (sx0, sx1, sy0, sy1) = mon.map(|r| (r.left, r.right, r.top, r.bottom)).unwrap_or((0, 100_000, 0, 100_000));
        let y = if below + th <= sy1 { below } else { top_rect.1 - th - off };
        let x = x.clamp(sx0, (sx1 - tw).max(sx0));
        let y = y.clamp(sy0, (sy1 - th).max(sy0));
        self.tip.layer.pos = (x, y);
        win::move_window(self.tip.layer.hwnd, x, y, tw, th, true);
        let be = &self.cfg.tooltip.blur_effect;
        if be.enabled {
            win::enable_blur(self.tip.layer.hwnd, be.dark_mode, be.round_corners, be.round_corners_type == "small", &be.border_color, None);
        }
        self.tip.layer.alpha = 0.0;
        self.tip.layer.slide = 10.0;
        self.tip.layer.fade = Some(Fade { from: 0.0, to: 1.0, start: Instant::now(), ms: 200.0, ease_out: true, slide: Some((10.0 * scale, 0.0)) });
        self.tip.layer.present(&self.gfx);
        win::show_noactivate(self.tip.layer.hwnd);
        self.tip.layer.visible = true;
        self.tip.shown = true;
        self.ensure_anim();
    }

    fn hide_tooltip(&mut self, immediate: bool) {
        self.tip.pending = None;
        self.tip.target = None;
        win::kill_timer(T_TOOLTIP);
        if !self.tip.shown {
            if layer::tooltip_window_stale(false, win::is_visible(self.tip.layer.hwnd)) {
                win::hide(self.tip.layer.hwnd);
                self.tip.layer.visible = false;
            }
            return;
        }
        if immediate {
            win::hide(self.tip.layer.hwnd);
            self.tip.layer.visible = false;
            self.tip.layer.fade = None;
            self.tip.shown = false;
        } else {
            let from = self.tip.layer.alpha;
            let s = self.tip.layer.scale;
            self.tip.layer.fade = Some(Fade { from, to: 0.0, start: Instant::now(), ms: 200.0, ease_out: false, slide: Some((self.tip.layer.slide, 10.0 * s)) });
            self.ensure_anim();
        }
    }

    // ------------------------------------------------------------------ mouse

    /// (bar index, or usize::MAX for the popup)
    fn layer_of(&mut self, hwnd: HWND) -> Option<usize> {
        if let Some(i) = self.bars.iter().position(|b| b.layer.hwnd == hwnd) {
            return Some(i);
        }
        if self.popup.as_ref().is_some_and(|p| p.layer.hwnd == hwnd) {
            return Some(usize::MAX);
        }
        None
    }

    fn layer_mut(&mut self, which: usize) -> Option<&mut Layer> {
        if which == usize::MAX {
            self.popup.as_mut().map(|p| &mut p.layer)
        } else {
            self.bars.get_mut(which).map(|b| &mut b.layer)
        }
    }

    fn on_mouse_move(&mut self, hwnd: HWND, x: i32, y: i32) {
        let Some(which) = self.layer_of(hwnd) else { return };
        let mut slider_ev = None;
        let tip_target;
        let hover_target;
        {
            let layer = self.layer_mut(which).unwrap();
            let p = (x as f32 / layer.scale, y as f32 / layer.scale);
            layer.mouse = Some(p);
            let chain = layer.chain_at(p.0, p.1);
            if chain != layer.hover {
                layer.dirty = true;
            }
            if let Some(k) = layer.drag_slider
                && let Some(id) = layer.node_by_key(k) {
                    slider_ev = Some((layer.tree.nodes[id].owner, slider_value(&layer.tree.nodes[id], p.0), layer.tree.nodes[id].on_slider.unwrap_or(0)));
                }
            hover_target = if which == usize::MAX || layer.drag_slider.is_some() || layer.pressed.is_some() {
                None
            } else {
                chain.iter().find_map(|&c| {
                    let n = &layer.tree.nodes[c];
                    n.on_hover.map(|id| HoverNode { key: n.key, owner: n.owner, id, anchor: layer.screen_rect(c) })
                })
            };
            tip_target = if layer.drag_slider.is_some() || layer.pressed.is_some() {
                None
            } else {
                chain.iter().find_map(|&c| layer.tree.nodes[c].tooltip.clone().filter(|t| !t.is_empty()).map(|t| (layer.tree.nodes[c].key, t)))
            };
        }
        if let Some((owner, value, id)) = slider_ev {
            self.dispatch(owner, Ev::Slider { id, value, released: false });
        }
        self.update_tooltip_target(hwnd.0 as isize, tip_target);
        self.update_hover(hwnd.0 as isize, which, hover_target);
    }

    // ------------------------------------------------------------------ thumbnail on hover

    /// Stop tracking hover: stop the timer and close the thumbnail if it is showing.
    fn clear_hover(&mut self) {
        win::kill_timer(T_HOVER);
        self.hover = None;
        self.thumb = None;
    }

    fn update_hover(&mut self, hwnd: isize, bar: usize, target: Option<HoverNode>) {
        match target {
            Some(HoverNode { key, owner, id, anchor }) => {
                if self.hover.as_ref().is_some_and(|h| h.hwnd == hwnd && h.key == key) {
                    return;
                }
                self.clear_hover();
                self.hover = Some(Hover { hwnd, key, owner, id, bar, anchor, fired: false });
                win::set_timer(T_HOVER, HOVER_MS);
            }
            None => {
                if self.hover.is_some() {
                    self.clear_hover();
                }
            }
        }
    }

    fn on_hover_timer(&mut self) {
        win::kill_timer(T_HOVER);
        let Some(h) = self.hover.as_mut().filter(|h| !h.fired) else { return };
        h.fired = true;
        let (owner, id) = (h.owner, h.id);
        self.dispatch(owner, Ev::Hover { id });
    }

    fn open_thumb(&mut self, owner: usize, source: isize) {
        let Some(h) = self.hover.as_ref().filter(|h| h.owner == owner && h.fired) else { return };
        let Some(b) = self.bars.get(h.bar) else { return };
        let above = b.cfg.alignment.position == "bottom";
        let (anchor, scale, hwnd_key) = (h.anchor, b.mon.scale, (h.hwnd, h.key));
        self.thumb = None;
        self.thumb = Thumb::open(HWND(source as *mut _), anchor, above, scale);
        if self.thumb.is_some() {
            // the tooltip (400 ms) and thumbnail (500 ms) are both on the node → hide the tooltip and block it from reappearing
            // until the mouse leaves the node (same `blocked` mechanism as after a click)
            self.hide_tooltip(true);
            self.tip.blocked = Some(hwnd_key);
        }
    }

    fn on_mouse_leave(&mut self, hwnd: HWND) {
        let Some(which) = self.layer_of(hwnd) else { return };
        let (cx, cy) = win::cursor_pos();
        let layer = self.layer_mut(which).unwrap();
        // a window resize/move also generates WM_MOUSELEAVE even though the mouse is still inside → ignore
        if layer.drag_slider.is_some() || layer.contains_screen(cx, cy) {
            return;
        }
        layer.mouse = None;
        layer.dirty = true;
        self.update_tooltip_target(hwnd.0 as isize, None);
        self.update_hover(hwnd.0 as isize, which, None);
    }

    fn on_button(&mut self, hwnd: HWND, x: i32, y: i32, btn: MouseBtn, down: bool) {
        let Some(which) = self.layer_of(hwnd) else { return };
        self.hide_tooltip(true);
        self.clear_hover();
        if down {
            let layer = self.layer_mut(which).unwrap();
            let p = (x as f32 / layer.scale, y as f32 / layer.scale);
            let chain = layer.chain_at(p.0, p.1);
            let key = chain.iter().find(|&&c| layer.tree.nodes[c].tooltip.as_ref().is_some_and(|t| !t.is_empty())).map(|&c| layer.tree.nodes[c].key);
            self.tip.blocked = key.map(|k| (hwnd.0 as isize, k));
        }
        let mut events: Vec<(usize, Ev)> = Vec::new();
        let mut bar_menu = false;
        {
            let swallow = self.swallow_click;
            let layer = self.layer_mut(which).unwrap();
            let p = (x as f32 / layer.scale, y as f32 / layer.scale);
            layer.mouse = Some(p);
            let chain = layer.chain_at(p.0, p.1);
            layer.dirty = true;
            if down {
                layer.pressed = chain.first().map(|&n| (layer.tree.nodes[n].key, btn));
                for &c in &chain {
                    let n = &layer.tree.nodes[c];
                    if matches!(btn, MouseBtn::Left) && matches!(n.content, Content::Slider { .. }) && let Some(slider_id) = n.on_slider {
                        layer.drag_slider = Some(n.key);
                        events.push((n.owner, Ev::Slider { id: slider_id, value: slider_value(n, p.0), released: false }));
                        break;
                    }
                    if let Some(id) = n.on_press {
                        if matches!(btn, MouseBtn::Left) {
                            events.push((n.owner, Ev::Press { id }));
                        }
                        break;
                    }
                    if n.on_click.is_some() {
                        break;
                    }
                }
            } else {
                if let Some(k) = layer.drag_slider.take() {
                    if let Some(id) = layer.node_by_key(k) {
                        let n = &layer.tree.nodes[id];
                        events.push((n.owner, Ev::Slider { id: n.on_slider.unwrap_or(0), value: slider_value(n, p.0), released: true }));
                    }
                } else if layer.pressed.is_some() {
                    // dispatch the click to the nearest node with a handler (like mouseReleaseEvent propagating to the parent)
                    let mut handled = false;
                    for &c in &chain {
                        let n = &layer.tree.nodes[c];
                        if n.on_press.is_some() {
                            handled = true;
                            break;
                        }
                        if let Some(id) = n.on_click {
                            if n.states & crate::engine::css::state::DISABLED == 0 && !(id == widgets::BASE && swallow == Some(n.owner)) {
                                events.push((n.owner, Ev::Click { id, btn }));
                            }
                            handled = true;
                            break;
                        }
                    }
                    if !handled && matches!(btn, MouseBtn::Right) && which != usize::MAX {
                        bar_menu = true;
                    }
                }
                layer.pressed = None;
            }
        }
        if !down {
            self.swallow_click = None;
        }
        for (owner, ev) in events {
            self.dispatch(owner, ev);
        }
        if bar_menu && self.bars.get(which).is_some_and(|b| b.cfg.context_menu) {
            let lang = self.lang;
            let entries = vec![
                MenuEntry::Item { id: BM_TASKMGR, text: menu_text(lang, BM_TASKMGR).into(), enabled: true },
                MenuEntry::Item { id: BM_OPEN_APP, text: menu_text(lang, BM_OPEN_APP).into(), enabled: true },
                MenuEntry::Sep,
                MenuEntry::Item { id: BM_TURN_OFF, text: menu_text(lang, BM_TURN_OFF).into(), enabled: true },
            ];
            self.open_menu(BAR_OWNER, which, entries);
        }
    }

    fn on_wheel(&mut self, hwnd: HWND, sx: i32, sy: i32, delta: i32) {
        // WM_MOUSEWHEEL goes to the focused window; find the window under the mouse
        let mut target = None;
        if let Some(p) = &self.popup
            && p.layer.contains_screen(sx, sy) {
                target = Some(usize::MAX);
            }
        if target.is_none() {
            target = self.bars.iter().position(|b| b.layer.contains_screen(sx, sy));
        }
        let _ = hwnd;
        let Some(which) = target else { return };
        let mut ev = None;
        {
            let layer = self.layer_mut(which).unwrap();
            let p = ((sx - layer.pos.0) as f32 / layer.scale, (sy - layer.pos.1) as f32 / layer.scale);
            for c in layer.chain_at(p.0, p.1) {
                let n = &layer.tree.nodes[c];
                if let Content::Slider { value, max } = n.content {
                    let step = (max / 50).max(1) * if delta > 0 { 1 } else { -1 };
                    ev = Some((n.owner, Ev::Slider { id: n.on_slider.unwrap_or(0), value: (value + step).clamp(0, max), released: true }));
                    break;
                }
                if let Some(id) = n.on_wheel {
                    ev = Some((n.owner, Ev::Wheel { id, delta }));
                    break;
                }
            }
        }
        if let Some((o, e)) = ev {
            self.dispatch(o, e);
        }
    }

    fn on_global_press(&mut self, x: i32, y: i32) {
        let Some(p) = &self.popup else { return };
        crate::engine::log::debug(&format!("popup: bấm toàn cục {x},{y}"));
        if p.closing || p.layer.contains_screen(x, y) {
            return;
        }
        let owner = p.owner;
        // clicking the owner widget again → only close (toggle), don't reopen
        let (ax, ay, aw, ah) = p.anchor;
        if x >= ax && x < ax + aw && y >= ay && y < ay + ah {
            self.swallow_click = Some(owner);
        }
        self.close_popup(true);
    }

    // ------------------------------------------------------------------ event loop

    pub fn handle(&mut self, e: WinEvent) {
        match e {
            WinEvent::MouseMove { hwnd, x, y } => self.on_mouse_move(hwnd, x, y),
            WinEvent::MouseLeave { hwnd } => self.on_mouse_leave(hwnd),
            WinEvent::Button { hwnd, x, y, btn, down } => self.on_button(hwnd, x, y, btn, down),
            WinEvent::Wheel { hwnd, x, y, delta } => self.on_wheel(hwnd, x, y, delta),
            WinEvent::GlobalPress { x, y } => self.on_global_press(x, y),
            WinEvent::Activate { hwnd, active } => {
                if !active && self.popup.as_ref().is_some_and(|p| p.layer.hwnd == hwnd) {
                    crate::engine::log::debug("popup: mất focus");
                    // lost focus (clicked outside) → close like Qt::Popup
                    let (cx, cy) = win::cursor_pos();
                    if let Some(p) = &self.popup {
                        let (ax, ay, aw, ah) = p.anchor;
                        if cx >= ax && cx < ax + aw && cy >= ay && cy < ay + ah {
                            self.swallow_click = Some(p.owner);
                        }
                    }
                    self.close_popup(true);
                }
            }
            WinEvent::Key { hwnd, vk } => {
                if vk == 0x1B && self.popup.as_ref().is_some_and(|p| p.layer.hwnd == hwnd) {
                    self.close_popup(true);
                }
            }
            WinEvent::Timer { id } => match id {
                T_ANIM => self.tick_anim(),
                T_TOOLTIP => self.show_tooltip(),
                T_HOVER => self.on_hover_timer(),
                T_AUTOHIDE => self.tick_auto_hide(),
                T_REPLACE => {
                    win::kill_timer(T_REPLACE);
                    self.replace_all_appbars();
                }
                T_FULLSCREEN => self.refresh_fullscreen(),
                T_APPBAR => {
                    win::set_timer(T_APPBAR, APPBAR_EVERY_MS);
                    self.check_appbars("timer");
                }
                T_APPBAR_DEBOUNCE => {
                    win::kill_timer(T_APPBAR_DEBOUNCE);
                    self.check_appbars("debounce");
                }
                T_DISPLAY => {
                    win::kill_timer(T_DISPLAY);
                    crate::engine::log::info("Màn hình thay đổi → dựng lại thanh bar");
                    self.teardown();
                    self.build_bars();
                }
                id if id >= T_WIDGET => self.dispatch(id - T_WIDGET, Ev::Timer),
                _ => {}
            },
            WinEvent::DisplayChange => win::set_timer(T_DISPLAY, 1000),
            // Schedule a check once things settle (not immediately: rcWork may be mid-change caused by us)
            WinEvent::WorkAreaChange => win::set_timer(T_APPBAR_DEBOUNCE, APPBAR_DEBOUNCE_MS),
            WinEvent::SettingChange | WinEvent::AccentChange => {
                self.refresh_theme();
                for b in &mut self.bars {
                    b.layer.dirty = true;
                }
            }
            WinEvent::AppBar { hwnd, code } => {
                // ABN_POSCHANGED: another taskbar/appbar moved → reposition
                if code == 1
                    && let Some(bi) = self.bars.iter().position(|b| b.layer.hwnd == hwnd && b.appbar)
                {
                    self.place_appbar(bi);
                    win::set_timer(T_APPBAR_DEBOUNCE, APPBAR_DEBOUNCE_MS);
                }
            }
            WinEvent::Notify { code, param } => match code {
                N_AUDIO => self.broadcast(SysEv::AudioChanged),
                N_AUDIO_DEVICES => self.broadcast(SysEv::AudioDevicesChanged),
                N_NOTIFICATIONS => self.broadcast(SysEv::Notifications(param as u32)),
                N_SYSTRAY => self.broadcast(SysEv::Systray),
                _ => {}
            },
            WinEvent::Hook { event, hwnd, id_object } => {
                use windows::Win32::UI::WindowsAndMessaging::*;
                let h = hwnd.0 as isize;
                let ev = match event {
                    EVENT_SYSTEM_FOREGROUND | EVENT_SYSTEM_MINIMIZEEND => Some(SysEv::Foreground(h)),
                    EVENT_SYSTEM_MOVESIZEEND => Some(SysEv::MoveSizeEnd(h)),
                    EVENT_OBJECT_NAMECHANGE if id_object == OBJID_WINDOW.0 => Some(SysEv::NameChange(h)),
                    EVENT_OBJECT_DESTROY if id_object == OBJID_WINDOW.0 => Some(SysEv::Destroy(h)),
                    _ => None,
                };
                if let Some(ev) = ev {
                    if let SysEv::Destroy(h) = ev
                        && self.thumb.as_ref().is_some_and(|t| t.source() == h)
                    {
                        self.thumb = None;
                    }
                    let fg_changed = matches!(ev, SysEv::Foreground(_) | SysEv::MoveSizeEnd(_));
                    self.broadcast(ev);
                    if fg_changed {
                        self.refresh_fullscreen();
                    }
                }
            }
            WinEvent::EndSession => request_exit(),
        }
    }

    pub fn shutdown(&mut self) {
        self.clear_hover();
        self.teardown();
        for b in self.reuse.drain(..) {
            if b.appbar {
                win::appbar_remove(b.layer.hwnd);
            }
            win::destroy(b.layer.hwnd);
        }
        sys::systray::stop();
        sys::notifications::stop();
        win::destroy(self.tip.layer.hwnd);
    }
}

/// The bar is topmost when `always_on_top`, and also when `auto_hide` (when revealed it must overlay other windows).
fn bar_topmost(bc: &BarConfig) -> bool {
    bc.window_flags.always_on_top || bc.window_flags.auto_hide
}

/// The bar's top/bottom margins converted to pixels using the monitor's DPI.
fn pad_px(bc: &BarConfig, scale: f32) -> (i32, i32) {
    ((bc.padding.top as f32 * scale) as i32, (bc.padding.bottom as f32 * scale) as i32)
}

fn area_of(r: windows::Win32::Foundation::RECT) -> heal::Area {
    heal::Area::new(r.left, r.top, r.right, r.bottom)
}

/// Build the QMenu tree: `.context-menu` → the `::item` / `::separator` parts.
fn build_menu(ui: &mut Ui, class: &str, entries: &[MenuEntry]) {
    let mut root = Node::new("QMenu", class);
    root.dir = Dir::Col;
    ui.with(root, |ui| {
        for e in entries {
            match e {
                MenuEntry::Item { id, text, enabled } => {
                    let mut n = Node::new("QMenuItem", "");
                    n.sub_element = Some("item");
                    n.content = Content::Text(text.clone());
                    n.on_click = Some(*id);
                    if !enabled {
                        n.states |= crate::engine::css::state::DISABLED;
                    }
                    ui.add(n);
                }
                MenuEntry::Sep => {
                    let mut n = Node::new("QMenuSeparator", "");
                    n.sub_element = Some("separator");
                    n.content = Content::HLine;
                    n.vpolicy = Policy::Fixed;
                    ui.add(n);
                }
            }
        }
    });
}

fn slider_value(n: &Node, x: f32) -> i32 {
    let Content::Slider { max, .. } = n.content else { return 0 };
    let r = n.rect.inset(&n.style.margin);
    let x0 = r.x + 7.0;
    let x1 = r.right() - 7.0;
    let frac = if x1 > x0 { ((x - x0) / (x1 - x0)).clamp(0.0, 1.0) } else { 0.0 };
    (frac * max as f32).round() as i32
}

pub fn exit_requested() -> bool {
    EXIT.with(|e| e.get())
}

