//! Taskbar engine: ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! Runs the whole bar (window, AppBar, widgets, popups) on a dedicated thread with a `GetMessageW` loop;
//! the rest of the app only sees `TaskbarHandle` (spawn / reload / shutdown) and `TaskbarEvent`.

// code copied from TachyonTaskbar (MIT): keep `unsafe fn` bodies in the edition 2021 style and a few purely stylistic lints.
#![allow(unsafe_op_in_unsafe_fn)]

mod app;
mod appbar;
mod config;
mod css;
mod gfx;
mod heal;
mod layer;
mod log;
mod sys;
mod thumb;
mod ui;
mod widgets;
mod win;

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{JoinHandle, ThreadId};
use std::time::Duration;

use sg_core::i18n::Lang;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, MSG, PostMessageW, TranslateMessage};

use crate::Generated;

/// The original taskbar's hot corner opens Start using the same function as the Windows button on the bar.
pub(crate) use sys::start::open_start;

/// Events sent from the bar to the app (called on the engine thread: do not block for long; `shutdown` called from inside
/// the callback detects this itself and only sets a flag instead of waiting on itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskbarEvent {
    /// Bar right-click menu: "Open ShunguangDecor".
    OpenApp,
    /// Bar right-click menu: "Turn off this bar".
    TurnOff,
    /// A config reload was rejected (syntax error): the old bar keeps running normally.
    ConfigRejected(String),
    /// The bar hit a severe error (panic in the engine thread: by then the thread has cleaned up and exited).
    Error(String),
    /// The engine thread exited on its own, not because of `shutdown()` (Windows sign-out, power menu...): the bar is no longer running.
    Stopped,
}

enum Cmd {
    Reload(Generated),
    /// Like `Reload` but re-registers the AppBar (Explorer restarted).
    ReloadReregister(Generated),
    /// Repositions the AppBar of every bar (after the original taskbar changes the space it reserves).
    ReplaceAppbars,
    Shutdown,
}

struct Shared {
    cmds: Mutex<VecDeque<Cmd>>,
    /// message window of the engine thread (wakes the `GetMessageW` loop); 0 when absent/destroyed
    hwnd: AtomicIsize,
}

impl Shared {
    fn send(&self, c: Cmd) {
        if let Ok(mut q) = self.cmds.lock() {
            q.push_back(c);
        }
        let h = self.hwnd.load(Ordering::SeqCst);
        if h != 0 {
            unsafe {
                let _ = PostMessageW(Some(HWND(h as *mut _)), win::WM_NOTIFY_APP, WPARAM(app::N_CMD), LPARAM(0));
            }
        }
    }

    fn take(&self) -> Option<Cmd> {
        self.cmds.lock().ok().and_then(|mut q| q.pop_front())
    }
}

/// A running bar. `Drop` calls `shutdown` itself.
pub struct TaskbarHandle {
    shared: Arc<Shared>,
    thread: Mutex<Option<(JoinHandle<()>, Receiver<()>)>>,
    engine_thread: ThreadId,
}

