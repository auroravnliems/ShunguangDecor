//! App side of `shunguang_explorer.dll`: extracts the embedded DLL, loads it into `explorer.exe`, runs the pipe server,
//! and watches for Explorer restarts. Used by the desktop labels module.
//!
//! How it loads: a `WH_CALLWNDPROC` hook for Explorer's desktop thread only (the hook function lives in the DLL), triggered by a
//! message, then wait for the DLL to connect the pipe, then remove the hook. It does **not** use `CreateRemoteThread`/`WriteProcessMemory`, does not read other
//! processes' memory, and targets only the `explorer.exe` that owns `SHELLDLL_DefView`.
//!
//! The desired state (label paint table, taskbar background) is kept here: when the DLL reconnects (app reopened, Explorer restarted)
//! the server resends `Hello` + that state by itself.

// The server/DLL loading exists only on Windows; elsewhere most of the state is never read.
#![cfg_attr(not(windows), allow(dead_code))]

pub mod embed;
mod inject;
mod server;
mod watch;

use sg_core::paths::Paths;
use sg_explorer_proto::{FromDll, LabelsPaint, PROTO_VERSION, ToDll};
use std::collections::VecDeque;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// The DLL must connect the pipe within this time after the hook; past that it is considered blocked (antivirus…) → fallback.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// A DLL already resident in Explorer (from a previous run) reconnects by itself every 2 s; wait this long for it before concluding.
pub const RECONNECT_WAIT: Duration = Duration::from_millis(3500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    /// This build does not embed the DLL (`npm run hook` has not been run).
    NoDll,
    /// Could not be loaded (desktop not found, file not writable, hook refused…).
    HookFailed(String),
    /// The DLL did not connect the pipe in time (usually blocked by antivirus software).
    Timeout,
    /// Explorer already holds a DLL from another app build: Explorer needs to be restarted.
    VersionMismatch,
    /// This operating system has no Explorer.
    Unsupported,
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BridgeError::NoDll => write!(f, "this build has no embedded explorer DLL"),
            BridgeError::HookFailed(e) => write!(f, "cannot load the DLL into explorer: {e}"),
            BridgeError::Timeout => write!(f, "the DLL did not connect in time (blocked?)"),
            BridgeError::VersionMismatch => write!(f, "an older DLL is loaded in explorer; explorer must restart"),
            BridgeError::Unsupported => write!(f, "not supported on this OS"),
        }
    }
}

/// The DLL introduces itself when it connects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DllHello {
    pub version: u32,
    pub pid: u32,
    pub build: String,
}

/// Notifies the modules using the bridge (called on the server/watcher thread; must not block for long).
#[derive(Debug, Clone, PartialEq)]
pub enum BridgeEvent {
    Connected(DllHello),
    Disconnected,
    Message(FromDll),
    /// Explorer restarted (or rebuilt the desktop): the DLL has been reloaded if some module needs it.
    ExplorerRestarted,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BridgeStatus {
    pub connected: bool,
    pub explorer_pid: Option<u32>,
    pub dll_build: Option<String>,
    /// The DLL in Explorer has a different protocol version: Explorer needs to be restarted.
    pub needs_explorer_restart: bool,
    pub last_error: Option<String>,
}

/// What the app wants the DLL to apply. Resent in full every time the DLL (re)connects.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Desired {
    pub labels: Option<LabelsPaint>,
}

impl Desired {
    pub fn record(&mut self, msg: &ToDll) {
        match msg {
            ToDll::Labels(p) => self.labels = Some(p.clone()),
            ToDll::LabelsOff => self.labels = None,
            _ => {}
        }
    }

    pub fn any(&self) -> bool {
        self.labels.is_some()
    }

    pub fn replay(&self) -> Vec<ToDll> {
        let mut v = Vec::new();
        if let Some(p) = &self.labels {
            v.push(ToDll::Labels(p.clone()));
        }
        v
    }
}

/// Messages sent as soon as the DLL connects: `Hello`, then the entire desired state.
pub fn connect_messages(desired: &Desired, log_path: Option<String>) -> Vec<ToDll> {
    let mut v = vec![ToDll::Hello { version: PROTO_VERSION, log_path }];
    v.extend(desired.replay());
    v
}

type Sub = Arc<dyn Fn(&BridgeEvent) + Send + Sync>;

#[derive(Default)]
pub(crate) struct St {
    pub desired: Desired,
    pub connected: bool,
    pub hello: Option<DllHello>,
    pub mismatch: bool,
    pub last_error: Option<String>,
}

