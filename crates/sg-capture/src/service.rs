//! `CaptureService`: a single thread that periodically captures the monitors that have viewers.
//! The latest frame (JPEG or PNG) lives in memory only; when nobody is watching, the thread exits and the frame is dropped.
//!
//! Each viewer registers a mode: `Full` (whole desktop, JPEG) or `Icons` (icon layer only, PNG with alpha, to be
//! overlaid on the wallpaper playing live in the layer below). The capture period is tracked per (monitor, mode).

use crate::{capture_desktop, capture_icons, shrink};
use image::RgbaImage;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hasher};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const JPEG_QUALITY: u8 = 80;
pub const MAX_WIDTH: u32 = 960;
/// Normal capture period (static image, icon layer).
pub const PERIOD: Duration = Duration::from_millis(500);
/// Fast capture period (~10 frames/s) for live wallpapers that cannot be played live (web, stream, application).
pub const FAST_PERIOD: Duration = Duration::from_millis(100);

/// What the viewer wants to receive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum ViewMode {
    /// The whole desktop (wallpaper + icons), JPEG.
    #[default]
    Full,
    /// Desktop icons only, PNG with an alpha channel.
    Icons,
}

impl ViewMode {
    /// Mode from the string sent by the UI; unknown -> `Full` (the old behavior).
    pub fn parse(s: &str) -> ViewMode {
        if s == "icons" { ViewMode::Icons } else { ViewMode::Full }
    }
}

/// Capture period for one viewer: `dynamic` = the wallpaper on that monitor is a capturable live type (web, stream,
/// application) and is actually running. The icon layer changes slowly, so it is always 500 ms.
pub fn period_for(mode: ViewMode, dynamic: bool) -> Duration {
    match mode {
        ViewMode::Full if dynamic => FAST_PERIOD,
        _ => PERIOD,
    }
}

/// Notifies the UI that a monitor has a new frame (`icons` = the icon layer's frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct CaptureFrame {
    pub monitor: u32,
    pub seq: u64,
    pub icons: bool,
}

type Capturer = dyn Fn(u32, ViewMode) -> Option<RgbaImage> + Send + Sync;
type Emit = dyn Fn(CaptureFrame) + Send + Sync;
type DynamicProbe = dyn Fn(u32) -> bool + Send + Sync;
type Key = (u32, ViewMode);

/// An encoded frame: sequence number, bytes, hash of the source image (to skip when the image is unchanged).
struct Stored {
    seq: u64,
    bytes: Arc<Vec<u8>>,
    hash: u64,
}

#[derive(Default)]
struct Inner {
    viewers: HashMap<u64, Key>,
    frames: HashMap<Key, Stored>,
    seq: HashMap<Key, u64>,
    /// The latest command sequence number received from each viewer (an older command arriving late is ignored).
    last_call: HashMap<u64, u64>,
    running: bool,
}

struct Shared {
    inner: Mutex<Inner>,
    wake: Condvar,
    capturer: Box<Capturer>,
    emit: Box<Emit>,
    /// `Some` = force every period to this value (tests); `None` = follow `period_for`.
    period_override: Option<Duration>,
    dynamic: Mutex<Option<Arc<DynamicProbe>>>,
}

pub struct CaptureService {
    shared: Arc<Shared>,
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn encode_jpeg(img: RgbaImage) -> Option<Vec<u8>> {
    let img = shrink(img, MAX_WIDTH);
    let (w, h) = img.dimensions();
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for p in img.pixels() {
        rgb.extend_from_slice(&p.0[..3]);
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode(&rgb, w, h, image::ExtendedColorType::Rgb8).ok()?;
    Some(out)
}

fn encode_png(img: RgbaImage) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    let img = shrink(img, MAX_WIDTH);
    let (w, h) = img.dimensions();
    let mut out = Vec::new();
    // Fast compression: the icon layer is mostly transparent so it stays small without costing CPU.
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Sub)
        .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgba8)
        .ok()?;
    Some(out)
}

fn hash_pixels(img: &RgbaImage) -> u64 {
    let mut h = DefaultHasher::new();
    h.write(img.as_raw());
    h.write_u32(img.width());
    h.finish()
}

