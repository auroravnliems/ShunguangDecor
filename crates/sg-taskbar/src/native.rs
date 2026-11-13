//! Controls the native Windows taskbar: leave as is / hide completely / hide and reveal on edge touch (`reveal`), plus the hot corner.
//!
//! The logic sits on top of `NativePort` (a small port) so it can be tested with fakes; the real Win32 version is in `win32` (Windows only).
//! Only READS the cursor and the foreground window from a polling thread: no hooks, nothing injected into other processes
//! (games do not treat this as a global mouse hook).
//!
//! # Work area while the native taskbar is hidden (measured on Windows 11 26200)
//! - With only `SW_HIDE`, Windows still keeps the taskbar's work area (a 48 px dead strip).
//! - Setting the autohide bit (`ABM_SETSTATE` + `ABS_AUTOHIDE`) returns the work area immediately; clearing it takes it back immediately.
//! - Autohide on + `SW_HIDE`: touching the edge does NOT make it appear (fits the Hide mode). To show it (reveal mode), use `SW_SHOWNOACTIVATE`:
//!   Explorer slides it in within ~72 ms.
//!
//! Therefore in Hide/reveal mode: if the user has NOT enabled autohide, write `DisableTaskbarAutohide` to the journal FIRST and then enable autohide
//! (keeping the other bits), then write `ShowNativeTaskbar` BEFORE `SW_HIDE`. `restore()` shows it again, disables autohide ONLY IF we
//! enabled it, and only then removes the two entries from the journal. Keep mode never touches the autohide bit.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use sg_core::guard::RestoreAction;
use sg_core::module::ModuleCtx;

use crate::model::{Corner, Edge, NativeMode};
use crate::reveal::{
    HotCornerState, Rect, RevealInput, RevealState, cursor_in_corner, cursor_on_edge, horizontal_edge, hot_corner_step,
    is_flyout_under_cursor, is_shell_flyout, reveal_step,
};

/// Polling period when a fast reaction is needed (reveal, hot corner).
pub const POLL_MS: u64 = 30;
/// Watch period when it is only necessary to keep the native taskbar hidden (Hide mode, no hot corner).
pub const HOLD_POLL_MS: u64 = 250;
/// How long to wait at most for the polling thread to stop before abandoning it (a hung Explorer makes the system call get stuck).
pub const STOP_JOIN_MS: u64 = 500;

/// Window handle (`HWND` as a number).
pub type Handle = isize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskbarWnd {
    pub handle: Handle,
    /// The rectangle, even while hidden.
    pub rect: Rect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Foreground {
    pub class: String,
    /// Name or path of the owning process's exe.
    pub exe: String,
}

/// Everything the logic needs from the operating system. Only `show`/`hide`/`set_autohide` change the system.
pub trait NativePort: Send + Sync + 'static {
    /// All native taskbars (primary + secondary), NOT including this process's own fake `Shell_TrayWnd` window.
    fn taskbars(&self) -> Vec<TaskbarWnd>;
    /// Show, without taking focus, without blocking.
    fn show(&self, h: Handle);
    /// Hide, without blocking.
    fn hide(&self, h: Handle);
    fn is_visible(&self, h: Handle) -> bool;
    /// The native taskbar's autohide bit (`ABM_GETSTATE`); `None` if it cannot be read.
    fn autohide(&self) -> Option<bool>;
    /// Sets the autohide bit, keeping the other bits (`ABM_SETSTATE`).
    fn set_autohide(&self, on: bool) -> io::Result<()>;
    fn cursor_pos(&self) -> Option<(i32, i32)>;
    fn monitor_rects(&self) -> Vec<Rect>;
    fn foreground(&self) -> Option<Foreground>;
    /// The root window (`GetAncestor(GA_ROOT)`) under `pt`: Win11's thumbnails/jump lists do not take the foreground, so this must be queried
    /// as well to know whether the mouse is hovering over a native taskbar flyout.
    fn window_under(&self, pt: (i32, i32)) -> Option<Foreground>;
    /// Called once at the start of the polling thread (real: set per-monitor DPI so mouse/monitor coordinates match).
    fn init_poll_thread(&self) {}
}

struct Worker {
    stop: mpsc::Sender<()>,
    /// The thread reports it has exited (sends, or is closed, when the thread ends).
    done: mpsc::Receiver<()>,
    /// Set before stopping: an abandoned (stuck) thread must not change the taskbar anymore when it wakes up.
    cancel: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

pub struct NativeTaskbar<P: NativePort> {
    port: Arc<P>,
    /// What to do when the hot corner fires (real: open Start).
    fire: Arc<dyn Fn() + Send + Sync>,
    ctx: Option<ModuleCtx>,
    current: Option<(NativeMode, Option<Corner>)>,
    worker: Option<Worker>,
    /// We hid the native taskbar ourselves (the `ShowNativeTaskbar` entry is in the journal).
    we_hid: bool,
    /// We enabled autohide ourselves (the `DisableTaskbarAutohide` entry is in the journal).
    we_set_autohide: bool,
}

/// Area of the intersection of two rectangles (0 if disjoint).
fn overlap_area(a: Rect, b: Rect) -> i64 {
    let w = (a.right.min(b.right) - a.left.max(b.left)).max(0) as i64;
    let h = (a.bottom.min(b.bottom) - a.top.max(b.top)).max(0) as i64;
    w * h
}

/// Squared distance between two rectangles (0 if touching/overlapping).
fn gap_sq(a: Rect, b: Rect) -> i64 {
    let dx = (a.left.max(b.left) - a.right.min(b.right)).max(0) as i64;
    let dy = (a.top.max(b.top) - a.bottom.min(b.bottom)).max(0) as i64;
    dx * dx + dy * dy
}

/// The monitor of `r`: the one with the largest overlap; if it overlaps no monitor, the nearest monitor (like
/// `MONITOR_DEFAULTTONEAREST`). A taskbar that is auto-hiding has a rect slid almost off the monitor (measured: top 1078 on a 1080 monitor,
/// center outside), so it must not be chosen by its center — it used to fall back to the first monitor. On a tie, the earlier monitor wins.
fn monitor_of(r: Rect, monitors: &[Rect]) -> Option<Rect> {
    let mut best: Option<(i64, i64, Rect)> = None;
    for &m in monitors {
        let key = (overlap_area(r, m), -gap_sq(r, m));
        if best.is_none_or(|(o, g, _)| key > (o, g)) {
            best = Some((key.0, key.1, m));
        }
    }
    best.map(|(_, _, m)| m)
}

impl<P: NativePort> NativeTaskbar<P> {
    pub fn new(port: Arc<P>, fire: Arc<dyn Fn() + Send + Sync>) -> Self {
        NativeTaskbar { port, fire, ctx: None, current: None, worker: None, we_hid: false, we_set_autohide: false }
    }

