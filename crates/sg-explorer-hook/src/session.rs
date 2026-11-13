//! One session talking to the app over a pipe (pure logic; the real pipe reads/writes live in `pipe.rs`).
//!
//! The session knows nothing about Windows: it receives bytes, returns the messages to send, and commands
//! the desktop thread through [`DesktopPort`] (real: sets the paint table + posts messages to DefView; test: records them).

use crate::layout::LayoutTracker;
use crate::state::{DllState, Effect, Input};
use sg_explorer_proto::{FrameBuf, FromDll, LabelRect, LabelsPaint, PROTO_VERSION, PaintMode, ProtoError, ToDll};
use std::sync::Arc;

/// What the session needs from Explorer's desktop thread.
pub trait DesktopPort {
    /// New paint table (None = restore the Windows default). Must not block for long.
    fn set_paint(&mut self, paint: Option<Arc<LabelsPaint>>);
    /// Whether the listview reported a layout change since the last query.
    fn take_layout_dirty(&mut self) -> bool;
    /// Reads the text rectangle of every item (None = Explorer is busy / cannot be read).
    fn collect_layout(&mut self) -> Option<Vec<LabelRect>>;
    /// A severe error that occurred on the desktop thread since the last query.
    fn take_fault(&mut self) -> Option<String>;
    /// Switches the log file (the app sends it in Hello).
    fn set_log_path(&mut self, _path: Option<String>) {}
}

pub struct Session<D: DesktopPort> {
    pub state: DllState,
    frames: FrameBuf,
    layout: LayoutTracker,
    desk: D,
    pid: u32,
    build: String,
}

/// When idle, the pipe thread wakes up every this many ms (flush the log, check for errors).
pub const IDLE_WAKE_MS: u64 = 1000;

impl<D: DesktopPort> Session<D> {
    pub fn new(desk: D, pid: u32, build: String) -> Session<D> {
        Session { state: DllState::Idle, frames: FrameBuf::new(), layout: LayoutTracker::default(), desk, pid, build }
    }

    pub fn desk(&mut self) -> &mut D {
        &mut self.desk
    }

    fn apply(&mut self, input: Input) -> Vec<FromDll> {
        let state = std::mem::replace(&mut self.state, DllState::Idle);
        let (next, effects) = state.step(input);
        self.state = next;
        let mut out = Vec::new();
        for e in effects {
            match e {
                Effect::Paint(p) => {
                    let per_item = p.as_ref().is_some_and(|p| p.mode == PaintMode::PerItem);
                    self.desk.set_paint(p);
                    if per_item {
                        self.layout.force();
                    }
                }
                Effect::Reply(m) => out.push(m),
            }
        }
        out
    }

    /// Painting item by item → the app needs the layout updated periodically.
    fn polling(&self) -> bool {
        self.state.labels().is_some_and(|p| p.mode == PaintMode::PerItem)
    }

    /// Pipe just connected: send the hello first, clear the old buffer.
    pub fn on_connected(&mut self) -> Vec<FromDll> {
        self.frames = FrameBuf::new();
        self.layout.reset();
        vec![FromDll::Hello { version: PROTO_VERSION, pid: self.pid, build: self.build.clone() }]
    }

    /// New bytes from the app. A framing error (oversized, broken JSON) → the connection should be closed.
    pub fn on_bytes(&mut self, bytes: &[u8], now: u64) -> Result<Vec<FromDll>, ProtoError> {
        self.frames.push(bytes);
        let mut out = Vec::new();
        while let Some(msg) = self.frames.next_msg::<ToDll>()? {
            out.extend(self.on_message(msg, now));
        }
        Ok(out)
    }

    fn on_message(&mut self, msg: ToDll, now: u64) -> Vec<FromDll> {
        match msg {
            ToDll::Hello { version, log_path } => {
                self.desk.set_log_path(log_path);
                let out = self.apply(Input::AppHello { version });
                self.layout.force();
                let _ = now;
                out
            }
            ToDll::Labels(p) => self.apply(Input::Labels(p)),
            ToDll::LabelsOff => self.apply(Input::LabelsOff),
            ToDll::GetLayout => {
                self.layout.force();
                Vec::new()
            }
            ToDll::Ping => vec![FromDll::Pong],
        }
    }