impl CaptureService {
    /// Uses the real capture functions (`capture_desktop` / `capture_icons`).
    pub fn new(emit: impl Fn(CaptureFrame) + Send + Sync + 'static) -> Arc<CaptureService> {
        let real = |monitor: u32, mode: ViewMode| {
            let list = sg_desktop::monitors();
            let m = sg_desktop::find(&list, &monitor.to_string())?;
            match mode {
                ViewMode::Full => capture_desktop(m, MAX_WIDTH),
                ViewMode::Icons => capture_icons(m, MAX_WIDTH),
            }
        };
        Self::build(real, emit, None)
    }

    /// Fake capture function for tests; a short `period` for speed (applies to every mode).
    pub fn with_capturer(
        capturer: impl Fn(u32, ViewMode) -> Option<RgbaImage> + Send + Sync + 'static,
        emit: impl Fn(CaptureFrame) + Send + Sync + 'static,
        period: Duration,
    ) -> Arc<CaptureService> {
        Self::build(capturer, emit, Some(period))
    }

    fn build(
        capturer: impl Fn(u32, ViewMode) -> Option<RgbaImage> + Send + Sync + 'static,
        emit: impl Fn(CaptureFrame) + Send + Sync + 'static,
        period_override: Option<Duration>,
    ) -> Arc<CaptureService> {
        Arc::new(CaptureService {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner::default()),
                wake: Condvar::new(),
                capturer: Box::new(capturer),
                emit: Box::new(emit),
                period_override,
                dynamic: Mutex::new(None),
            }),
        })
    }

    /// Function telling whether the wallpaper on a monitor is a capturable live type (web/stream/application, running), so it is captured
    /// fast at 100 ms. Not set = treated as no.
    pub fn set_dynamic_probe(&self, f: impl Fn(u32) -> bool + Send + Sync + 'static) {
        *self.shared.dynamic.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
    }

    /// Viewer `viewer` starts watching `monitor` in `Full` mode (call again to switch monitors).
    pub fn start(&self, viewer: u64, monitor: u32) {
        self.start_seq(viewer, monitor, ViewMode::Full, u64::MAX);
    }

    /// Like `start` but with a mode and the UI's command sequence number: IPC commands sent back to back can arrive out
    /// of order (React runs an effect, cleans up, and re-runs within a few milliseconds), so a command older than one already received is ignored.
    pub fn start_seq(&self, viewer: u64, monitor: u32, mode: ViewMode, call: u64) {
        let (spawn, existing) = {
            let mut g = lock(&self.shared.inner);
            if !accept_call(&mut g, viewer, call) {
                return;
            }
            g.viewers.insert(viewer, (monitor, mode));
            // The viewer changed mode/monitor: drop the frame of any (monitor, mode) nobody is watching anymore.
            prune(&mut g);
            let spawn = !g.running;
            g.running = true;
            (spawn, g.frames.get(&(monitor, mode)).map(|f| f.seq))
        };
        // A frame already exists for this (monitor, mode) (kept by a previous viewer, or the page was just reloaded): notify right away so the new
        // viewer loads the frame. Otherwise, if the image is unchanged (skipped by hash), they would never receive any event.
        if let Some(seq) = existing {
            (self.shared.emit)(CaptureFrame { monitor, seq, icons: mode == ViewMode::Icons });
        }
        self.shared.wake.notify_all();
        if spawn {
            let shared = self.shared.clone();
            if let Err(e) = std::thread::Builder::new().name("sg-capture".into()).spawn(move || run(shared)) {
                tracing::error!(target: "sg::capture", "cannot start capture thread: {e}");
                lock(&self.shared.inner).running = false;
            }
        }
    }

    pub fn stop(&self, viewer: u64) {
        self.stop_seq(viewer, u64::MAX);
    }

    pub fn stop_seq(&self, viewer: u64, call: u64) {
        let mut g = lock(&self.shared.inner);
        if !accept_call(&mut g, viewer, call) {
            return;
        }
        g.viewers.remove(&viewer);
        prune(&mut g);
        drop(g);
        self.shared.wake.notify_all();
    }

    pub fn stop_all(&self) {
        let mut g = lock(&self.shared.inner);
        g.viewers.clear();
        g.last_call.clear();
        prune(&mut g);
        drop(g);
        self.shared.wake.notify_all();
    }

    /// The latest full frame of a monitor: (sequence number, JPEG).
    pub fn latest(&self, monitor: u32) -> Option<(u64, Arc<Vec<u8>>)> {
        self.latest_mode(monitor, ViewMode::Full)
    }

    /// The latest icon-layer frame of a monitor: (sequence number, PNG with alpha).
    pub fn latest_icons(&self, monitor: u32) -> Option<(u64, Arc<Vec<u8>>)> {
        self.latest_mode(monitor, ViewMode::Icons)
    }

    fn latest_mode(&self, monitor: u32, mode: ViewMode) -> Option<(u64, Arc<Vec<u8>>)> {
        lock(&self.shared.inner).frames.get(&(monitor, mode)).map(|s| (s.seq, s.bytes.clone()))
    }

    /// Whether the capture thread is running (for tests/diagnostics).
    pub fn is_running(&self) -> bool {
        lock(&self.shared.inner).running
    }
}