impl TaskbarHandle {
    /// Creates the engine thread (COM STA + message loop) and builds the bar. Returns `Err` if the config is invalid or
    /// graphics cannot be initialized; in that case the thread has exited cleanly.
    pub fn spawn(
        generated: Generated,
        lang: Lang,
        events: impl Fn(TaskbarEvent) + Send + 'static,
    ) -> Result<TaskbarHandle, String> {
        let shared = Arc::new(Shared { cmds: Mutex::new(VecDeque::new()), hwnd: AtomicIsize::new(0) });
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let events: Box<dyn Fn(TaskbarEvent) + Send> = Box::new(events);
        let s2 = shared.clone();
        let join = std::thread::Builder::new()
            .name("sg-taskbar".into())
            .spawn(move || {
                engine_thread(generated, lang, events, s2, ready_tx);
                let _ = done_tx.send(());
            })
            .map_err(|e| format!("không tạo được luồng thanh: {e}"))?;
        let engine_thread = join.thread().id();
        match ready_rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(())) => Ok(TaskbarHandle { shared, thread: Mutex::new(Some((join, done_rx))), engine_thread }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(RecvTimeoutError::Timeout) => {
                // the thread did not report in time: tell it to exit and do not wait any longer
                shared.send(Cmd::Shutdown);
                Err("luồng thanh không khởi động được trong 15 giây".into())
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = join.join();
                Err("luồng thanh thoát đột ngột khi khởi động".into())
            }
        }
    }

    /// Rebuilds the bar from the new config, keeping the window/AppBar with the same name and display (no flicker).
    /// An invalid config keeps the old bar and sends `TaskbarEvent::ConfigRejected`.
    pub fn reload(&self, generated: Generated) {
        self.shared.send(Cmd::Reload(generated));
    }

    /// Explorer just restarted: rebuild the bar and re-register each bar's AppBar (ABM_REMOVE + ABM_NEW +
    /// ABM_SETPOS) along with blur/position, because the new Explorer no longer knows the work area the bar reserved.
    pub fn reload_after_explorer_restart(&self, generated: Generated) {
        self.shared.send(Cmd::ReloadReregister(generated));
    }

    /// Repositions the AppBar of every bar (`ABM_QUERYPOS` + `ABM_SETPOS`, then moves the window) without rebuilding the bar. Call AFTER
    /// the original taskbar changes mode (auto-hide turned off, shown again...): when the bar was built the original taskbar was still auto-hiding, so the system had not yet counted
    /// the space it reserves, and the bar would sit on top of the original taskbar.
    pub fn replace_appbars(&self) {
        self.shared.send(Cmd::ReplaceAppbars);
    }

    /// Removes the AppBar (`ABM_REMOVE`), destroys the window, exits the thread; waits at most 3 seconds. Safe to call repeatedly.
    pub fn shutdown(&self) {
        let taken = self.thread.lock().ok().and_then(|mut t| t.take());
        let Some((join, done)) = taken else { return };
        self.shared.send(Cmd::Shutdown);
        if std::thread::current().id() == self.engine_thread {
            // called from an event callback (right on the engine thread): do not wait on yourself
            return;
        }
        match done.recv_timeout(Duration::from_secs(3)) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                let _ = join.join();
            }
            Err(RecvTimeoutError::Timeout) => {
                tracing::warn!(target: "sg::taskbar", "luồng thanh chưa thoát sau 3 giây, bỏ qua không chờ nữa");
            }
        }
    }
}

impl Drop for TaskbarHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Checks whether two files are valid for the engine (without building the bar): used when importing a YASB theme.
pub fn validate(g: &Generated) -> Result<(), String> {
    parse(g).map(|_| ())
}

/// The clock's chrono locale code is already in the YAML produced by `generate`; here the config is only parsed.
fn parse(g: &Generated) -> Result<(config::RootConfig, css::Stylesheet), String> {
    let cfg = config::parse_yaml(&g.yaml)?;
    let sheet = css::Stylesheet::parse(&css::preprocess(&g.css, Path::new("")));
    Ok((cfg, sheet))
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "lỗi không rõ".into())
}

fn engine_thread(
    generated: Generated,
    lang: Lang,
    events: Box<dyn Fn(TaskbarEvent) + Send>,
    shared: Arc<Shared>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let com_ok = unsafe {
        // Per-thread DPI: the Tauri app already set the DPI for the whole process, the engine does not touch it
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok()
    };
    let events: Rc<dyn Fn(TaskbarEvent)> = Rc::new(events);
    let ev2 = events.clone();
    let result = catch_unwind(AssertUnwindSafe(|| run(generated, lang, events, &shared, &ready)));
    if let Err(p) = result {
        let msg = panic_text(&*p);
        tracing::error!(target: "sg::taskbar", "luồng thanh bị panic: {msg}");
        // just in case (a panic outside `run`): clean up any leftover global state, then notify the app
        teardown_globals();
        let _ = catch_unwind(AssertUnwindSafe(|| ev2(TaskbarEvent::Error(msg.clone()))));
        let _ = ready.send(Err(format!("thanh bị lỗi khi khởi động: {msg}")));
    }
    shared.hwnd.store(0, Ordering::SeqCst);
    if com_ok {
        unsafe {
            CoUninitialize();
        }
    }
}

/// Cleans up all global engine state (tray, notification counter, hook, message window); each step is wrapped in
/// `catch_unwind` so one failing step does not cause later steps to be skipped.
fn teardown_globals() {
    let _ = catch_unwind(sys::systray::stop);
    let _ = catch_unwind(sys::notifications::stop);
    let _ = catch_unwind(win::cleanup);
}