    /// Periodic tick / when woken up: check the desktop thread for errors, send the layout if due.
    pub fn on_tick(&mut self, now: u64) -> Vec<FromDll> {
        let mut out = Vec::new();
        if let Some(f) = self.desk.take_fault() {
            out.extend(self.apply(Input::Fault(f)));
        }
        if self.desk.take_layout_dirty() {
            self.layout.mark_dirty(now);
        }
        if matches!(self.state, DllState::Active { .. }) && self.layout.due(now, self.polling()) {
            match self.desk.collect_layout() {
                Some(items) => {
                    if let Some(items) = self.layout.offer(now, items) {
                        out.push(FromDll::LabelLayout { items });
                    }
                }
                None => self.layout.retry_later(now),
            }
        }
        out
    }

    /// Pipe lost: restore the default immediately (required within < 1 s of the app dying).
    pub fn on_disconnected(&mut self) {
        let _ = self.apply(Input::Disconnected);
        self.frames = FrameBuf::new();
    }

    /// An error in the pipe thread itself (the panic has been caught).
    pub fn on_fault(&mut self, msg: String) {
        let _ = self.apply(Input::Fault(msg));
    }

    /// How long to wait at most before the next tick.
    pub fn wait_ms(&self, now: u64) -> u64 {
        if !matches!(self.state, DllState::Active { .. }) {
            return IDLE_WAKE_MS;
        }
        self.layout.wait_ms(now, self.polling()).unwrap_or(IDLE_WAKE_MS).min(IDLE_WAKE_MS)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use sg_explorer_proto::{ItemStyle, TextStyle, encode};

    /// Fake desktop thread: records every paint command (this is the "command queue" the tests check).
    #[derive(Default)]
    pub struct FakeDesk {
        pub paints: Vec<Option<Arc<LabelsPaint>>>,
        pub dirty: bool,
        pub layout: Vec<LabelRect>,
        pub fault: Option<String>,
        pub busy: bool,
        pub log_path: Option<String>,
    }

    impl DesktopPort for FakeDesk {
        fn set_paint(&mut self, paint: Option<Arc<LabelsPaint>>) {
            self.paints.push(paint);
        }
        fn take_layout_dirty(&mut self) -> bool {
            std::mem::take(&mut self.dirty)
        }
        fn collect_layout(&mut self) -> Option<Vec<LabelRect>> {
            (!self.busy).then(|| self.layout.clone())
        }
        fn take_fault(&mut self) -> Option<String> {
            self.fault.take()
        }
        fn set_log_path(&mut self, path: Option<String>) {
            self.log_path = path;
        }
    }

    fn uniform(color: u32) -> ToDll {
        ToDll::Labels(LabelsPaint { mode: PaintMode::Uniform, uniform: Some(TextStyle::plain(color)), items: vec![] })
    }

    fn feed(s: &mut Session<FakeDesk>, msgs: &[ToDll]) -> Vec<FromDll> {
        let mut bytes = Vec::new();
        for m in msgs {
            bytes.extend(encode(m));
        }
        s.on_bytes(&bytes, 0).unwrap()
    }

    /// A "fake pipe" closed midway: the desktop thread's command queue ends with the restore-default command.
    #[test]
    fn disconnect_returns_defaults() {
        let mut s = Session::new(FakeDesk::default(), 42, "test".into());
        let hello = s.on_connected();
        assert!(matches!(hello[..], [FromDll::Hello { version: PROTO_VERSION, pid: 42, .. }]));
        feed(&mut s, &[ToDll::Hello { version: PROTO_VERSION, log_path: Some("C:/l.log".into()) }, uniform(0xFF00FF)]);
        assert_eq!(s.desk().log_path.as_deref(), Some("C:/l.log"));
        assert_eq!(s.desk().paints.last().unwrap().as_ref().unwrap().uniform.unwrap().color, 0xFF00FF);

        // A frame is cut off midway and then the pipe breaks: half a frame must not be applied; the default must be restored.
        let half = encode(&uniform(0x00FF00));
        s.on_bytes(&half[..half.len() / 2], 0).unwrap();
        s.on_disconnected();
        assert_eq!(s.desk().paints.last(), Some(&None), "mất pipe → LabelsOff cho luồng desktop");
        assert_eq!(s.state, DllState::Idle);

        // Reconnect: must send hello again; the old half frame has been discarded.
        s.on_connected();
        let out = feed(&mut s, &[ToDll::Hello { version: PROTO_VERSION, log_path: None }, uniform(0x123456)]);
        assert!(out.iter().any(|m| matches!(m, FromDll::Status { labels_active: true, .. })));
        assert_eq!(s.desk().paints.last().unwrap().as_ref().unwrap().uniform.unwrap().color, 0x123456);
    }

    #[test]
    fn ping_and_layout() {
        let mut s = Session::new(FakeDesk::default(), 1, "t".into());
        s.on_connected();
        s.desk().layout = vec![LabelRect { index: 0, left: 1, top: 2, right: 3, bottom: 4 }];
        let out = feed(&mut s, &[ToDll::Ping]);
        assert_eq!(out, vec![FromDll::Pong]);
        assert!(s.on_tick(0).is_empty(), "chưa chào thì không gửi bố cục");
        feed(&mut s, &[ToDll::Hello { version: PROTO_VERSION, log_path: None }]);
        let out = s.on_tick(0);
        assert!(matches!(&out[..], [FromDll::LabelLayout { items }] if items.len() == 1));
        assert!(s.on_tick(10).is_empty(), "không đổi thì không gửi lại");
        feed(&mut s, &[ToDll::GetLayout]);
        assert_eq!(s.on_tick(20).len(), 1, "app xin thì gửi lại");
        // Painting item by item → the layout is read periodically.
        let per_item = ToDll::Labels(LabelsPaint {
            mode: PaintMode::PerItem,
            uniform: None,
            items: vec![ItemStyle { index: 0, style: TextStyle::plain(0) }],
        });
        feed(&mut s, &[per_item]);
        s.on_tick(30);
        s.desk().layout[0].left = 99;
        assert!(s.on_tick(100).is_empty());
        assert_eq!(s.wait_ms(100), IDLE_WAKE_MS.min(crate::layout::POLL_MS - 70));
        assert!(matches!(&s.on_tick(30 + crate::layout::POLL_MS)[..], [FromDll::LabelLayout { items }] if items[0].left == 99));
        // Explorer is busy → retry later, send nothing.
        s.desk().busy = true;
        s.desk().dirty = true;
        assert!(s.on_tick(5000).is_empty());
    }

    #[test]
    fn desktop_fault_rests_and_reports() {
        let mut s = Session::new(FakeDesk::default(), 1, "t".into());
        s.on_connected();
        feed(&mut s, &[ToDll::Hello { version: PROTO_VERSION, log_path: None }, uniform(1)]);
        s.desk().fault = Some("boom".into());
        let out = s.on_tick(0);
        assert!(out.iter().any(|m| matches!(m, FromDll::Status { error: Some(e), .. } if e.contains("boom"))));
        assert!(s.state.is_resting());
        assert_eq!(s.desk().paints.last(), Some(&None));
        let out = feed(&mut s, &[uniform(2)]);
        assert!(matches!(&out[..], [FromDll::Status { labels_active: false, .. }]));
        assert_eq!(s.desk().paints.last(), Some(&None), "đang nghỉ thì không tô nữa");
    }

    #[test]
    fn garbage_frame_is_error() {
        let mut s = Session::new(FakeDesk::default(), 1, "t".into());
        s.on_connected();
        let mut bad = 5u32.to_le_bytes().to_vec();
        bad.extend_from_slice(b"{{{{{");
        assert!(s.on_bytes(&bad, 0).is_err());
    }
}
