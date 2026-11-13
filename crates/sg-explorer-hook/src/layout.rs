//! When to resend the label layout (`LabelLayout`) to the app (pure logic).
//!
//! The listview reports a change (`LVN_ITEMCHANGED`/`LVN_INSERTITEM`/`LVN_DELETEITEM`/`WM_SIZE`…) → mark dirty,
//! debounce for [`DEBOUNCE_MS`], then re-read; additionally, while per-item coloring is active, re-read every
//! [`POLL_MS`] (dragging an icon elsewhere generates no notification). Only send when it differs from the last send.

use sg_explorer_proto::LabelRect;

pub const DEBOUNCE_MS: u64 = 200;
pub const POLL_MS: u64 = 2000;

#[derive(Debug, Default)]
pub struct LayoutTracker {
    last_sent: Option<Vec<LabelRect>>,
    dirty_at: Option<u64>,
    forced: bool,
    next_poll: u64,
}

pub fn layout_changed(old: Option<&[LabelRect]>, new: &[LabelRect]) -> bool {
    old != Some(new)
}

impl LayoutTracker {
    /// New connection: forget the previous send and resend immediately.
    pub fn reset(&mut self) {
        *self = LayoutTracker { forced: true, ..LayoutTracker::default() };
    }

    pub fn mark_dirty(&mut self, now: u64) {
        self.dirty_at = Some(now);
    }

    /// Send on the next read even if unchanged (the app requested `GetLayout`).
    pub fn force(&mut self) {
        self.forced = true;
    }

    pub fn due(&self, now: u64, polling: bool) -> bool {
        self.forced
            || self.dirty_at.is_some_and(|t| now >= t + DEBOUNCE_MS)
            || (polling && now >= self.next_poll)
    }

    /// How long until it is due (so the pipe thread knows how long to wait).
    pub fn wait_ms(&self, now: u64, polling: bool) -> Option<u64> {
        if self.forced {
            return Some(0);
        }
        let d = self.dirty_at.map(|t| (t + DEBOUNCE_MS).saturating_sub(now));
        let p = polling.then(|| self.next_poll.saturating_sub(now));
        match (d, p) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The layout was just read: returns `Some` if it needs to be sent.
    pub fn offer(&mut self, now: u64, items: Vec<LabelRect>) -> Option<Vec<LabelRect>> {
        let forced = std::mem::take(&mut self.forced);
        self.dirty_at = None;
        self.next_poll = now + POLL_MS;
        if forced || layout_changed(self.last_sent.as_deref(), &items) {
            self.last_sent = Some(items.clone());
            Some(items)
        } else {
            None
        }
    }

    /// Read failed (Explorer busy): retry after one debounce interval.
    pub fn retry_later(&mut self, now: u64) {
        self.dirty_at = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(index: u32, x: i32) -> LabelRect {
        LabelRect { index, left: x, top: 10, right: x + 70, bottom: 40 }
    }

    #[test]
    fn layout_diff_triggers_report() {
        let mut t = LayoutTracker::default();
        t.reset();
        assert!(t.due(0, false), "kết nối mới thì gửi ngay");
        assert_eq!(t.offer(0, vec![r(0, 0), r(1, 80)]).map(|v| v.len()), Some(2));
        // Unchanged → don't send, even when marked dirty.
        t.mark_dirty(100);
        assert!(!t.due(250, false), "chưa đủ 200 ms gom");
        t.mark_dirty(150);
        assert!(!t.due(300, false), "đánh dấu mới thì gom lại từ đầu");
        assert!(t.due(350, false));
        assert_eq!(t.offer(350, vec![r(0, 0), r(1, 80)]), None);
        // One item moved → send.
        t.mark_dirty(400);
        assert_eq!(t.offer(600, vec![r(0, 0), r(1, 160)]).unwrap()[1].left, 160);
        // Item added → send; item removed → send.
        assert!(t.offer(700, vec![r(0, 0), r(1, 160), r(2, 240)]).is_some());
        assert!(t.offer(800, vec![r(0, 0)]).is_some());
        // Resend requested → send even if unchanged.
        t.force();
        assert!(t.due(801, false));
        assert!(t.offer(801, vec![r(0, 0)]).is_some());
    }

    #[test]
    fn polling_schedule() {
        let mut t = LayoutTracker::default();
        t.reset();
        t.offer(1000, vec![]);
        assert!(!t.due(2000, true));
        assert_eq!(t.wait_ms(2000, true), Some(1000));
        assert!(t.due(3000, true));
        assert!(!t.due(3000, false), "không tô từng mục thì không đọc định kỳ");
        assert_eq!(t.wait_ms(3000, false), None);
        t.mark_dirty(3000);
        assert_eq!(t.wait_ms(3050, false), Some(150));
        assert_eq!(t.wait_ms(3050, true), Some(0), "lần đọc định kỳ đã quá hạn");
    }
}
