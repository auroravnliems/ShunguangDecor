//! Pure decisions (no Win32) for the bar's AppBar self-check and self-heal mechanism.
//! Win32 only supplies measurements (`rcMonitor`, `rcWork`) and then asks the functions here.

use std::collections::VecDeque;

/// Rectangle in screen coordinates (left, top, right, bottom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Area {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Area {
        Area { left, top, right, bottom }
    }
}

/// Does the monitor's work area leave out the bar's share?
/// `reserve` is the number of px the bar holds at the edge (bar height + margin). A work area that leaves out MORE is
/// still considered correct (another AppBar exists, e.g. the native taskbar in "keep" mode); only leaving out LESS needs healing.
pub fn needs_heal(monitor: Area, work: Area, top_edge: bool, reserve: i32) -> bool {
    if reserve <= 0 {
        return false;
    }
    if top_edge {
        work.top < monitor.top + reserve
    } else {
        work.bottom > monitor.bottom - reserve
    }
}

/// The number of px the bar actually holds at the edge, computed from the rectangle `ABM_SETPOS` returns (`rc`). In the native-taskbar "keep" mode,
/// the system places the bar OUTSIDE the native taskbar, so `rc` is `reserve` + the native taskbar height away from the screen edge: comparing against `rc` (not
/// against `monitor.bottom - reserve`) is how we tell the AppBar is still registered, since the native taskbar itself already reserves a strip.
pub fn effective_reserve(monitor: Area, rc: Area, top_edge: bool) -> i32 {
    if top_edge { rc.bottom - monitor.top } else { monitor.bottom - rc.top }
}

/// The y coordinate of the bar window once the system has placed the AppBar at `rc` (the `ABM_SETPOS` result): the bar sits fully inside `rc`,
/// exactly the `pad_edge` margin away from the outer edge of `rc`. In "keep" mode `rc` sits above the native taskbar, so the bar does too.
pub fn bar_y(top_edge: bool, rc: Area, pad_top: i32, pad_bottom: i32, bar_h: i32) -> i32 {
    if top_edge { rc.top + pad_top } else { rc.bottom - pad_bottom - bar_h }
}

/// Does the bar window `bar` overlap (area > 0) the rect of any native taskbar in `natives` (pass only native taskbars
/// that are visible and reserving space). A correctly placed bar sits above the native taskbar, with no overlap.
pub fn overlaps_any(bar: Area, natives: &[Area]) -> bool {
    natives.iter().any(|n| bar.left < n.right && n.left < bar.right && bar.top < n.bottom && n.top < bar.bottom)
}

/// Keeps only native taskbars on the SAME monitor and the SAME horizontal edge as the bar: vertical taskbars (Win10, ExplorerPatcher), secondary taskbars on
/// other monitors, or taskbars on the opposite edge never overlap the bar, so they are not counted, lest the bar be "healed" forever.
pub fn natives_on_same_edge(natives: &[Area], monitor: Area, top_edge: bool) -> Vec<Area> {
    use crate::model::Edge;
    use crate::reveal::{horizontal_edge, Rect};
    let rect = |a: Area| Rect { left: a.left, top: a.top, right: a.right, bottom: a.bottom };
    let want = if top_edge { Edge::Top } else { Edge::Bottom };
    natives
        .iter()
        .copied()
        // same monitor: the taskbar's center lies within the bar's monitor (an auto-hidden taskbar slid off-screen still has its center near the edge)
        .filter(|n| {
            let (cx, cy) = ((n.left + n.right) / 2, (n.top + n.bottom) / 2);
            cx >= monitor.left && cx < monitor.right && cy >= monitor.top && cy <= monitor.bottom
        })
        .filter(|n| horizontal_edge(rect(*n), rect(monitor)) == Some(want))
        .collect()
}

/// What to do with the bar window that was just built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    Show,
    Hide,
    Nothing,
}

/// A newly created window starts hidden, so it only needs showing when we want it visible; a REUSED window may really be visible on screen
/// (a freshly built `Layer` always starts with `visible = false`), so when we want it hidden we must actually hide it, otherwise the engine
/// believes it is hidden while the window is still there.
pub fn initial_window_action(want_visible: bool, reused_window: bool) -> WindowAction {
    match (want_visible, reused_window) {
        (true, _) => WindowAction::Show,
        (false, true) => WindowAction::Hide,
        (false, false) => WindowAction::Nothing,
    }
}

/// Guards against repeated healing: at most `max` times within a `window_ms` window.
pub struct HealLimiter {
    max: usize,
    window_ms: u64,
    times: VecDeque<u64>,
}

impl HealLimiter {
    pub fn new(max: usize, window_ms: u64) -> HealLimiter {
        HealLimiter { max, window_ms, times: VecDeque::new() }
    }