fn run(
    generated: Generated,
    lang: Lang,
    events: Rc<dyn Fn(TaskbarEvent)>,
    shared: &Shared,
    ready: &mpsc::Sender<Result<(), String>>,
) {
    let (cfg, sheet) = match parse(&generated) {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(format!("cấu hình thanh sai: {e}")));
            return;
        }
    };
    win::register_classes();
    let hwnd = match win::create_message_window() {
        Ok(h) => h,
        Err(e) => {
            teardown_globals();
            let _ = ready.send(Err(e));
            return;
        }
    };
    shared.hwnd.store(hwnd.0 as isize, Ordering::SeqCst);
    win::install_win_event_hooks();
    let mut app = match catch_unwind(AssertUnwindSafe(|| app::App::new(lang, events.clone()))) {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => {
            teardown_globals();
            let _ = ready.send(Err(e));
            return;
        }
        Err(p) => {
            teardown_globals();
            let _ = ready.send(Err(format!("thanh bị lỗi khi khởi động: {}", panic_text(&*p))));
            return;
        }
    };

    // `app` lives outside catch_unwind, so even if `apply`/the loop panics midway, `app.shutdown()` still
    // removes the AppBar and destroys every bar window that was built.
    let looped = catch_unwind(AssertUnwindSafe(|| {
        app.apply(cfg, sheet);
        app.flush();
        let _ = ready.send(Ok(()));
        message_loop(&mut app, shared, &events)
    }));
    let (panic_msg, requested) = match looped {
        Ok(requested) => (None, requested),
        Err(p) => (Some(panic_text(&*p)), false),
    };
    if let Some(m) = &panic_msg {
        tracing::error!(target: "sg::taskbar", "thanh bị panic: {m}");
    }
    if catch_unwind(AssertUnwindSafe(|| app.shutdown())).is_err() {
        tracing::error!(target: "sg::taskbar", "dọn thanh bị panic");
    }
    drop(app);
    teardown_globals();
    shared.hwnd.store(0, Ordering::SeqCst);
    match panic_msg {
        Some(m) => {
            let _ = ready.send(Err(format!("thanh bị lỗi khi khởi động: {m}")));
            let _ = catch_unwind(AssertUnwindSafe(|| events(TaskbarEvent::Error(m))));
        }
        None if !requested => {
            let _ = catch_unwind(AssertUnwindSafe(|| events(TaskbarEvent::Stopped)));
        }
        None => {}
    }
}