/// Accepts a command if its sequence number is newer than the one already received (`u64::MAX` = internal call, always accepted).
fn accept_call(g: &mut Inner, viewer: u64, call: u64) -> bool {
    if call == u64::MAX {
        return true;
    }
    if g.last_call.get(&viewer).is_some_and(|&last| call <= last) {
        return false;
    }
    // Keep the table small: drop viewers that have left when the table grows large.
    if g.last_call.len() > 1024 {
        let alive: HashSet<u64> = g.viewers.keys().copied().collect();
        g.last_call.retain(|v, _| alive.contains(v));
    }
    g.last_call.insert(viewer, call);
    true
}

/// Drops the frame of any (monitor, mode) nobody is watching anymore.
fn prune(g: &mut Inner) {
    let watched: HashSet<Key> = g.viewers.values().copied().collect();
    g.frames.retain(|k, _| watched.contains(k));
}

fn tasks_of(g: &Inner) -> Vec<Key> {
    let set: HashSet<Key> = g.viewers.values().copied().collect();
    let mut v: Vec<Key> = set.into_iter().collect();
    v.sort_unstable();
    v
}

fn run(shared: Arc<Shared>) {
    tracing::info!(target: "sg::capture", "capture thread started");
    let mut method_logged = false;
    // When each (monitor, mode) next needs capturing.
    let mut due: HashMap<Key, Instant> = HashMap::new();
    loop {
        let tasks: Vec<Key> = {
            let mut g = lock(&shared.inner);
            if g.viewers.is_empty() {
                g.running = false;
                g.frames.clear();
                tracing::info!(target: "sg::capture", "capture thread stopped (no viewers)");
                return;
            }
            tasks_of(&g)
        };
        due.retain(|k, _| tasks.contains(k));
        for &(m, mode) in &tasks {
            if due.get(&(m, mode)).is_some_and(|d| *d > Instant::now()) {
                continue;
            }
            capture_one(&shared, m, mode, &mut method_logged);
            let period = shared.period_override.unwrap_or_else(|| {
                let probe = shared.dynamic.lock().unwrap_or_else(|e| e.into_inner()).clone();
                period_for(mode, mode == ViewMode::Full && probe.is_some_and(|p| p(m)))
            });
            due.insert((m, mode), Instant::now() + period);
        }
        let g = lock(&shared.inner);
        // A new viewer joined/changed mode during capture: loop again immediately instead of sleeping a whole period.
        if g.viewers.is_empty() || tasks_of(&g) != tasks {
            continue;
        }
        let wait = tasks.iter().filter_map(|k| due.get(k)).min().map_or(PERIOD, |d| d.saturating_duration_since(Instant::now()));
        let _ = shared.wake.wait_timeout(g, wait).unwrap_or_else(|e| e.into_inner());
    }
}