    /// Asks permission to heal at time `now_ms`; if allowed, records this attempt.
    pub fn allow(&mut self, now_ms: u64) -> bool {
        while let Some(&t) = self.times.front() {
            if now_ms.saturating_sub(t) >= self.window_ms {
                self.times.pop_front();
            } else {
                break;
            }
        }
        if self.times.len() >= self.max {
            return false;
        }
        self.times.push_back(now_ms);
        true
    }
}

/// Grace period after the engine's own AppBar operation: meanwhile the work area is still changing (REMOVE -> NEW ->
/// SETPOS temporarily makes rcWork fill the screen and Windows broadcasts SPI_SETWORKAREA), so it must not be concluded as lost yet.
pub struct HealGate {
    grace_ms: u64,
    quiet_until: u64,
}

impl HealGate {
    pub fn new(grace_ms: u64) -> HealGate {
        HealGate { grace_ms, quiet_until: 0 }
    }

    /// The engine just performed an AppBar operation itself (register, set position, heal) at `now_ms`.
    pub fn note_own_operation(&mut self, now_ms: u64) {
        self.quiet_until = now_ms.saturating_add(self.grace_ms);
    }

    /// May we check at `now_ms`? Not during the grace period (and it does not spend a `HealLimiter` attempt either).
    pub fn may_check(&self, now_ms: u64) -> bool {
        now_ms >= self.quiet_until
    }
}