/// Returns `true` if it exited because of the `shutdown()` command, `false` if the engine exited on its own (sign-out, power menu...).
fn message_loop(app: &mut app::App, shared: &Shared, events: &Rc<dyn Fn(TaskbarEvent)>) -> bool {
    let mut msg = MSG::default();
    loop {
        while let Some(c) = shared.take() {
            let reregister = matches!(c, Cmd::ReloadReregister(_));
            match c {
                Cmd::Shutdown => return true,
                Cmd::ReplaceAppbars => {
                    app.replace_appbars();
                    app.flush();
                }
                Cmd::Reload(g) | Cmd::ReloadReregister(g) => match parse(&g) {
                    Ok((cfg, sheet)) => {
                        if reregister {
                            app.apply_reregistering_appbar(cfg, sheet);
                        } else {
                            app.apply(cfg, sheet);
                        }
                        app.flush();
                    }
                    Err(e) => {
                        tracing::warn!(target: "sg::taskbar", "bỏ qua cấu hình mới: {e}");
                        events(TaskbarEvent::ConfigRejected(format!("cấu hình thanh sai: {e}")));
                    }
                },
            }
        }
        if app::exit_requested() {
            return false;
        }
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            return false;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        while let Some(e) = win::pop() {
            app.handle(e);
            if app::exit_requested() {
                return false;
            }
        }
        app.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TaskbarConfig, generate};

    /// R1: the generator and the engine understand each other — the default YAML/CSS parse and have all 3 zones in order.
    #[test]
    fn generated_default_parses_in_engine() {
        let g = generate(&TaskbarConfig::default(), "vi");
        let (cfg, sheet) = parse(&g).expect("YAML/CSS sinh ra phải đọc được");
        assert!(sheet.rule_count() > 0);
        assert_eq!(cfg.bar_order.len(), 1);
        let bar = &cfg.bars[&cfg.bar_order[0]];
        let want = TaskbarConfig::default();
        assert_eq!(bar.widgets.left.len(), want.widgets.left.len());
        assert_eq!(bar.widgets.center.len(), want.widgets.center.len());
        assert_eq!(bar.widgets.right.len(), want.widgets.right.len());
        // widget names set by the generator: each zone matches its item count and every name has a widget type definition
        for name in bar.widgets.left.iter().chain(&bar.widgets.center).chain(&bar.widgets.right) {
            assert!(cfg.widgets.contains_key(name), "widget '{name}' phải có định nghĩa");
        }
        let type_of = |n: &String| cfg.widgets[n].type_name.clone();
        let left: Vec<String> = bar.widgets.left.iter().map(type_of).collect();
        let center: Vec<String> = bar.widgets.center.iter().map(type_of).collect();
        let right: Vec<String> = bar.widgets.right.iter().map(type_of).collect();
        let short = |v: Vec<String>| v.into_iter().map(|t| t.rsplit('.').next().unwrap_or("").to_string()).collect::<Vec<_>>();
        assert_eq!(short(left), ["WindowsButtonWidget", "SearchWidget", "TaskbarWidget"]);
        assert_eq!(short(center), ["ClockWidget"]);
        assert_eq!(
            short(right),
            ["SystrayWidget", "WifiWidget", "VolumeWidget", "BatteryWidget", "NotificationsWidget"]
        );
    }

    /// R2: an unknown widget name is skipped (None) and does not break the whole bar.
    #[test]
    fn unknown_widget_type_is_skipped() {
        let v = serde_yaml::Value::Null;
        assert!(widgets::create("x", "NoSuchWidget", &v).is_none());
    }

    #[test]
    fn menu_labels_follow_lang() {
        assert_eq!(app::menu_text(Lang::Vi, 3), "Tắt thanh này");
        assert_eq!(app::menu_text(Lang::En, 3), "Turn this bar off");
        assert_eq!(app::menu_text(Lang::Vi, 1), "Trình quản lý tác vụ");
    }

    fn work_area() -> windows::Win32::Foundation::RECT {
        use windows::Win32::UI::WindowsAndMessaging::{
            SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
        };
        let mut r = windows::Win32::Foundation::RECT::default();
        unsafe {
            let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut r as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        }
        r
    }

    fn taskbar_pos() -> windows::Win32::Foundation::RECT {
        use windows::Win32::UI::Shell::{ABM_GETTASKBARPOS, APPBARDATA, SHAppBarMessage};
        let mut d = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, ..Default::default() };
        unsafe {
            SHAppBarMessage(ABM_GETTASKBARPOS, &mut d);
        }
        d.rc
    }

    fn has_window(class: windows::core::PCWSTR) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
        unsafe { FindWindowW(class, windows::core::PCWSTR::null()).is_ok_and(|h| !h.0.is_null()) }
    }

    /// Shows the real bar on the user's screen and then turns it off, two consecutive rounds in the same process.
    /// Does not hide the original taskbar. Run manually: `cargo test -p sg-taskbar spawn_and_shutdown_real -- --ignored`.
    #[test]
    #[ignore]
    fn spawn_and_shutdown_real() {
        use windows::core::w;
        let before_tb = taskbar_pos();
        let before_wa = work_area();
        for round in 0..2 {
            let g = generate(&TaskbarConfig::default(), "vi");
            let h = TaskbarHandle::spawn(g.clone(), Lang::Vi, |e| eprintln!("sự kiện: {e:?}")).expect("spawn");
            assert!(has_window(w!("TachyonTaskbarWnd")), "vòng {round}: phải thấy cửa sổ thanh");
            std::thread::sleep(Duration::from_millis(1200));
            h.reload(g);
            std::thread::sleep(Duration::from_millis(800));
            let t = std::time::Instant::now();
            h.shutdown();
            h.shutdown(); // the second call must not error
            assert!(t.elapsed() < Duration::from_secs(3), "shutdown quá 3 giây: {:?}", t.elapsed());
            assert!(!has_window(w!("TachyonTaskbarWnd")), "vòng {round}: cửa sổ thanh phải biến mất");
            assert!(!has_window(w!("TachyonTaskbarMsg")), "vòng {round}: cửa sổ thông báo phải biến mất");
            std::thread::sleep(Duration::from_millis(300));
            let (tb, wa) = (taskbar_pos(), work_area());
            assert_eq!((tb.left, tb.top, tb.right, tb.bottom), (before_tb.left, before_tb.top, before_tb.right, before_tb.bottom));
            assert_eq!(
                (wa.left, wa.top, wa.right, wa.bottom),
                (before_wa.left, before_wa.top, before_wa.right, before_wa.bottom),
                "AppBar phải được gỡ"
            );
        }
    }
}