    /// The edge (top/bottom) each horizontal native taskbar is docked to; vertical taskbars do not count. Used to know whether the bar's "autohide"
    /// competes for the edge with a native taskbar.
    pub fn native_edges(&self) -> Vec<Edge> {
        let monitors = self.port.monitor_rects();
        self.port
            .taskbars()
            .iter()
            .filter_map(|t| monitor_of(t.rect, &monitors).and_then(|m| horizontal_edge(t.rect, m)))
            .collect()
    }

    /// Applies the mode (and hot corner). Required order: write the journal BEFORE each change; if writing fails, change nothing further.
    pub fn apply(&mut self, mode: NativeMode, hot_corner: Option<Corner>, ctx: &ModuleCtx) -> io::Result<()> {
        self.stop_worker();
        self.ctx = Some(ctx.clone());
        match mode {
            NativeMode::Keep => {
                // Only restore what we ourselves changed; if nothing was hidden, do not touch the taskbar.
                if self.we_hid || self.we_set_autohide {
                    self.undo(ctx)?;
                }
            }
            NativeMode::Hide | NativeMode::Reveal => {
                self.take_work_area(ctx)?;
                ctx.journal_add(RestoreAction::ShowNativeTaskbar)?;
                self.we_hid = true;
                self.hide_all();
            }
        }
        self.current = Some((mode, hot_corner));
        let hold = matches!(mode, NativeMode::Hide | NativeMode::Reveal);
        if (hold || hot_corner.is_some())
            && let Err(e) = self.start_worker(mode, hot_corner)
        {
            // Without a watcher thread, do not leave the taskbar hidden with nobody watching it.
            let _ = self.restore();
            return Err(e);
        }
        Ok(())
    }

    /// Enables autohide if the user has not (so the work area is returned); if already enabled, leave it alone.
    fn take_work_area(&mut self, ctx: &ModuleCtx) -> io::Result<()> {
        // A leftover `DisableTaskbarAutohide` in the journal = last time we enabled autohide but the restore failed. The autohide that is on
        // right now is ours, not the user's: reclaim ownership so the next disable still turns it off.
        if !self.we_set_autohide && ctx.journal_contains(RestoreAction::DisableTaskbarAutohide) {
            self.we_set_autohide = true;
        }
        if self.we_set_autohide {
            // A restarted Explorer keeps the old setting, but check again to be sure.
            if self.port.autohide() == Some(false) {
                self.port.set_autohide(true)?;
            }
            return Ok(());
        }
        let state = self.port.autohide();
        if state.is_none() {
            tracing::warn!(target: "sg::taskbar", "cannot read the native taskbar auto-hide state; leaving auto-hide alone");
        }
        if state == Some(false) {
            ctx.journal_add(RestoreAction::DisableTaskbarAutohide)?;
            if let Err(e) = self.port.set_autohide(true) {
                let _ = ctx.journal_remove(RestoreAction::DisableTaskbarAutohide);
                return Err(e);
            }
            self.we_set_autohide = true;
        }
        Ok(())
    }

    /// Explorer restarted (`TaskbarCreated`) or the taskbar reappeared on its own: re-apply the current mode
    /// (including re-checking the autohide bit if we own it).
    pub fn reapply(&mut self) -> io::Result<()> {
        let (Some((mode, corner)), Some(ctx)) = (self.current, self.ctx.clone()) else { return Ok(()) };
        self.apply(mode, corner, &ctx)
    }

    /// Shows every native taskbar, stops the polling thread, restores the autohide bit if we enabled it, then removes the journal entries.
    pub fn restore(&mut self) -> io::Result<()> {
        self.stop_worker();
        self.current = None;
        match self.ctx.clone() {
            Some(ctx) => self.undo(&ctx),
            None => Ok(()),
        }
    }

    /// Shows again; disables autohide only if we enabled it; clears the journal only after each step succeeds.
    /// Errors (journal left intact) when it cannot be restored: no taskbar found to show (Explorer is dead) or
    /// autohide could not be turned off.
    fn undo(&mut self, ctx: &ModuleCtx) -> io::Result<()> {
        if self.we_hid {
            if self.port.taskbars().is_empty() {
                return Err(io::Error::new(io::ErrorKind::NotFound, "native taskbar not found"));
            }
            self.show_all();
        }
        if self.we_set_autohide {
            self.port.set_autohide(false)?;
            self.we_set_autohide = false;
            ctx.journal_remove(RestoreAction::DisableTaskbarAutohide)?;
        }
        if self.we_hid {
            self.we_hid = false;
            ctx.journal_remove(RestoreAction::ShowNativeTaskbar)?;
        }
        Ok(())
    }