/// The AppBar geometry key: if any component changes, it must be re-registered (not just ABM_SETPOS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppBarGeometry {
    pub monitor: Area,
    pub top_edge: bool,
    pub reserve: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Area = Area { left: 0, top: 0, right: 1920, bottom: 1080 };

    #[test]
    fn natives_filter_keeps_only_same_monitor_and_edge() {
        let bottom = Area::new(0, 1032, 1920, 1080);
        let vertical = Area::new(0, 0, 60, 1080);
        let top = Area::new(0, 0, 1920, 48);
        let other_monitor = Area::new(1920, 1032, 3840, 1080);
        let all = [bottom, vertical, top, other_monitor];
        assert_eq!(natives_on_same_edge(&all, MON, false), vec![bottom]);
        assert_eq!(natives_on_same_edge(&all, MON, true), vec![top]);
        assert!(natives_on_same_edge(&[vertical], MON, false).is_empty());
        assert!(natives_on_same_edge(&[other_monitor], MON, false).is_empty());
    }

    #[test]
    fn bottom_bar_ok_when_work_area_leaves_room() {
        assert!(!needs_heal(MON, Area::new(0, 0, 1920, 1032), false, 48));
        // reserving more (another AppBar exists) is still correct
        assert!(!needs_heal(MON, Area::new(0, 0, 1920, 1000), false, 48));
    }

    #[test]
    fn bottom_bar_needs_heal_when_work_area_is_full() {
        // F6: the work area covers the whole screen even though the bar is still there
        assert!(needs_heal(MON, Area::new(0, 0, 1920, 1080), false, 48));
    }

    #[test]
    fn bottom_bar_needs_heal_when_height_grew_but_work_area_did_not() {
        // F5: bar 36 → 48 but the work area is still 1044
        assert!(needs_heal(MON, Area::new(0, 0, 1920, 1044), false, 48));
        assert!(!needs_heal(MON, Area::new(0, 0, 1920, 1044), false, 36));
    }

    #[test]
    fn top_bar_checks_the_top_edge() {
        assert!(!needs_heal(MON, Area::new(0, 48, 1920, 1080), true, 48));
        assert!(needs_heal(MON, Area::new(0, 0, 1920, 1080), true, 48));
        assert!(needs_heal(MON, Area::new(0, 36, 1920, 1080), true, 48));
    }

    #[test]
    fn second_monitor_uses_its_own_coordinates() {
        let m2 = Area::new(1920, 0, 3840, 1080);
        assert!(!needs_heal(m2, Area::new(1920, 0, 3840, 1032), false, 48));
        assert!(needs_heal(m2, Area::new(1920, 0, 3840, 1080), false, 48));
    }

    #[test]
    fn zero_reserve_never_heals() {
        assert!(!needs_heal(MON, MON, false, 0));
    }

    #[test]
    fn bar_overlapping_a_reserving_native_taskbar_is_detected() {
        let native = Area::new(0, 1032, 1920, 1080);
        // measured on a real machine: the bar sits ON TOP of the native taskbar (same rect)
        assert!(overlaps_any(Area::new(0, 1032, 1920, 1080), &[native]));
        // partial overlap
        assert!(overlaps_any(Area::new(0, 1000, 1920, 1048), &[native]));
        // correct placement: right above the native taskbar (touching the edge only, no overlap)
        assert!(!overlaps_any(Area::new(0, 984, 1920, 1032), &[native]));
        // native taskbar on another monitor
        assert!(!overlaps_any(Area::new(0, 1032, 1920, 1080), &[Area::new(1920, 1032, 3840, 1080)]));
        assert!(!overlaps_any(Area::new(0, 1032, 1920, 1080), &[]));
    }

    #[test]
    fn reused_window_is_hidden_when_the_new_config_wants_it_hidden() {
        assert_eq!(initial_window_action(false, true), WindowAction::Hide);
        assert_eq!(initial_window_action(false, false), WindowAction::Nothing);
        assert_eq!(initial_window_action(true, true), WindowAction::Show);
        assert_eq!(initial_window_action(true, false), WindowAction::Show);
    }

    #[test]
    fn bar_y_follows_the_setpos_rect() {
        // native taskbar hidden/shown: rc is flush with the bottom edge → the bar sits flush with the edge as before
        assert_eq!(bar_y(false, Area::new(0, 1032, 1920, 1080), 0, 0, 48), 1032);
        assert_eq!(bar_y(false, Area::new(0, 1016, 1920, 1080), 8, 8, 48), 1024);
        // keep: the native taskbar, 48 tall, occupies 1032..1080 → the system places rc right above it, and the bar sits above it too
        assert_eq!(bar_y(false, Area::new(0, 984, 1920, 1032), 0, 0, 48), 984);
        assert_ne!(bar_y(false, Area::new(0, 984, 1920, 1032), 0, 0, 48), 1080 - 48, "không còn y = mon.bottom - h");
        // top edge: the native taskbar at the top, 48 tall → rc starts at 48
        assert_eq!(bar_y(true, Area::new(0, 48, 1920, 96), 0, 0, 48), 48);
        assert_eq!(bar_y(true, Area::new(0, 0, 1920, 64), 8, 8, 48), 8);
    }

    #[test]
    fn effective_reserve_counts_from_the_setpos_rect() {
        assert_eq!(effective_reserve(MON, Area::new(0, 1032, 1920, 1080), false), 48);
        assert_eq!(effective_reserve(MON, Area::new(0, 984, 1920, 1032), false), 96);
        assert_eq!(effective_reserve(MON, Area::new(0, 48, 1920, 96), true), 96);
        assert_eq!(effective_reserve(MON, Area::new(0, 0, 1920, 48), true), 48);
    }

    #[test]
    fn keep_mode_heals_when_only_the_native_taskbar_still_reserves() {
        // keep: rc = 984..1032. The native taskbar alone reserves up to 1032 → with the naive `reserve` (48) a missing AppBar goes unnoticed
        let rc = Area::new(0, 984, 1920, 1032);
        let work_only_native = Area::new(0, 0, 1920, 1032);
        assert!(!needs_heal(MON, work_only_native, false, 48), "cách so cũ không phát hiện");
        assert!(needs_heal(MON, work_only_native, false, effective_reserve(MON, rc, false)));
        // AppBar still registered: the work area reserves up to 984 → correct, no endless healing
        assert!(!needs_heal(MON, Area::new(0, 0, 1920, 984), false, effective_reserve(MON, rc, false)));
    }

    #[test]
    fn limiter_caps_heals_per_window_and_recovers() {
        let mut l = HealLimiter::new(3, 60_000);
        assert!(l.allow(0));
        assert!(l.allow(1_000));
        assert!(l.allow(2_000));
        assert!(!l.allow(3_000));
        assert!(!l.allow(59_999));
        // the first attempt has left the 60 s window
        assert!(l.allow(60_000));
        assert!(!l.allow(60_500));
    }

    #[test]
    fn gate_suppresses_checks_inside_grace_and_allows_after() {
        let mut g = HealGate::new(1_000);
        assert!(g.may_check(0));
        g.note_own_operation(5_000);
        assert!(!g.may_check(5_000));
        assert!(!g.may_check(5_999));
        assert!(g.may_check(6_000));
        // a new operation pushes the deadline further out
        g.note_own_operation(5_900);
        assert!(!g.may_check(6_000));
        assert!(g.may_check(6_900));
    }

    #[test]
    fn suppressed_checks_do_not_consume_the_limiter() {
        let mut g = HealGate::new(1_000);
        let mut l = HealLimiter::new(2, 60_000);
        g.note_own_operation(0);
        // 3 SPI_SETWORKAREA events within 6 ms: blocked by the gate before asking for a heal attempt
        for t in [10, 13, 16] {
            if g.may_check(t) {
                assert!(l.allow(t));
            }
        }
        assert!(l.allow(2_000));
        assert!(l.allow(2_001));
        assert!(!l.allow(2_002));
    }

    #[test]
    fn geometry_key_detects_each_change() {
        let a = AppBarGeometry { monitor: MON, top_edge: false, reserve: 36 };
        assert_eq!(a, a);
        assert_ne!(a, AppBarGeometry { reserve: 48, ..a });
        assert_ne!(a, AppBarGeometry { top_edge: true, ..a });
        assert_ne!(a, AppBarGeometry { monitor: Area::new(0, 0, 2560, 1440), ..a });
    }
}