/// Captures, encodes and stores a frame for (monitor, mode); notifies the UI if there is a new frame.
fn capture_one(shared: &Shared, m: u32, mode: ViewMode, method_logged: &mut bool) {
    let Some(img) = (shared.capturer)(m, mode) else { return };
    // The icon layer rarely changes: if the image is identical to the held frame, skip encoding and skip notifying the UI.
    let hash = (mode == ViewMode::Icons).then(|| hash_pixels(&img));
    if let Some(h) = hash
        && lock(&shared.inner).frames.get(&(m, mode)).is_some_and(|s| s.hash == h)
    {
        return;
    }
    let hash = hash.unwrap_or(0);
    let bytes = match mode {
        ViewMode::Full => encode_jpeg(img),
        ViewMode::Icons => encode_png(img),
    };
    let Some(bytes) = bytes else { return };
    let seq = {
        let mut g = lock(&shared.inner);
        // All viewers left during capture: drop the frame, do not keep it.
        if !g.viewers.values().any(|&v| v == (m, mode)) {
            return;
        }
        let s = g.seq.entry((m, mode)).or_insert(0);
        *s += 1;
        let s = *s;
        g.frames.insert((m, mode), Stored { seq: s, bytes: Arc::new(bytes), hash });
        s
    };
    if !*method_logged {
        // Record only the capture method's name (not the image content).
        *method_logged = true;
        tracing::info!(target: "sg::capture", "first frame for monitor {m} via {}", crate::last_method());
    }
    (shared.emit)(CaptureFrame { monitor: m, seq, icons: mode == ViewMode::Icons });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn fake() -> (Arc<CaptureService>, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let emits = Arc::new(AtomicUsize::new(0));
        let (c, e) = (calls.clone(), emits.clone());
        let svc = CaptureService::with_capturer(
            move |_, _| {
                c.fetch_add(1, Ordering::SeqCst);
                Some(RgbaImage::from_pixel(64, 36, image::Rgba([200, 80, 40, 255])))
            },
            move |_| {
                e.fetch_add(1, Ordering::SeqCst);
            },
            Duration::from_millis(20),
        );
        (svc, calls, emits)
    }

    fn wait_for(f: impl Fn() -> bool, ms: u64) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        f()
    }

    #[test]
    fn capture_emits_frames() {
        let (svc, _calls, emits) = fake();
        svc.start(1, 1);
        assert!(wait_for(|| emits.load(Ordering::SeqCst) >= 3, 2000));
        let (seq, jpeg) = svc.latest(1).unwrap();
        assert!(seq >= 3);
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8]);
        svc.stop_all();
    }

    #[test]
    fn capture_stops_when_last_viewer_leaves() {
        let (svc, _calls, _emits) = fake();
        svc.start(7, 1);
        assert!(wait_for(|| svc.latest(1).is_some(), 2000));
        svc.stop(7);
        assert!(svc.latest(1).is_none(), "khung bị xoá ngay khi hết người xem");
        assert!(wait_for(|| !svc.is_running(), 1500), "luồng phải thoát trong 1,5 s");
        assert!(svc.latest(1).is_none());
    }

    #[test]
    fn two_viewers_one_thread() {
        let (svc, _calls, _emits) = fake();
        svc.start(1, 1);
        svc.start(2, 1);
        svc.start(3, 2);
        assert!(wait_for(|| svc.latest(1).is_some() && svc.latest(2).is_some(), 2000));
        svc.stop(1);
        assert!(svc.is_running(), "còn người xem thì luồng còn chạy");
        assert!(svc.latest(1).is_some());
        svc.stop(2);
        assert!(wait_for(|| svc.latest(1).is_none(), 500), "màn hình 1 hết người xem thì bỏ khung");
        assert!(svc.latest(2).is_some() || wait_for(|| svc.latest(2).is_some(), 1000));
        svc.stop(3);
        assert!(wait_for(|| !svc.is_running(), 1500));
    }

    #[test]
    fn stale_ipc_calls_are_ignored() {
        let (svc, _calls, _emits) = fake();
        // The UI sends start(1), stop(2), start(3) but the core receives them out of order: start(1), start(3), stop(2).
        svc.start_seq(9, 1, ViewMode::Full, 1);
        svc.start_seq(9, 1, ViewMode::Full, 3);
        svc.stop_seq(9, 2);
        assert!(wait_for(|| svc.latest(1).is_some(), 2000), "stop(2) tới muộn phải bị bỏ qua, người xem còn nguyên");
        assert!(svc.is_running());
        // A newer stop takes effect; an old start arriving late cannot revive it.
        svc.stop_seq(9, 4);
        svc.start_seq(9, 1, ViewMode::Full, 3);
        assert!(svc.latest(1).is_none());
        assert!(wait_for(|| !svc.is_running(), 1500));
    }

    #[test]
    fn stop_all_clears() {
        let (svc, _calls, _emits) = fake();
        svc.start(1, 1);
        svc.start(2, 2);
        assert!(wait_for(|| svc.latest(1).is_some() && svc.latest(2).is_some(), 2000));
        svc.stop_all();
        assert!(svc.latest(1).is_none() && svc.latest(2).is_none());
        assert!(wait_for(|| !svc.is_running(), 1500));
        // Can start again.
        svc.start(1, 1);
        assert!(wait_for(|| svc.latest(1).is_some(), 2000));
        svc.stop_all();
    }

    #[test]
    fn period_for_viewer_kinds() {
        assert_eq!(period_for(ViewMode::Icons, false), Duration::from_millis(500));
        assert_eq!(period_for(ViewMode::Icons, true), Duration::from_millis(500), "lớp icon luôn chậm");
        assert_eq!(period_for(ViewMode::Full, false), Duration::from_millis(500));
        assert_eq!(period_for(ViewMode::Full, true), Duration::from_millis(100));
        assert_eq!(ViewMode::parse("icons"), ViewMode::Icons);
        assert_eq!(ViewMode::parse("full"), ViewMode::Full);
        assert_eq!(ViewMode::parse("zzz"), ViewMode::Full);
    }

    #[test]
    fn icons_viewer_gets_png_and_skips_unchanged() {
        let emits = Arc::new(AtomicUsize::new(0));
        let e = emits.clone();
        let svc = CaptureService::with_capturer(
            |_, mode| {
                assert_eq!(mode, ViewMode::Icons);
                Some(RgbaImage::from_pixel(32, 18, image::Rgba([10, 20, 30, 0])))
            },
            move |f| {
                assert!(f.icons);
                e.fetch_add(1, Ordering::SeqCst);
            },
            Duration::from_millis(10),
        );
        svc.start_seq(1, 1, ViewMode::Icons, 1);
        assert!(wait_for(|| svc.latest_icons(1).is_some(), 2000));
        let (seq, png) = svc.latest_icons(1).unwrap();
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
        assert!(svc.latest(1).is_none(), "không có khung Full khi chỉ có người xem icon");
        // Captured many times but the image is identical -> notify only once, seq stays the same.
        std::thread::sleep(Duration::from_millis(120));
        assert_eq!(emits.load(Ordering::SeqCst), 1);
        assert_eq!(svc.latest_icons(1).unwrap().0, seq);
        svc.stop_all();
    }

    #[test]
    fn new_viewer_gets_existing_frame_event() {
        let emits = Arc::new(AtomicUsize::new(0));
        let last = Arc::new(Mutex::new(None));
        let (e, l) = (emits.clone(), last.clone());
        let svc = CaptureService::with_capturer(
            |_, _| Some(RgbaImage::from_pixel(32, 18, image::Rgba([10, 20, 30, 255]))),
            move |f| {
                e.fetch_add(1, Ordering::SeqCst);
                *l.lock().unwrap() = Some(f);
            },
            Duration::from_millis(10),
        );
        svc.start_seq(1, 1, ViewMode::Icons, 1);
        assert!(wait_for(|| svc.latest_icons(1).is_some(), 2000));
        std::thread::sleep(Duration::from_millis(60)); // image unchanged: no more events
        let (seq, _) = svc.latest_icons(1).unwrap();
        let before = emits.load(Ordering::SeqCst);
        // A second viewer (page reload) for the same (monitor, mode): must receive the event for the existing frame.
        svc.start_seq(2, 1, ViewMode::Icons, 1);
        assert_eq!(emits.load(Ordering::SeqCst), before + 1);
        let f = last.lock().unwrap().unwrap();
        assert!(f.icons && f.monitor == 1 && f.seq >= seq);
        svc.stop_all();
    }

    #[test]
    fn modes_are_independent_and_pruned() {
        let (svc, _c, _e) = fake();
        svc.start_seq(1, 1, ViewMode::Full, 1);
        svc.start_seq(2, 1, ViewMode::Icons, 1);
        assert!(wait_for(|| svc.latest(1).is_some() && svc.latest_icons(1).is_some(), 2000));
        svc.stop_seq(2, 2);
        assert!(svc.latest_icons(1).is_none(), "hết người xem icon thì bỏ khung icon");
        assert!(svc.latest(1).is_some());
        svc.stop_all();
    }
}