pub(crate) struct Shared {
    pub st: Mutex<St>,
    pub cv: Condvar,
    pub subs: Mutex<Vec<(u64, Sub)>>,
    /// Queue of frames waiting for the server thread to write to the pipe.
    pub outgoing: Mutex<VecDeque<Vec<u8>>>,
    pub log_path: Option<String>,
    pub pipe_name: String,
    /// Accept only `explorer.exe` as the client (off in tests, where the client is the test process itself).
    pub require_explorer_client: bool,
    pub wake: server::WakeEvent,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    pub fn emit(&self, ev: &BridgeEvent) {
        let subs: Vec<Sub> = lock(&self.subs).iter().map(|(_, s)| s.clone()).collect();
        for s in subs {
            s(ev);
        }
    }

    /// Server thread: a client just connected. Snapshots the state + marks it connected under the same lock, so a `send`
    /// command arriving afterwards always lands after the resent state.
    pub fn on_client_connected(&self) -> Vec<ToDll> {
        let mut st = lock(&self.st);
        st.connected = true;
        st.hello = None;
        lock(&self.outgoing).clear();
        connect_messages(&st.desired, self.log_path.clone())
    }

    pub fn on_message(&self, msg: FromDll) {
        match &msg {
            FromDll::Hello { version, pid, build } => {
                let hello = DllHello { version: *version, pid: *pid, build: build.clone() };
                {
                    let mut st = lock(&self.st);
                    st.mismatch = *version != PROTO_VERSION;
                    st.hello = Some(hello.clone());
                    if st.mismatch {
                        st.last_error = Some(BridgeError::VersionMismatch.to_string());
                    }
                }
                self.cv.notify_all();
                tracing::info!(target: "sg::explorer", "DLL connected: build {build}, protocol {version}, explorer pid {pid}");
                self.emit(&BridgeEvent::Connected(hello));
            }
            FromDll::Status { error: Some(e), .. } => {
                {
                    let mut st = lock(&self.st);
                    if e == "version-mismatch" {
                        st.mismatch = true;
                    }
                    st.last_error = Some(e.clone());
                }
                self.cv.notify_all();
                tracing::warn!(target: "sg::explorer", "DLL reports: {e}");
                self.emit(&BridgeEvent::Message(msg));
            }
            _ => self.emit(&BridgeEvent::Message(msg)),
        }
    }

    pub fn on_client_disconnected(&self) {
        {
            let mut st = lock(&self.st);
            st.connected = false;
            st.hello = None;
        }
        lock(&self.outgoing).clear();
        self.cv.notify_all();
        tracing::info!(target: "sg::explorer", "DLL disconnected");
        self.emit(&BridgeEvent::Disconnected);
    }

    /// Waits for the DLL's greeting (whether it answers or not).
    pub fn wait_hello(&self, timeout: Duration) -> Option<DllHello> {
        let st = lock(&self.st);
        let (st, _) = self.cv.wait_timeout_while(st, timeout, |s| s.hello.is_none()).unwrap_or_else(|e| e.into_inner());
        st.hello.clone()
    }
}

/// Folder holding the DLL extracted from the exe for Explorer to load (`%LOCALAPPDATA%\ShunguangDecor\bin`). The uninstaller cleans it too.
pub fn bin_dir(paths: &Paths) -> PathBuf {
    paths.local.join("bin")
}

pub struct ExplorerBridge {
    shared: Arc<Shared>,
    dll: &'static [u8],
    bin_dir: PathBuf,
    /// Only one load at a time.
    load_lock: Mutex<()>,
    started: Mutex<bool>,
    /// The Explorer-restart watcher is running (independent of loading the DLL: the YASB taskbar part only needs this).
    watching: AtomicBool,
    next_sub: AtomicU64,
}

impl ExplorerBridge {
    /// `dll`: the contents of `shunguang_explorer.dll` embedded in the exe (empty = a build without the DLL).
    pub fn new(dll: &'static [u8], paths: &Paths) -> Arc<ExplorerBridge> {
        let log = paths.logs_dir().join("explorer-hook.log");
        let b = ExplorerBridge::with_pipe_name(dll, paths, default_pipe_name(), Some(log.to_string_lossy().to_string()), true);
        // Clean up DLLs from older builds (a file currently loaded in Explorer is left until next time).
        let keep = (!dll.is_empty()).then(|| b.bin_dir.join(embed::dll_file_name(env!("CARGO_PKG_VERSION"), dll)));
        let n = embed::clean_old(&b.bin_dir, keep.as_deref());
        if n > 0 {
            tracing::info!(target: "sg::explorer", "removed {n} old DLL file(s)");
        }
        b
    }