    fn show_all(&self) {
        for t in self.port.taskbars() {
            self.port.show(t.handle);
        }
    }

    fn hide_all(&self) {
        for t in self.port.taskbars() {
            self.port.hide(t.handle);
        }
    }

    fn stop_worker(&mut self) {
        let Some(w) = self.worker.take() else { return };
        w.cancel.store(true, Ordering::SeqCst);
        let _ = w.stop.send(());
        match w.done.recv_timeout(Duration::from_millis(STOP_JOIN_MS)) {
            // Finished (or dead): join does not block.
            Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                let _ = w.join.join();
            }
            // Stuck in a system call: abandon it; `cancel` prevents it from changing the taskbar when it wakes up.
            Err(RecvTimeoutError::Timeout) => drop(w.join),
        }
    }

    fn start_worker(&mut self, mode: NativeMode, corner: Option<Corner>) -> io::Result<()> {
        let (stop, stop_rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let port = Arc::clone(&self.port);
        let fire = Arc::clone(&self.fire);
        let flag = Arc::clone(&cancel);
        let join = std::thread::Builder::new().name("sg-native-taskbar".into()).spawn(move || {
            port.init_poll_thread();
            poll_loop(&*port, &*fire, mode, corner, &stop_rx, &flag);
            let _ = done_tx.send(());
        })?;
        self.worker = Some(Worker { stop, done, cancel, join });
        Ok(())
    }
}

impl<P: NativePort> Drop for NativeTaskbar<P> {
    fn drop(&mut self) {
        // Never leave the native taskbar hidden when the object goes away.
        if self.current.is_some() || self.we_hid || self.we_set_autohide {
            let _ = self.restore();
        }
    }
}

fn poll_loop(
    port: &dyn NativePort,
    fire: &(dyn Fn() + Send + Sync),
    mode: NativeMode,
    corner: Option<Corner>,
    stop: &mpsc::Receiver<()>,
    cancel: &AtomicBool,
) {
    let reveal = mode == NativeMode::Reveal;
    let hide_mode = mode == NativeMode::Hide;
    let interval = if reveal || corner.is_some() { POLL_MS } else { HOLD_POLL_MS };
    let start = Instant::now();
    // Each taskbar (primary / secondary) has its own shown/hidden state: whichever monitor edge the mouse touches, only that monitor's taskbar is shown.
    let mut states: HashMap<Handle, RevealState> = HashMap::new();
    let mut hc = HotCornerState::default();
    loop {
        match stop.recv_timeout(Duration::from_millis(interval)) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return,
        }
        let now_ms = start.elapsed().as_millis() as u64;
        let cursor = port.cursor_pos();
        // Only change the taskbar if not cancelled (a thread that got stuck and woke up after `restore`).
        let live = || !cancel.load(Ordering::SeqCst);
        if reveal || hide_mode {
            let bars = port.taskbars();
            if reveal {
                let monitors = port.monitor_rects();
                states.retain(|h, _| bars.iter().any(|t| t.handle == *h));
                if let Some(c) = cursor {
                    // The foreground and the window under the cursor only need to be queried when a taskbar is shown but the mouse has left (each costs
                    // one OpenProcess). The window under the cursor catches the taskbar button's thumbnail, which does not take the foreground.
                    let mut flyout: Option<bool> = None;
                    for t in &bars {
                        let rs = states.get(&t.handle).copied().unwrap_or(RevealState::HIDDEN);
                        let on_edge = monitor_of(t.rect, &monitors).is_some_and(|m| cursor_on_edge(c, t.rect, m));
                        let over = t.rect.contains(c.0, c.1);
                        let flyout_open = rs.visible
                            && !on_edge
                            && !over
                            && *flyout.get_or_insert_with(|| {
                                port.foreground().is_some_and(|f| is_shell_flyout(&f.class, &f.exe))
                                    || port.window_under(c).is_some_and(|f| is_flyout_under_cursor(&f.class, &f.exe))
                            });
                        let next = reveal_step(
                            rs,
                            RevealInput { cursor_on_edge: on_edge, cursor_over_native: over, shell_flyout_open: flyout_open, now_ms },
                        );
                        if next.visible != rs.visible && live() {
                            if next.visible {
                                port.show(t.handle);
                            } else {
                                port.hide(t.handle);
                            }
                        }
                        states.insert(t.handle, next);
                    }
                }
            }
            for t in &bars {
                let shown = states.get(&t.handle).is_some_and(|s| s.visible);
                if hide_mode || !shown {
                    // Explorer re-showed it on its own (or a new taskbar appeared after a monitor was plugged in): hide it again immediately.
                    if live() && port.is_visible(t.handle) {
                        port.hide(t.handle);
                    }
                }
            }
        }
        if let (Some(corner), Some(c)) = (corner, cursor) {
            let monitors = port.monitor_rects();
            let inside = monitors.iter().any(|m| m.contains(c.0, c.1) && cursor_in_corner(c, *m, corner));
            let (next, go) = hot_corner_step(hc, inside, now_ms);
            hc = next;
            if go && live() {
                fire();
            }
        }
    }
}

pub use win32::Win32Port;

impl NativeTaskbar<Win32Port> {
    /// Real version: the hot corner opens Start like pressing the Windows key.
    pub fn system() -> Self {
        NativeTaskbar::new(Arc::new(Win32Port), Arc::new(crate::engine::open_start))
    }
}

