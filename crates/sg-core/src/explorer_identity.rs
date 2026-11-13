//! Explorer's "identity": used to tell whether Explorer has REALLY restarted.
//!
//! `TaskbarCreated` alone is not enough to conclude this: any app can broadcast it (the app's own taskbar
//! tray does exactly that every time it starts, YASB-style). Explorer is considered restarted only when its identity
//! changes: the desktop's DefView/pid or the real taskbar windows (`platform::native_taskbars`, with the app's own windows excluded).
//! Shared by sg-explorer (reloading the DLL) and sg-wallpaper (rebuilding the live wallpaper).

/// Explorer's identity at a point in time.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ExplorerIdentity {
    /// (the desktop's DefView, its owner pid); `None` if not measured yet.
    pub desktop: Option<(isize, u32)>,
    /// HWNDs of Explorer's real taskbars (sorted); empty if none yet.
    pub taskbars: Vec<isize>,
}

impl ExplorerIdentity {
    /// Measure again: the desktop part is supplied by the caller (each crate has its own way to find DefView); the taskbar part is measured here.
    pub fn measure(desktop: Option<(isize, u32)>) -> Self {
        let mut taskbars: Vec<isize> = crate::platform::native_taskbars().into_iter().map(|t| t.hwnd).collect();
        taskbars.sort_unstable();
        Self { desktop, taskbars }
    }

    /// Did Explorer restart between two measurements? A component that could not be measured in either one
    /// (no DefView yet, no taskbar yet) is not compared.
    pub fn changed_since(&self, now: &Self) -> bool {
        let desktop = matches!((self.desktop, now.desktop), (Some(a), Some(b)) if a != b);
        let bars = !self.taskbars.is_empty() && !now.taskbars.is_empty() && self.taskbars != now.taskbars;
        desktop || bars
    }

    /// Merges a new measurement into the old baseline, keeping the old component if this time it could not be measured.
    fn merged_into(self, now: Self) -> Self {
        Self {
            desktop: now.desktop.or(self.desktop),
            taskbars: if now.taskbars.is_empty() { self.taskbars } else { now.taskbars },
        }
    }
}

/// Holds the most recent identity baseline. The first measurement only sets the baseline (never reports "changed").
#[derive(Default)]
pub struct IdentityTracker {
    last: Option<ExplorerIdentity>,
}

impl IdentityTracker {
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// Sets the baseline (or merges into the old one) without reporting a change.
    pub fn seed(&mut self, now: ExplorerIdentity) {
        self.last = Some(match self.last.take() {
            Some(p) => p.merged_into(now),
            None => now,
        });
    }

    /// Updates the baseline; returns `true` if the identity changed compared with the previous baseline.
    pub fn observe(&mut self, now: ExplorerIdentity) -> bool {
        let changed = self.last.as_ref().is_some_and(|p| p.changed_since(&now));
        self.seed(now);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(dv: Option<(isize, u32)>, bars: &[isize]) -> ExplorerIdentity {
        ExplorerIdentity { desktop: dv, taskbars: bars.to_vec() }
    }

    #[test]
    fn broadcast_without_any_change_is_not_a_restart() {
        // The engine's tray broadcasts TaskbarCreated while Explorer stays exactly the same.
        let a = id(Some((100, 7)), &[200]);
        assert!(!a.changed_since(&a.clone()));
    }

    #[test]
    fn new_taskbar_window_or_desktop_is_a_restart() {
        let a = id(Some((100, 7)), &[200]);
        assert!(a.changed_since(&id(Some((100, 7)), &[300])));
        assert!(a.changed_since(&id(Some((101, 7)), &[200])));
        assert!(a.changed_since(&id(Some((100, 8)), &[200])));
    }

    #[test]
    fn extra_secondary_taskbar_counts_as_change() {
        let a = id(Some((100, 7)), &[200]);
        assert!(a.changed_since(&id(Some((100, 7)), &[200, 201])));
    }

    #[test]
    fn missing_parts_are_not_compared() {
        let a = id(Some((100, 7)), &[200]);
        assert!(!a.changed_since(&id(None, &[])));
        assert!(!id(None, &[]).changed_since(&a));
    }

    #[test]
    fn tracker_first_observation_is_never_a_change() {
        let mut t = IdentityTracker::default();
        assert!(!t.observe(id(Some((1, 2)), &[3])));
    }

    #[test]
    fn tracker_same_identity_does_not_fire_changed_identity_does() {
        let mut t = IdentityTracker::default();
        t.seed(id(Some((100, 7)), &[200]));
        assert!(!t.observe(id(Some((100, 7)), &[200])), "tin TaskbarCreated từ app khác: không dựng lại");
        assert!(t.observe(id(Some((100, 9)), &[300])), "Explorer thật khởi động lại: dựng lại");
        // Baseline updated: the next broadcast with the new identity does not report again.
        assert!(!t.observe(id(Some((100, 9)), &[300])));
    }

    #[test]
    fn tracker_keeps_old_parts_that_could_not_be_measured() {
        let mut t = IdentityTracker::default();
        t.seed(id(Some((100, 7)), &[200]));
        // A measurement lacking desktop + taskbar does not lose the old baseline.
        assert!(!t.observe(id(None, &[])));
        assert!(t.observe(id(Some((101, 7)), &[])));
        // The desktop baseline is now 101; the taskbar still keeps 200.
        assert!(!t.observe(id(Some((101, 7)), &[200])));
    }
}