    pub fn with_pipe_name(
        dll: &'static [u8],
        paths: &Paths,
        pipe_name: String,
        log_path: Option<String>,
        require_explorer_client: bool,
    ) -> Arc<ExplorerBridge> {
        Arc::new(ExplorerBridge {
            shared: Arc::new(Shared {
                st: Mutex::new(St::default()),
                cv: Condvar::new(),
                subs: Mutex::new(Vec::new()),
                outgoing: Mutex::new(VecDeque::new()),
                log_path,
                pipe_name,
                require_explorer_client,
                wake: server::WakeEvent::new(),
            }),
            dll,
            bin_dir: bin_dir(paths),
            load_lock: Mutex::new(()),
            started: Mutex::new(false),
            watching: AtomicBool::new(false),
            next_sub: AtomicU64::new(1),
        })
    }

    pub fn has_dll(&self) -> bool {
        !self.dll.is_empty()
    }

    /// Subscribes to bridge events. Returns an ID for `unsubscribe`.
    pub fn subscribe(&self, f: impl Fn(&BridgeEvent) + Send + Sync + 'static) -> u64 {
        let id = self.next_sub.fetch_add(1, Ordering::Relaxed);
        lock(&self.shared.subs).push((id, Arc::new(f)));
        id
    }

    pub fn unsubscribe(&self, id: u64) {
        lock(&self.shared.subs).retain(|(i, _)| *i != id);
    }

    pub fn is_connected(&self) -> bool {
        lock(&self.shared.st).hello.as_ref().is_some_and(|h| h.version == PROTO_VERSION)
    }

    pub fn status(&self) -> BridgeStatus {
        let st = lock(&self.shared.st);
        BridgeStatus {
            connected: st.hello.is_some() && !st.mismatch,
            explorer_pid: st.hello.as_ref().map(|h| h.pid),
            dll_build: st.hello.as_ref().map(|h| h.build.clone()),
            needs_explorer_restart: st.mismatch,
            last_error: st.last_error.clone(),
        }
    }

    /// Sends a command to the DLL. State commands (Labels/LabelsOff) are remembered and resent when the DLL reconnects.
    pub fn send(&self, msg: ToDll) {
        let mut st = lock(&self.shared.st);
        st.desired.record(&msg);
        if st.connected {
            lock(&self.shared.outgoing).push_back(sg_explorer_proto::encode(&msg));
            drop(st);
            self.shared.wake.set();
        }
    }

    /// Ensures the DLL is inside Explorer and has connected the pipe (loading it if needed). May take up to ~5 s.
    pub fn ensure_loaded(self: &Arc<Self>) -> Result<(), BridgeError> {
        if self.dll.is_empty() {
            return Err(BridgeError::NoDll);
        }
        self.start()?;
        let _g = lock(&self.load_lock);
        let r = self.load_locked();
        if let Err(e) = &r {
            lock(&self.shared.st).last_error = Some(e.to_string());
            tracing::warn!(target: "sg::explorer", "ensure_loaded: {e}");
        }
        r
    }

    fn start(self: &Arc<Self>) -> Result<(), BridgeError> {
        let mut started = lock(&self.started);
        if *started {
            return Ok(());
        }
        server::spawn(self.shared.clone()).map_err(BridgeError::HookFailed)?;
        self.ensure_watching();
        *started = true;
        Ok(())
    }

    /// Starts the `TaskbarCreated` + desktop-change watcher WITHOUT loading the DLL: the YASB taskbar part (the custom bar) needs to know when Explorer
    /// restarts so it can hide the original taskbar again, even if no module uses the DLL. Safe to call repeatedly; the thread runs until
    /// the app exits. Events arrive through `subscribe` (`BridgeEvent::ExplorerRestarted`).
    pub fn ensure_watching(self: &Arc<Self>) {
        if !self.watching.swap(true, Ordering::SeqCst) {
            let weak = Arc::downgrade(self);
            watch::spawn(move || {
                if let Some(b) = weak.upgrade() {
                    b.on_explorer_changed();
                }
            });
        }
    }

    fn check_hello(&self, h: &DllHello) -> Result<(), BridgeError> {
        if h.version != PROTO_VERSION { Err(BridgeError::VersionMismatch) } else { Ok(()) }
    }