mod win32 {
    use super::{Foreground, Handle, NativePort, TaskbarWnd};
    use crate::reveal::Rect;
    use std::io;
    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
    use windows::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible,
        SW_HIDE, SW_SHOWNOACTIVATE, ShowWindowAsync, WindowFromPoint,
    };
    use windows::core::PWSTR;

    /// The real Win32 port. Holds no state.
    pub struct Win32Port;

    fn hwnd(h: Handle) -> HWND {
        HWND(h as *mut _)
    }

    unsafe extern "system" fn collect_monitor(_: HMONITOR, _: HDC, r: *mut RECT, data: LPARAM) -> windows::core::BOOL {
        // SAFETY: `data` is a pointer to the `Vec<Rect>` passed in by `monitor_rects`; `r` is valid during the callback.
        unsafe {
            let out = &mut *(data.0 as *mut Vec<Rect>);
            let r = &*r;
            out.push(Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom });
        }
        true.into()
    }

    /// The window class and the exe name of the process owning `h`; any error → `None`.
    fn describe(h: HWND) -> Option<Foreground> {
        if h.is_invalid() {
            return None;
        }
        // SAFETY: read-only; `h` may have disappeared midway (the calls return errors → None); the buffer lives for the whole call.
        unsafe {
            let mut cls = [0u16; 128];
            let n = GetClassNameW(h, &mut cls);
            if n <= 0 {
                return None;
            }
            let class = String::from_utf16_lossy(&cls[..n as usize]);
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            let proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 520];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(proc, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
            let _ = CloseHandle(proc);
            ok.then(|| Foreground { class, exe: String::from_utf16_lossy(&buf[..len as usize]) })
        }
    }

    impl NativePort for Win32Port {
        fn taskbars(&self) -> Vec<TaskbarWnd> {
            sg_core::platform::native_taskbars()
                .into_iter()
                .map(|t| TaskbarWnd {
                    handle: t.hwnd,
                    rect: Rect { left: t.rect.0, top: t.rect.1, right: t.rect.2, bottom: t.rect.3 },
                })
                .collect()
        }

        fn show(&self, h: Handle) {
            // SAFETY: the HWND was just enumerated; if it has disappeared, the call just returns an error. `ShowWindowAsync` only posts a request so it does not block
            // when Explorer hangs; SHOWNOACTIVATE does not steal the game's focus (and with autohide on, Explorer slides it in).
            unsafe {
                let _ = ShowWindowAsync(hwnd(h), SW_SHOWNOACTIVATE);
            }
        }

        fn hide(&self, h: Handle) {
            // SAFETY: same as `show`.
            unsafe {
                let _ = ShowWindowAsync(hwnd(h), SW_HIDE);
            }
        }

        fn is_visible(&self, h: Handle) -> bool {
            // SAFETY: read-only.
            unsafe { IsWindowVisible(hwnd(h)).as_bool() }
        }

        fn autohide(&self) -> Option<bool> {
            sg_core::platform::taskbar_autohide()
        }

        fn set_autohide(&self, on: bool) -> io::Result<()> {
            sg_core::platform::set_taskbar_autohide(on)
        }

        fn cursor_pos(&self) -> Option<(i32, i32)> {
            let mut p = POINT::default();
            // SAFETY: `p` lives for the whole call. On error (e.g. a locked screen) → None.
            unsafe { GetCursorPos(&mut p).ok().map(|()| (p.x, p.y)) }
        }

        fn monitor_rects(&self) -> Vec<Rect> {
            let mut out: Vec<Rect> = Vec::new();
            // SAFETY: the `out` pointer is only used during the call (EnumDisplayMonitors runs synchronously).
            unsafe {
                let _ = EnumDisplayMonitors(None, None, Some(collect_monitor), LPARAM(&mut out as *mut Vec<Rect> as isize));
            }
            out
        }

        fn foreground(&self) -> Option<Foreground> {
            // SAFETY: read-only; any error → None.
            unsafe { describe(GetForegroundWindow()) }
        }

        fn window_under(&self, pt: (i32, i32)) -> Option<Foreground> {
            // SAFETY: read-only; if the window disappears midway, later calls return errors → None.
            unsafe {
                let h = WindowFromPoint(POINT { x: pt.0, y: pt.1 });
                if h.is_invalid() {
                    return None;
                }
                describe(GetAncestor(h, GA_ROOT))
            }
        }

        fn init_poll_thread(&self) {
            // SAFETY: only changes the current thread's DPI context, so mouse/monitor/taskbar coordinates share the same physical unit.
            unsafe {
                SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sg_core::guard::GuardJournal;
    use sg_core::paths::Paths;
    use std::sync::atomic::{AtomicU64, AtomicUsize};
    use std::sync::{Mutex, MutexGuard};

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    const MON: Rect = Rect { left: 0, top: 0, right: 1920, bottom: 1080 };
    const MON2: Rect = Rect { left: 1920, top: 0, right: 3840, bottom: 1080 };
    const BAR: Rect = Rect { left: 0, top: 1040, right: 1920, bottom: 1080 };
    const BAR2: Rect = Rect { left: 1920, top: 1040, right: 3840, bottom: 1080 };

    struct State {
        bars: Vec<(Handle, Rect, bool)>,
        monitors: Vec<Rect>,
        cursor: Option<(i32, i32)>,
        fg: Option<Foreground>,
        /// The root window under the cursor (the native taskbar's thumbnail/jump list).
        under: Option<Foreground>,
        autohide: bool,
        shows: usize,
        hides: usize,
        hides_with_journal: usize,
        /// (enabled?, did the journal already contain `DisableTaskbarAutohide` at that time?)
        autohide_calls: Vec<(bool, bool)>,
        /// Every autohide set must happen before the very first hide.
        hide_before_autohide: bool,
        /// Simulates `set_autohide(false)` failing (SHAppBarMessage stuck/failed).
        fail_autohide_off: bool,
    }

    struct Fake {
        journal: Arc<Mutex<GuardJournal>>,
        st: Mutex<State>,
        cursor_delay_ms: AtomicU64,
    }

    impl Fake {
        fn visible(&self) -> Vec<bool> {
            lock(&self.st).bars.iter().map(|b| b.2).collect()
        }
        fn set_cursor(&self, c: Option<(i32, i32)>) {
            lock(&self.st).cursor = c;
        }
    }

    impl NativePort for Fake {
        fn taskbars(&self) -> Vec<TaskbarWnd> {
            lock(&self.st).bars.iter().map(|b| TaskbarWnd { handle: b.0, rect: b.1 }).collect()
        }
        fn show(&self, h: Handle) {
            let mut s = lock(&self.st);
            s.shows += 1;
            s.bars.iter_mut().filter(|b| b.0 == h).for_each(|b| b.2 = true);
        }
        fn hide(&self, h: Handle) {
            // Required order: at this point the journal must already contain the restore entry.
            let has = lock(&self.journal).actions().contains(&RestoreAction::ShowNativeTaskbar);
            let mut s = lock(&self.st);
            s.hides += 1;
            if has {
                s.hides_with_journal += 1;
            }
            if s.autohide_calls.is_empty() && !s.autohide {
                s.hide_before_autohide = true;
            }
            s.bars.iter_mut().filter(|b| b.0 == h).for_each(|b| b.2 = false);
        }
        fn is_visible(&self, h: Handle) -> bool {
            lock(&self.st).bars.iter().any(|b| b.0 == h && b.2)
        }
        fn autohide(&self) -> Option<bool> {
            Some(lock(&self.st).autohide)
        }
        fn set_autohide(&self, on: bool) -> io::Result<()> {
            let has = lock(&self.journal).actions().contains(&RestoreAction::DisableTaskbarAutohide);
            let mut s = lock(&self.st);
            if !on && s.fail_autohide_off {
                return Err(io::Error::other("SHAppBarMessage failed"));
            }
            s.autohide_calls.push((on, has));
            s.autohide = on;
            Ok(())
        }
        fn cursor_pos(&self) -> Option<(i32, i32)> {
            let d = self.cursor_delay_ms.load(Ordering::SeqCst);
            if d > 0 {
                std::thread::sleep(Duration::from_millis(d));
            }
            lock(&self.st).cursor
        }
        fn monitor_rects(&self) -> Vec<Rect> {
            lock(&self.st).monitors.clone()
        }
        fn foreground(&self) -> Option<Foreground> {
            lock(&self.st).fg.clone()
        }
        fn window_under(&self, _: (i32, i32)) -> Option<Foreground> {
            lock(&self.st).under.clone()
        }
    }

    struct Rig {
        _dir: tempfile::TempDir,
        journal: Arc<Mutex<GuardJournal>>,
        ctx: ModuleCtx,
        port: Arc<Fake>,
        fired: Arc<AtomicUsize>,
        nt: NativeTaskbar<Fake>,
    }

    fn rig_with(autohide: bool) -> Rig {
        rig_custom(autohide, vec![(1, BAR, true), (2, BAR2, true)], vec![MON, MON2])
    }

    fn rig_custom(autohide: bool, bars: Vec<(Handle, Rect, bool)>, monitors: Vec<Rect>) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let journal = Arc::new(Mutex::new(GuardJournal::open(&dir.path().join("guard.toml"))));
        let paths = Paths::resolve(dir.path(), true, None, None);
        let ctx = ModuleCtx::new(paths, Arc::clone(&journal), Arc::new(|_, _| {}));
        let port = Arc::new(Fake {
            journal: Arc::clone(&journal),
            st: Mutex::new(State {
                bars,
                monitors,
                cursor: Some((900, 500)),
                fg: None,
                under: None,
                autohide,
                shows: 0,
                hides: 0,
                hides_with_journal: 0,
                autohide_calls: Vec::new(),
                hide_before_autohide: false,
                fail_autohide_off: false,
            }),
            cursor_delay_ms: AtomicU64::new(0),
        });
        let fired = Arc::new(AtomicUsize::new(0));
        let f = Arc::clone(&fired);
        let nt = NativeTaskbar::new(
            Arc::clone(&port),
            Arc::new(move || {
                f.fetch_add(1, Ordering::SeqCst);
            }),
        );
        Rig { _dir: dir, journal, ctx, port, fired, nt }
    }

    fn rig() -> Rig {
        rig_with(false)
    }

    fn wait_for(mut cond: impl FnMut() -> bool, limit_ms: u64) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(limit_ms) {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        cond()
    }

    #[test]
    fn hide_records_journal_first() {
        let mut r = rig();
        r.nt.apply(NativeMode::Hide, None, &r.ctx).unwrap();
        {
            let s = lock(&r.port.st);
            assert_eq!(s.hides, 2, "ẩn cả taskbar chính lẫn taskbar phụ");
            assert_eq!(s.hides_with_journal, 2, "mọi lần ẩn phải xảy ra SAU khi ghi nhật ký");
            assert_eq!(s.autohide_calls, vec![(true, true)], "bật tự ẩn một lần, sau khi ghi DisableTaskbarAutohide");
            assert!(!s.hide_before_autohide, "bật tự ẩn trước khi ẩn");
        }
        assert_eq!(
            lock(&r.journal).actions(),
            vec![RestoreAction::ShowNativeTaskbar, RestoreAction::DisableTaskbarAutohide]
        );
        assert_eq!(r.port.visible(), vec![false, false]);
        r.nt.restore().unwrap();
        assert_eq!(r.port.visible(), vec![true, true]);
        {
            let s = lock(&r.port.st);
            assert!(!s.autohide, "ta bật thì ta tắt");
            assert_eq!(s.autohide_calls.last(), Some(&(false, true)), "tắt tự ẩn khi nhật ký còn việc (chỉ xoá sau)");
        }
        assert!(lock(&r.journal).actions().is_empty(), "restore xoá nhật ký sau khi đã hiện lại");
    }

    #[test]
    fn restore_without_any_taskbar_is_an_error_and_keeps_the_journal() {
        let mut r = rig();
        r.nt.apply(NativeMode::Hide, None, &r.ctx).unwrap();
        // Explorer is dead: no taskbar is left to show again → it must not be treated as restored.
        let saved = std::mem::take(&mut lock(&r.port.st).bars);
        let e = r.nt.restore().unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert!(lock(&r.journal).actions().contains(&RestoreAction::ShowNativeTaskbar), "nhật ký phải được giữ");
        // Explorer has a taskbar again (hidden, as we hid it): the retry succeeds and only then is the journal cleared.
        lock(&r.port.st).bars = saved;
        r.nt.restore().unwrap();
        assert_eq!(r.port.visible(), vec![true, true]);
        assert!(lock(&r.journal).actions().is_empty());
    }

    #[test]
    fn failed_autohide_restore_keeps_both_journal_entries_and_retries() {
        let mut r = rig();
        r.nt.apply(NativeMode::Hide, None, &r.ctx).unwrap();
        lock(&r.port.st).fail_autohide_off = true;
        assert!(r.nt.restore().is_err());
        assert_eq!(
            lock(&r.journal).actions(),
            vec![RestoreAction::ShowNativeTaskbar, RestoreAction::DisableTaskbarAutohide],
            "tắt tự ẩn lỗi: nhật ký không được xoá"
        );
        lock(&r.port.st).fail_autohide_off = false;
        r.nt.restore().unwrap();
        assert!(!lock(&r.port.st).autohide);
        assert!(lock(&r.journal).actions().is_empty());
    }

    #[test]
    fn leftover_disable_autohide_in_journal_is_adopted_as_ours() {
        // The last restore failed: autohide is on and the journal still has DisableTaskbarAutohide.
        let mut r = rig_with(true);
        r.ctx.journal_add(RestoreAction::DisableTaskbarAutohide).unwrap();
        r.nt.apply(NativeMode::Reveal, None, &r.ctx).unwrap();
        r.nt.restore().unwrap();
        let s = lock(&r.port.st);
        assert!(!s.autohide, "tự ẩn là do ta bật từ lần trước nên lần này vẫn phải tắt, không tưởng là của người dùng");
        drop(s);
        assert!(lock(&r.journal).actions().is_empty());
    }

    #[test]
    fn user_autohide_is_never_touched() {
        let mut r = rig_with(true);
        r.nt.apply(NativeMode::Reveal, None, &r.ctx).unwrap();
        assert_eq!(lock(&r.journal).actions(), vec![RestoreAction::ShowNativeTaskbar]);
        r.nt.restore().unwrap();
        let s = lock(&r.port.st);
        assert!(s.autohide, "người dùng đã bật tự ẩn từ trước: không bao giờ tắt");
        assert!(s.autohide_calls.is_empty());
        drop(s);
        assert!(lock(&r.journal).actions().is_empty());
    }

    #[test]
    fn keep_mode_touches_nothing() {
        let mut r = rig();
        r.nt.apply(NativeMode::Keep, None, &r.ctx).unwrap();
        let s = lock(&r.port.st);
        assert_eq!((s.hides, s.shows), (0, 0), "chưa ẩn gì thì không ép hiện");
        assert!(s.autohide_calls.is_empty());
        drop(s);
        assert!(lock(&r.journal).actions().is_empty());
    }

    #[test]
    fn switching_hide_to_keep_restores_everything() {
        let mut r = rig();
        r.nt.apply(NativeMode::Hide, None, &r.ctx).unwrap();
        r.nt.apply(NativeMode::Keep, None, &r.ctx).unwrap();
        assert_eq!(r.port.visible(), vec![true, true]);
        assert!(!lock(&r.port.st).autohide);
        assert!(lock(&r.journal).actions().is_empty());
        assert!(r.nt.worker.is_none());
    }

    #[test]
    fn reapply_hides_a_taskbar_that_came_back_and_reasserts_autohide() {
        let mut r = rig();
        r.nt.apply(NativeMode::Hide, None, &r.ctx).unwrap();
        // Explorer restarts: the new taskbar is shown, the old taskbar is shown again, the autohide setting is lost
        {
            let mut s = lock(&r.port.st);
            s.bars[0].2 = true;
            s.bars.push((3, BAR, true));
            s.autohide = false;
        }
        r.nt.reapply().unwrap();
        assert_eq!(r.port.visible(), vec![false, false, false]);
        let s = lock(&r.port.st);
        assert!(s.autohide, "bật lại tự ẩn vì ta sở hữu nó");
        assert_eq!(s.hides, s.hides_with_journal);
    }

    #[test]
    fn reapply_before_apply_is_noop() {
        let mut r = rig();
        r.nt.reapply().unwrap();
        assert_eq!(lock(&r.port.st).hides, 0);
    }

    #[test]
    fn hide_mode_worker_hides_again() {
        let mut r = rig();
        r.nt.apply(NativeMode::Hide, None, &r.ctx).unwrap();
        lock(&r.port.st).bars[1].2 = true;
        assert!(wait_for(|| !r.port.visible()[1], 1000), "chế độ Ẩn phải ẩn lại khi Explorer hiện taskbar");
        let s = lock(&r.port.st);
        assert_eq!(s.hides, s.hides_with_journal);
    }

    #[test]
    fn reveal_shows_on_edge_and_hides_after_leaving() {
        let mut r = rig();
        r.nt.apply(NativeMode::Reveal, None, &r.ctx).unwrap();
        assert_eq!(r.port.visible(), vec![false, false]);
        r.port.set_cursor(Some((500, 1079)));
        assert!(wait_for(|| r.port.visible()[0], 300), "phải hiện ngay khi chạm mép");
        // the mouse goes onto the taskbar and then opens Start (a shell flyout): it stays shown
        r.port.set_cursor(Some((500, 600)));
        lock(&r.port.st).fg = Some(Foreground {
            class: "Windows.UI.Core.CoreWindow".into(),
            exe: "C:\\Windows\\SystemApps\\StartMenuExperienceHost.exe".into(),
        });
        std::thread::sleep(Duration::from_millis(450));
        assert!(r.port.visible()[0], "flyout đang mở thì không được biến mất");
        // close the flyout: hidden again after ~300 ms
        lock(&r.port.st).fg = None;
        assert!(wait_for(|| !r.port.visible()[0], 1500));
        r.nt.restore().unwrap();
        assert_eq!(r.port.visible(), vec![true, true]);
    }

    /// Secondary taskbar that is auto-hiding: the measured rect slid almost off the monitor (top 1078 on a 1080 monitor), its center falls outside every monitor.
    const BAR2_SLID: Rect = Rect { left: 1920, top: 1078, right: 3840, bottom: 1126 };

    #[test]
    fn reveal_on_second_monitor_edge() {
        let mut r = rig_custom(false, vec![(1, BAR, true), (2, BAR2_SLID, true)], vec![MON, MON2]);
        r.nt.apply(NativeMode::Reveal, None, &r.ctx).unwrap();
        // touch the bottom edge of monitor 2, where the secondary taskbar is
        r.port.set_cursor(Some((2500, 1079)));
        assert!(wait_for(|| r.port.visible()[1], 300));
        // only that monitor's taskbar is shown; the primary taskbar on the other monitor stays hidden
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(r.port.visible(), vec![false, true], "chỉ hiện taskbar của màn hình chuột chạm mép");
        // the bottom edge of monitor 1 corresponds only to the primary taskbar, not to monitor 1's right edge
        r.port.set_cursor(Some((1919, 500)));
        assert!(wait_for(|| !r.port.visible()[1], 1500));
        r.nt.restore().unwrap();
    }

    #[test]
    fn hovering_a_taskbar_thumbnail_keeps_the_native_taskbar_visible() {
        let mut r = rig();
        r.nt.apply(NativeMode::Reveal, None, &r.ctx).unwrap();
        r.port.set_cursor(Some((500, 1079)));
        assert!(wait_for(|| r.port.visible()[0], 300));
        // the mouse hovers over a button's thumbnail (outside the taskbar rect, foreground unchanged): the root window under the cursor is
        // explorer's TaskListThumbnailWnd → still on the native taskbar
        r.port.set_cursor(Some((500, 900)));
        lock(&r.port.st).under = Some(Foreground { class: "TaskListThumbnailWnd".into(), exe: "C:\\Windows\\explorer.exe".into() });
        std::thread::sleep(Duration::from_millis(450));
        assert!(r.port.visible()[0], "đang rê trên thumbnail thì không được ẩn");
        // leave the thumbnail for another app window: hidden again after ~300 ms
        lock(&r.port.st).under = Some(Foreground { class: "Notepad".into(), exe: "notepad.exe".into() });
        assert!(wait_for(|| !r.port.visible()[0], 1500));
        r.nt.restore().unwrap();
    }

    #[test]
    fn moving_to_another_monitors_edge_hides_the_previous_native_taskbar() {
        let mut r = rig();
        r.nt.apply(NativeMode::Reveal, None, &r.ctx).unwrap();
        r.port.set_cursor(Some((2500, 1079)));
        assert!(wait_for(|| r.port.visible()[1], 300));
        // the mouse moves to monitor 1's edge: the root window under the cursor is monitor 1's taskbar, not a flyout holding taskbar 2
        lock(&r.port.st).under = Some(Foreground { class: "Shell_TrayWnd".into(), exe: "C:\\Windows\\explorer.exe".into() });
        r.port.set_cursor(Some((500, 1079)));
        assert!(wait_for(|| r.port.visible()[0], 300));
        assert!(wait_for(|| !r.port.visible()[1], 1500), "taskbar màn hình 2 phải ẩn dù taskbar màn hình 1 nằm dưới con trỏ");
        r.nt.restore().unwrap();
    }

    #[test]
    fn reveal_with_two_monitors_of_different_sizes_touches_only_that_monitor() {
        // 1920x1080 on the left + 2560x1440 on the right; both taskbars are auto-hiding (slid almost off their own monitor)
        let left_mon = MON;
        let right_mon = Rect { left: 1920, top: 0, right: 4480, bottom: 1440 };
        let left_bar = Rect { left: 0, top: 1078, right: 1920, bottom: 1126 };
        let right_bar = Rect { left: 1920, top: 1438, right: 4480, bottom: 1486 };
        let mut r = rig_custom(false, vec![(1, left_bar, true), (2, right_bar, true)], vec![left_mon, right_mon]);
        r.nt.apply(NativeMode::Reveal, None, &r.ctx).unwrap();
        assert_eq!(r.port.visible(), vec![false, false]);
        // the bottom edge of the right monitor (y = 1439, lower than the left monitor): only the right taskbar is shown
        r.port.set_cursor(Some((3000, 1439)));
        assert!(wait_for(|| r.port.visible()[1], 300), "taskbar màn hình lớn phải hiện");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(r.port.visible(), vec![false, true]);
        // to the bottom edge of the left monitor: the left taskbar is shown, the right taskbar is hidden after the mouse leaves
        r.port.set_cursor(Some((500, 1079)));
        assert!(wait_for(|| r.port.visible()[0], 300), "taskbar màn hình nhỏ hiện");
        assert!(wait_for(|| !r.port.visible()[1], 1500));
        assert_eq!(r.port.visible(), vec![true, false]);
        r.nt.restore().unwrap();
    }

    #[test]
    fn monitor_is_chosen_by_largest_overlap_then_nearest() {
        let left = MON;
        let right = Rect { left: 1920, top: 0, right: 4480, bottom: 1440 };
        let mons = [left, right];
        // entirely within one monitor
        assert_eq!(monitor_of(BAR, &mons), Some(left));
        // an auto-hiding taskbar, center outside every monitor but still touching its own monitor
        assert_eq!(monitor_of(Rect { left: 1920, top: 1438, right: 4480, bottom: 1486 }, &mons), Some(right));
        assert_eq!(monitor_of(Rect { left: 0, top: 1078, right: 1920, bottom: 1126 }, &mons), Some(left));
        // spanning two monitors: the larger overlap wins
        assert_eq!(monitor_of(Rect { left: 1400, top: 1000, right: 2200, bottom: 1040 }, &mons), Some(left));
        assert_eq!(monitor_of(Rect { left: 1700, top: 1000, right: 2900, bottom: 1040 }, &mons), Some(right));
        // entirely outside every monitor: the nearest monitor
        assert_eq!(monitor_of(Rect { left: 0, top: 1200, right: 1000, bottom: 1248 }, &mons), Some(left));
        assert_eq!(monitor_of(Rect { left: 4500, top: 0, right: 4548, bottom: 1440 }, &mons), Some(right));
        assert_eq!(monitor_of(BAR, &[]), None);
    }

    #[test]
    fn reveal_rehides_when_windows_shows_it_again() {
        let mut r = rig();
        r.nt.apply(NativeMode::Reveal, None, &r.ctx).unwrap();
        lock(&r.port.st).bars[1].2 = true;
        assert!(wait_for(|| !r.port.visible()[1], 300));
    }

    #[test]
    fn stop_joins_quickly_and_is_restartable() {
        let mut r = rig();
        for _ in 0..3 {
            r.nt.apply(NativeMode::Reveal, Some(Corner::TopRight), &r.ctx).unwrap();
            let t = Instant::now();
            r.nt.restore().unwrap();
            assert!(t.elapsed() < Duration::from_millis(250), "dừng luồng mất {:?}", t.elapsed());
            assert_eq!(r.port.visible(), vec![true, true]);
        }
        assert!(r.nt.worker.is_none());
    }

    #[test]
    fn hung_poll_thread_does_not_block_restore() {
        let mut r = rig();
        r.nt.apply(NativeMode::Reveal, None, &r.ctx).unwrap();
        // the polling thread is stuck for 1.5 s in a system call
        r.port.cursor_delay_ms.store(1500, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        let t = Instant::now();
        r.nt.restore().unwrap();
        assert!(t.elapsed() < Duration::from_millis(1100), "restore bị chặn {:?}", t.elapsed());
        assert_eq!(r.port.visible(), vec![true, true]);
        assert!(lock(&r.journal).actions().is_empty());
        // the abandoned thread wakes up: it must not hide the already-shown taskbar again
        r.port.cursor_delay_ms.store(0, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(1600));
        assert_eq!(r.port.visible(), vec![true, true]);
    }

    #[test]
    fn dropping_restores_the_taskbar() {
        let r = rig();
        let (port, ctx, journal) = (Arc::clone(&r.port), r.ctx.clone(), Arc::clone(&r.journal));
        let mut nt = r.nt;
        nt.apply(NativeMode::Reveal, None, &ctx).unwrap();
        drop(nt);
        assert_eq!(port.visible(), vec![true, true]);
        assert!(!lock(&port.st).autohide);
        assert!(lock(&journal).actions().is_empty());
    }

    #[test]
    fn hot_corner_thread_fires_once_per_visit() {
        let mut r = rig();
        r.nt.apply(NativeMode::Keep, Some(Corner::TopRight), &r.ctx).unwrap();
        r.port.set_cursor(Some((1919, 0)));
        assert!(wait_for(|| r.fired.load(Ordering::SeqCst) == 1, 1000));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(r.fired.load(Ordering::SeqCst), 1, "đứng yên trong góc chỉ bắn một lần");
        r.port.set_cursor(Some((900, 500)));
        std::thread::sleep(Duration::from_millis(100));
        r.port.set_cursor(Some((1919, 0)));
        assert!(wait_for(|| r.fired.load(Ordering::SeqCst) == 2, 1000));
        r.nt.restore().unwrap();
        assert_eq!(lock(&r.port.st).shows, 0, "Giữ + góc nóng: không đụng taskbar");
    }
}