    fn load_locked(&self) -> Result<(), BridgeError> {
        if let Some(h) = lock(&self.shared.st).hello.clone() {
            return self.check_hello(&h);
        }
        // The DLL is already resident in Explorer (from a previous run): wait for it to reconnect by itself, do not load a second copy.
        if inject::marker_exists() {
            tracing::info!(target: "sg::explorer", "DLL already present in explorer; waiting for it to reconnect");
            return match self.shared.wait_hello(RECONNECT_WAIT) {
                Some(h) => self.check_hello(&h),
                None => Err(BridgeError::Timeout),
            };
        }
        let target = inject::explorer_target().map_err(BridgeError::HookFailed)?;
        let path = embed::extract(&self.bin_dir, env!("CARGO_PKG_VERSION"), self.dll).map_err(|e| BridgeError::HookFailed(e.to_string()))?;
        if !embed::same_content(&path, self.dll) {
            return Err(BridgeError::HookFailed("DLL file on disk does not match the embedded copy".into()));
        }
        tracing::info!(target: "sg::explorer", "loading {} into explorer pid {} (desktop thread {})", path.display(), target.pid, target.tid);
        let hook = inject::inject(&path, &target).map_err(BridgeError::HookFailed)?;
        let hello = self.shared.wait_hello(CONNECT_TIMEOUT);
        drop(hook);
        match hello {
            Some(h) => self.check_hello(&h),
            None => Err(BridgeError::Timeout),
        }
    }

    /// Explorer restarted / desktop rebuilt: reload if some module needs it, then notify the modules.
    fn on_explorer_changed(self: &Arc<Self>) {
        let (want, connected) = {
            let st = lock(&self.shared.st);
            (st.desired.any(), st.hello.is_some())
        };
        tracing::info!(target: "sg::explorer", "explorer changed (wanted: {want}, DLL connected: {connected})");
        if want {
            if connected {
                // Same Explorer, new DefView: resend the state so the DLL finds DefView again and reapplies it.
                let msgs = lock(&self.shared.st).desired.replay();
                for m in msgs {
                    self.send(m);
                }
            } else {
                let _ = self.ensure_loaded();
            }
        }
        self.shared.emit(&BridgeEvent::ExplorerRestarted);
    }

    /// Only when the user clicks "Restart Explorer" (confirmed in the UI): every File Explorer window will close.
    pub fn restart_explorer(&self) -> Result<(), String> {
        inject::restart_explorer()
    }

    /// Manual test: simulates an Explorer restart (a `TaskbarCreated` message to our watcher window) without killing
    /// the real Explorer.
    pub fn debug_simulate_explorer_restart(&self) {
        watch::simulate_taskbar_created();
    }
}

/// `\\.\pipe\ShunguangDecor-explorer-<session id>` for the current Windows session.
pub fn default_pipe_name() -> String {
    sg_explorer_proto::pipe_name(server::session_id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sg_explorer_proto::{PaintMode, TextStyle};

    fn paint(c: u32) -> LabelsPaint {
        LabelsPaint { mode: PaintMode::Uniform, uniform: Some(TextStyle::plain(c)), items: vec![] }
    }

    #[test]
    fn desired_records_and_replays() {
        let mut d = Desired::default();
        assert!(!d.any());
        d.record(&ToDll::Labels(paint(1)));
        d.record(&ToDll::Ping);
        d.record(&ToDll::Labels(paint(2)));
        assert_eq!(d.replay(), vec![ToDll::Labels(paint(2))]);
        d.record(&ToDll::LabelsOff);
        assert!(!d.any());
        let m = connect_messages(&d, Some("x.log".into()));
        assert_eq!(m, vec![ToDll::Hello { version: PROTO_VERSION, log_path: Some("x.log".into()) }]);
    }

    #[test]
    fn send_while_disconnected_only_records() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::resolve(d.path(), true, None, None);
        let b = ExplorerBridge::with_pipe_name(b"", &paths, "x".into(), None, false);
        b.send(ToDll::Labels(paint(7)));
        assert!(lock(&b.shared.outgoing).is_empty());
        assert_eq!(lock(&b.shared.st).desired.labels, Some(paint(7)));
        assert_eq!(b.ensure_loaded(), Err(BridgeError::NoDll));
        let msgs = b.shared.on_client_connected();
        assert_eq!(msgs[1], ToDll::Labels(paint(7)), "nối lại → gửi lại trạng thái");
        b.send(ToDll::LabelsOff);
        assert_eq!(lock(&b.shared.outgoing).len(), 1, "đã nối thì xếp hàng gửi");
    }
}
