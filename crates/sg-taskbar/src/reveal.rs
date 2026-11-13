//! Pure part of the native taskbar mode: the `reveal` state machine, shell flyout detection, hot corners.
//! It makes no Win32 calls, so it runs and can be tested on any operating system.

use crate::model::{Corner, Edge, NativeMode};

/// How long after the mouse leaves the native taskbar (with no flyout open) before it hides again.
pub const HIDE_DELAY_MS: u64 = 300;
/// How long the mouse must stay in a corner to trigger the hot corner.
pub const HOT_CORNER_DWELL_MS: u64 = 150;
/// Thickness of the "touch the edge" strip, in pixels.
pub const EDGE_PX: i32 = 1;
/// Side length of the "touch the corner" zone, in pixels.
pub const CORNER_PX: i32 = 2;

/// A screen rectangle: `right`/`bottom` are the outer edges (not part of the rect), like Windows' `RECT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
    fn w(&self) -> i32 {
        self.right - self.left
    }
    fn h(&self) -> i32 {
        self.bottom - self.top
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevealState {
    /// Is the native taskbar visible?
    pub visible: bool,
    /// Since when the mouse has been away with no flyout open (to count up to `HIDE_DELAY_MS`).
    pub away_since: Option<u64>,
}

impl RevealState {
    /// Initial state: hidden.
    pub const HIDDEN: RevealState = RevealState { visible: false, away_since: None };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevealInput {
    pub cursor_on_edge: bool,
    pub cursor_over_native: bool,
    pub shell_flyout_open: bool,
    pub now_ms: u64,
}

/// Shows immediately when the mouse touches the edge; hides when the mouse has been off the native taskbar AND no flyout has been open for ≥ 300 ms continuously.
pub fn reveal_step(state: RevealState, input: RevealInput) -> RevealState {
    if input.cursor_on_edge {
        return RevealState { visible: true, away_since: None };
    }
    if !state.visible {
        return RevealState::HIDDEN;
    }
    if input.cursor_over_native || input.shell_flyout_open {
        return RevealState { visible: true, away_since: None };
    }
    let since = state.away_since.unwrap_or(input.now_ms);
    if input.now_ms.saturating_sub(since) >= HIDE_DELAY_MS {
        RevealState::HIDDEN
    } else {
        RevealState { visible: true, away_since: Some(since) }
    }
}

/// Is the mouse touching the screen edge where the native taskbar sits? `taskbar` is the taskbar's rectangle (even while hidden),
/// `monitor` is the screen containing it. The edge is inferred from the taskbar's position relative to the screen.
pub fn cursor_on_edge(cursor: (i32, i32), taskbar: Rect, monitor: Rect) -> bool {
    let (x, y) = cursor;
    if taskbar.w() >= taskbar.h() {
        let in_span = x >= taskbar.left && x < taskbar.right;
        if taskbar.top + taskbar.bottom > monitor.top + monitor.bottom {
            in_span && y >= monitor.bottom - EDGE_PX && y < monitor.bottom
        } else {
            in_span && y >= monitor.top && y < monitor.top + EDGE_PX
        }
    } else {
        let in_span = y >= taskbar.top && y < taskbar.bottom;
        if taskbar.left + taskbar.right > monitor.left + monitor.right {
            in_span && x >= monitor.right - EDGE_PX && x < monitor.right
        } else {
            in_span && x >= monitor.left && x < monitor.left + EDGE_PX
        }
    }
}

/// Window classes of the flyouts/menus built by the shell.
const FLYOUT_CLASSES: &[&str] = &[
    "Windows.UI.Core.CoreWindow",
    "NotifyIconOverflowWindow",
    "Xaml_WindowedPopupClass",
    "#32768",
    "TopLevelWindowForOverflowXamlIsland",
    "XamlExplorerHostIslandWindow",
    "Shell_InputSwitchTopLevelWindow",
    // thumbnail / jump-list when hovering a native taskbar button (does not take the foreground)
    "TaskListThumbnailWnd",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
];

/// Windows shell processes (compared case-insensitively).
const SHELL_EXES: &[&str] =
    &["explorer.exe", "startmenuexperiencehost.exe", "searchhost.exe", "searchapp.exe", "shellexperiencehost.exe", "shellhost.exe"];

/// Is the top-level window under the cursor a flyout of the native taskbar: like `is_shell_flyout` but it does NOT count the taskbar windows
/// (`Shell_TrayWnd`/`Shell_SecondaryTrayWnd`) — when the mouse moves from screen B's taskbar to screen A's edge, the window under the cursor
/// is A's taskbar, and that must not keep B's taskbar shown.
pub fn is_flyout_under_cursor(class: &str, exe: &str) -> bool {
    class != "Shell_TrayWnd" && class != "Shell_SecondaryTrayWnd" && is_shell_flyout(class, exe)
}

/// Considers only the window class (no exe name): used when the process is unknown, for example in the "fullscreen" check.
pub fn is_shell_flyout_class(class: &str) -> bool {
    FLYOUT_CLASSES.contains(&class)
}

/// Is the window a shell flyout/menu (Start, Search, hidden tray, calendar, Action Center, context menu, button thumbnail)?
/// Used for both the foreground window and the top-level window under the cursor. It needs both the class and the exe name: a `#32768` menu
/// of another app does not count.
pub fn is_shell_flyout(class: &str, exe: &str) -> bool {
    let exe = exe.rsplit(['\\', '/']).next().unwrap_or(exe);
    FLYOUT_CLASSES.contains(&class) && SHELL_EXES.iter().any(|e| exe.eq_ignore_ascii_case(e))
}

// ---------------------------------------------------------------------------------------------------------------
// The app's bar: "hide when fullscreen" and "auto-hide". Pure decision logic; the engine only takes Win32 measurements and asks here.

/// Thickness of the auto-hide bar's "touch the edge" strip, in pixels.
pub const AUTO_HIDE_EDGE_PX: i32 = 2;

/// Measurements of the foreground window that the engine can read (read-only, never touching the owning process).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundWindow {
    pub rect: Rect,
    /// The monitor containing most of the window (`MONITOR_DEFAULTTONEAREST`).
    pub monitor: Rect,
    pub class: String,
    /// Has `WS_CAPTION`? A normally maximized window has a title bar; a fullscreen game/browser does not.
    pub has_caption: bool,
    /// Is a window of the bar engine itself (bar, popup, tooltip): never counted as fullscreen.
    pub own: bool,
    /// Currently maximized (`IsZoomed`): maximized Chrome/VS Code/Discord is very common and is never fullscreen.
    pub zoomed: bool,
}

/// Window classes of the desktop: covering the whole screen is not a "fullscreen game" either. The shell UI (native taskbar, Start,
/// Search, Task View/Alt+Tab, hidden tray...) shares the `FLYOUT_CLASSES` list; no second list is kept.
const DESKTOP_CLASSES: &[&str] = &["Progman", "WorkerW"];

/// Whether rect `inner` fully covers `outer` (⊇).
fn covers(inner: Rect, outer: Rect) -> bool {
    inner.left <= outer.left && inner.top <= outer.top && inner.right >= outer.right && inner.bottom >= outer.bottom
}

fn excluded(w: &ForegroundWindow) -> bool {
    w.own || DESKTOP_CLASSES.contains(&w.class.as_str()) || is_shell_flyout_class(&w.class)
}

/// The `SHQueryUserNotificationState` state (trimmed down): only the values relevant to fullscreen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyState {
    /// No signal (normal, or unreadable).
    None,
    /// `QUNS_BUSY`.
    Busy,
    /// `QUNS_RUNNING_D3D_FULL_SCREEN`.
    D3dFullscreen,
    /// `QUNS_PRESENTATION_MODE`.
    Presentation,
}

/// Whether it is worth asking `SHQueryUserNotificationState`: only when the window could be genuinely fullscreen (not excluded, covers
/// the screen). A maximized window is STILL asked (some fullscreen games/browsers still carry WS_MAXIMIZE); preventing a maximized window
/// from being mistaken for fullscreen happens in `is_fullscreen_foreground`. This avoids a system call every 500 ms tick for every ordinary window.
pub fn should_query_quns(fg: Option<&ForegroundWindow>) -> bool {
    fg.is_some_and(|w| !excluded(w) && covers(w.rect, w.monitor))
}

/// Is the foreground window fullscreen on its monitor? The desktop, the native taskbar and the bar's own windows never
/// count. The `quns` signal (`SHQueryUserNotificationState`) catches genuine fullscreen:
/// - ordinary window: `Busy` / `D3dFullscreen` / `Presentation` all count, plus the rect comparison (covers the screen, no
///   title bar);
/// - maximized window (`zoomed`): the rect comparison does NOT count (when the bar reserves no space, it covers the screen plus an 8 px border), and
///   `Busy` is ignored (with an auto-hiding native taskbar, maximized Chrome/VS Code covers the screen and the shell may report busy); only
///   `D3dFullscreen` / `Presentation` count.
pub fn is_fullscreen_foreground(fg: Option<&ForegroundWindow>, quns: NotifyState) -> bool {
    let Some(w) = fg else { return false };
    if excluded(w) {
        return false;
    }
    if w.zoomed {
        return matches!(quns, NotifyState::D3dFullscreen | NotifyState::Presentation);
    }
    quns != NotifyState::None || (!w.has_caption && covers(w.rect, w.monitor))
}

/// The "over the bar" zone: the bar's rectangle when shown, extended to the screen edge (a floating bar has margins: the mouse in
/// the gap between the bar and the edge still counts as being on the bar).
pub fn auto_hide_zone(bar: Rect, monitor: Rect, top_edge: bool) -> Rect {
    if top_edge {
        Rect { top: monitor.top.min(bar.top), ..bar }
    } else {
        Rect { bottom: monitor.bottom.max(bar.bottom), ..bar }
    }
}

/// The mouse touches the screen edge the bar is on (≤ `AUTO_HIDE_EDGE_PX`), within the bar's horizontal span.
pub fn cursor_on_bar_edge(cursor: (i32, i32), bar: Rect, monitor: Rect, top_edge: bool) -> bool {
    let (x, y) = cursor;
    let in_span = x >= bar.left && x < bar.right;
    if top_edge {
        in_span && y >= monitor.top && y < monitor.top + AUTO_HIDE_EDGE_PX
    } else {
        in_span && y >= monitor.bottom - AUTO_HIDE_EDGE_PX && y < monitor.bottom
    }
}

/// The edge (top/bottom) a horizontal taskbar is attached to on screen `monitor`; `None` for a vertical taskbar.
pub fn horizontal_edge(taskbar: Rect, monitor: Rect) -> Option<Edge> {
    if taskbar.w() < taskbar.h() {
        return None;
    }
    // compare centers the way `cursor_on_edge` does: an auto-hiding taskbar slid almost off screen still lands on the right side
    Some(if taskbar.top + taskbar.bottom > monitor.top + monitor.bottom { Edge::Bottom } else { Edge::Top })
}

/// The bar's "auto-hide" is skipped when the native taskbar is in `reveal` mode and a native taskbar sits on the same edge as the bar: both would
/// fight over that edge (a hidden-mode native taskbar holds the edge's auto-hide slot, see `native.rs`).
pub fn auto_hide_conflicts(native: NativeMode, native_edges: &[Edge], bar: Edge) -> bool {
    native == NativeMode::Reveal && native_edges.contains(&bar)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoHideInput {
    pub cursor_on_edge: bool,
    pub cursor_over_bar: bool,
    /// A popup / menu / thumbnail of the bar is open.
    pub popup_open: bool,
    pub now_ms: u64,
}

/// The "auto-hide" state machine (same rules as `reveal_step`): shows immediately when the mouse touches the edge; hides again when the mouse has left the bar zone
/// for ≥ 300 ms continuously with no popup/menu/thumbnail of the bar open. `visible` = the bar is shown.
pub fn auto_hide_step(state: RevealState, input: AutoHideInput) -> RevealState {
    reveal_step(
        state,
        RevealInput {
            cursor_on_edge: input.cursor_on_edge,
            cursor_over_native: input.cursor_over_bar,
            shell_flyout_open: input.popup_open,
            now_ms: input.now_ms,
        },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HotCornerState {
    /// When the mouse started being in the corner.
    pub entered_at: Option<u64>,
    /// Already fired during this stay in the corner (fires again only after the mouse leaves the corner).
    pub fired: bool,
}

/// Returns the new state and `true` exactly once when the mouse has been in the corner for ≥ 150 ms.
pub fn hot_corner_step(state: HotCornerState, in_corner: bool, now_ms: u64) -> (HotCornerState, bool) {
    if !in_corner {
        return (HotCornerState::default(), false);
    }
    let entered = state.entered_at.unwrap_or(now_ms);
    if !state.fired && now_ms.saturating_sub(entered) >= HOT_CORNER_DWELL_MS {
        return (HotCornerState { entered_at: Some(entered), fired: true }, true);
    }
    (HotCornerState { entered_at: Some(entered), fired: state.fired }, false)
}

/// Whether the mouse is inside the `corner` zone of screen `monitor`.
pub fn cursor_in_corner(cursor: (i32, i32), monitor: Rect, corner: Corner) -> bool {
    let (x, y) = cursor;
    let in_x = match corner {
        Corner::TopLeft | Corner::BottomLeft => x >= monitor.left && x < monitor.left + CORNER_PX,
        Corner::TopRight | Corner::BottomRight => x >= monitor.right - CORNER_PX && x < monitor.right,
    };
    let in_y = match corner {
        Corner::TopLeft | Corner::TopRight => y >= monitor.top && y < monitor.top + CORNER_PX,
        Corner::BottomLeft | Corner::BottomRight => y >= monitor.bottom - CORNER_PX && y < monitor.bottom,
    };
    in_x && in_y
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(on_edge: bool, over: bool, flyout: bool, now: u64) -> RevealInput {
        RevealInput { cursor_on_edge: on_edge, cursor_over_native: over, shell_flyout_open: flyout, now_ms: now }
    }

    #[test]
    fn reveal_state_machine() {
        let mut s = RevealState::HIDDEN;
        // mouse in the middle of the screen: still hidden
        s = reveal_step(s, inp(false, false, false, 0));
        assert!(!s.visible);
        // touch the edge: shown immediately
        s = reveal_step(s, inp(true, true, false, 10));
        assert!(s.visible);
        // mouse goes onto the taskbar, then opens Start (flyout), then leaves: still shown even after 300 ms
        s = reveal_step(s, inp(false, true, false, 40));
        s = reveal_step(s, inp(false, false, true, 70));
        s = reveal_step(s, inp(false, false, true, 1000));
        assert!(s.visible);
        // close the flyout: counting starts, still shown until 300 ms have passed
        s = reveal_step(s, inp(false, false, false, 1100));
        s = reveal_step(s, inp(false, false, false, 1399));
        assert!(s.visible);
        // after 300 ms it hides
        s = reveal_step(s, inp(false, false, false, 1400));
        assert!(!s.visible);
    }

    #[test]
    fn returning_to_taskbar_resets_the_hide_timer() {
        let mut s = reveal_step(RevealState::HIDDEN, inp(true, true, false, 0));
        s = reveal_step(s, inp(false, false, false, 10));
        s = reveal_step(s, inp(false, true, false, 200)); // came back
        s = reveal_step(s, inp(false, false, false, 210));
        s = reveal_step(s, inp(false, false, false, 400)); // only 190 ms since the later departure
        assert!(s.visible);
        s = reveal_step(s, inp(false, false, false, 510));
        assert!(!s.visible);
    }

    #[test]
    fn edge_detection_follows_taskbar_side() {
        let mon = Rect { left: 0, top: 0, right: 1920, bottom: 1080 };
        let bottom = Rect { left: 0, top: 1040, right: 1920, bottom: 1080 };
        assert!(cursor_on_edge((500, 1079), bottom, mon));
        assert!(!cursor_on_edge((500, 1078), bottom, mon));
        assert!(!cursor_on_edge((500, 0), bottom, mon));
        let left = Rect { left: 0, top: 0, right: 48, bottom: 1080 };
        assert!(cursor_on_edge((0, 300), left, mon));
        assert!(!cursor_on_edge((1919, 300), left, mon));
        let top = Rect { left: 0, top: 0, right: 1920, bottom: 40 };
        assert!(cursor_on_edge((10, 0), top, mon));
        let right = Rect { left: 1872, top: 0, right: 1920, bottom: 1080 };
        assert!(cursor_on_edge((1919, 5), right, mon));
    }

    #[test]
    fn flyout_classifier() {
        assert!(is_shell_flyout("Windows.UI.Core.CoreWindow", "C:\\Windows\\SystemApps\\X\\StartMenuExperienceHost.exe"));
        assert!(is_shell_flyout("Windows.UI.Core.CoreWindow", "SearchHost.exe"));
        assert!(is_shell_flyout("NotifyIconOverflowWindow", "explorer.exe"));
        assert!(is_shell_flyout("#32768", "C:\\Windows\\EXPLORER.EXE"));
        assert!(is_shell_flyout("Shell_TrayWnd", "explorer.exe"));
        assert!(is_shell_flyout("TopLevelWindowForOverflowXamlIsland", "explorer.exe"));
        assert!(is_shell_flyout("Shell_InputSwitchTopLevelWindow", "explorer.exe"));
        // thumbnail when hovering a taskbar button (the top-level window under the cursor, not the foreground)
        assert!(is_shell_flyout("TaskListThumbnailWnd", "C:\\Windows\\explorer.exe"));
        assert!(is_shell_flyout("Xaml_WindowedPopupClass", "explorer.exe"));
        assert!(!is_shell_flyout("TaskListThumbnailWnd", "notepad.exe"));
        assert!(!is_shell_flyout("Progman", "explorer.exe"));
        assert!(!is_shell_flyout("WorkerW", "explorer.exe"));
        // a menu of another app, or an ordinary explorer window (a folder window), does not count
        assert!(!is_shell_flyout("#32768", "notepad.exe"));
        assert!(!is_shell_flyout("CabinetWClass", "explorer.exe"));
        assert!(!is_shell_flyout("Shell_TrayWnd", "ShunguangDecor.exe"));
    }

    const SCREEN: Rect = Rect { left: 0, top: 0, right: 1920, bottom: 1080 };

    fn fg(rect: Rect, class: &str, has_caption: bool, own: bool) -> ForegroundWindow {
        ForegroundWindow { rect, monitor: SCREEN, class: class.into(), has_caption, own, zoomed: false }
    }

    #[test]
    fn maximized_window_is_fullscreen_only_for_d3d_or_presentation() {
        // a borderless maximized window (Discord/VS Code) covering the screen
        let zoomed = ForegroundWindow { zoomed: true, ..fg(SCREEN, "Chrome_WidgetWin_1", false, false) };
        // no signal: not fullscreen
        assert!(!is_fullscreen_foreground(Some(&zoomed), NotifyState::None));
        // the shell reports busy (auto-hiding native taskbar): ignored for a maximized window
        assert!(!is_fullscreen_foreground(Some(&zoomed), NotifyState::Busy));
        // D3D fullscreen / presentation still count (a genuine fullscreen window may still carry WS_MAXIMIZE)
        assert!(is_fullscreen_foreground(Some(&zoomed), NotifyState::D3dFullscreen));
        assert!(is_fullscreen_foreground(Some(&zoomed), NotifyState::Presentation));
        // a maximized window covering the screen is still asked via QUNS
        assert!(should_query_quns(Some(&zoomed)));
        // ordinary window: Busy still counts as before
        let normal = fg(SCREEN, "Chrome_WidgetWin_1", true, false);
        assert!(is_fullscreen_foreground(Some(&normal), NotifyState::Busy));
        assert!(!is_fullscreen_foreground(Some(&normal), NotifyState::None));
        // the same window but not maximized: fullscreen
        assert!(is_fullscreen_foreground(Some(&fg(SCREEN, "Chrome_WidgetWin_1", false, false)), NotifyState::None));
    }

    #[test]
    fn quns_is_only_queried_for_plausible_fullscreen_windows() {
        assert!(should_query_quns(Some(&fg(SCREEN, "GameClass", false, false))));
        let small = Rect { left: 100, top: 100, right: 800, bottom: 600 };
        assert!(!should_query_quns(Some(&fg(small, "Notepad", true, false))));
        assert!(!should_query_quns(Some(&fg(SCREEN, "Progman", false, false))));
        assert!(!should_query_quns(Some(&fg(SCREEN, "TachyonTaskbarWnd", false, true))));
        assert!(!should_query_quns(None));
    }

    #[test]
    fn tray_windows_under_the_cursor_are_not_flyouts() {
        assert!(!is_flyout_under_cursor("Shell_TrayWnd", "explorer.exe"));
        assert!(!is_flyout_under_cursor("Shell_SecondaryTrayWnd", "explorer.exe"));
        assert!(is_flyout_under_cursor("TaskListThumbnailWnd", "explorer.exe"));
        assert!(is_flyout_under_cursor("Xaml_WindowedPopupClass", "explorer.exe"));
        assert!(!is_flyout_under_cursor("TaskListThumbnailWnd", "notepad.exe"));
    }

    #[test]
    fn fullscreen_foreground_decision() {
        // a borderless game covering the screen: fullscreen
        assert!(is_fullscreen_foreground(Some(&fg(SCREEN, "UnityWndClass", false, false)), NotifyState::None));
        // covering more than the screen (some games push the window a few px outside) still counts
        let wide = Rect { left: -2, top: -2, right: 1922, bottom: 1082 };
        assert!(is_fullscreen_foreground(Some(&fg(wide, "SDL_app", false, false)), NotifyState::None));
        // just 1 px short means it does not cover the screen
        let short = Rect { bottom: 1079, ..SCREEN };
        assert!(!is_fullscreen_foreground(Some(&fg(short, "SDL_app", false, false)), NotifyState::None));
        // an ordinary maximized window (with a title bar) is not fullscreen even if rect ⊇ screen (the bar reserves no space)
        assert!(!is_fullscreen_foreground(Some(&fg(wide, "Chrome_WidgetWin_1", true, false)), NotifyState::None));
        // desktop, native taskbar, the bar's own windows: never
        for class in ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"] {
            assert!(!is_fullscreen_foreground(Some(&fg(SCREEN, class, false, false)), NotifyState::None), "{class}");
            assert!(!is_fullscreen_foreground(Some(&fg(SCREEN, class, false, false)), NotifyState::Busy), "{class} + QUNS");
        }
        assert!(!is_fullscreen_foreground(Some(&fg(SCREEN, "TachyonTaskbarWnd", false, true)), NotifyState::Busy));
        // no foreground window
        assert!(!is_fullscreen_foreground(None, NotifyState::Busy));
    }

    #[test]
    fn shell_ui_windows_are_never_fullscreen() {
        // Start/Search, Task View/Alt+Tab, hidden tray, Xaml popup, menu: no caption, take the foreground, cover the screen
        for class in [
            "Windows.UI.Core.CoreWindow",
            "XamlExplorerHostIslandWindow",
            "NotifyIconOverflowWindow",
            "TopLevelWindowForOverflowXamlIsland",
            "Xaml_WindowedPopupClass",
            "#32768",
            "Shell_InputSwitchTopLevelWindow",
        ] {
            let w = fg(SCREEN, class, false, false);
            assert!(!is_fullscreen_foreground(Some(&w), NotifyState::None), "{class}");
            assert!(!is_fullscreen_foreground(Some(&w), NotifyState::Busy), "{class} + QUNS");
            assert!(!should_query_quns(Some(&w)), "{class} không đáng hỏi QUNS");
        }
    }

    #[test]
    fn quns_busy_counts_even_if_the_rect_does_not_cover() {
        // D3D exclusive / presentation mode: Windows reports busy even though the window rect does not ⊇ the screen
        let small = Rect { left: 100, top: 100, right: 800, bottom: 600 };
        assert!(is_fullscreen_foreground(Some(&fg(small, "GameClass", false, false)), NotifyState::Busy));
        assert!(!is_fullscreen_foreground(Some(&fg(small, "GameClass", false, false)), NotifyState::None));
    }

    #[test]
    fn auto_hide_zone_reaches_the_monitor_edge() {
        // floating bar: 48 high, 8 bottom margin
        let bar = Rect { left: 8, top: 1024, right: 1912, bottom: 1072 };
        assert_eq!(auto_hide_zone(bar, SCREEN, false), Rect { bottom: 1080, ..bar });
        let top_bar = Rect { left: 8, top: 8, right: 1912, bottom: 56 };
        assert_eq!(auto_hide_zone(top_bar, SCREEN, true), Rect { top: 0, ..top_bar });
        // the gap between the bar and the edge is inside the zone
        assert!(auto_hide_zone(bar, SCREEN, false).contains(500, 1078));
        assert!(!auto_hide_zone(bar, SCREEN, false).contains(500, 1023));
    }

    #[test]
    fn auto_hide_edge_is_two_pixels_within_the_bar_span() {
        let bar = Rect { left: 8, top: 1024, right: 1912, bottom: 1072 };
        assert!(cursor_on_bar_edge((500, 1079), bar, SCREEN, false));
        assert!(cursor_on_bar_edge((500, 1078), bar, SCREEN, false));
        assert!(!cursor_on_bar_edge((500, 1077), bar, SCREEN, false));
        assert!(!cursor_on_bar_edge((2, 1079), bar, SCREEN, false), "ngoài chiều ngang của thanh");
        let top_bar = Rect { left: 0, top: 0, right: 1920, bottom: 48 };
        assert!(cursor_on_bar_edge((500, 0), top_bar, SCREEN, true));
        assert!(cursor_on_bar_edge((500, 1), top_bar, SCREEN, true));
        assert!(!cursor_on_bar_edge((500, 2), top_bar, SCREEN, true));
        // second monitor: absolute coordinates
        let m2 = Rect { left: 1920, top: 0, right: 4480, bottom: 1440 };
        let bar2 = Rect { left: 1920, top: 1392, right: 4480, bottom: 1440 };
        assert!(cursor_on_bar_edge((3000, 1439), bar2, m2, false));
        assert!(!cursor_on_bar_edge((3000, 1079), bar2, m2, false));
    }

    fn ah(on_edge: bool, over: bool, popup: bool, now: u64) -> AutoHideInput {
        AutoHideInput { cursor_on_edge: on_edge, cursor_over_bar: over, popup_open: popup, now_ms: now }
    }

    #[test]
    fn auto_hide_conflict_only_for_reveal_on_the_same_edge() {
        use crate::model::{Edge, NativeMode};
        assert!(auto_hide_conflicts(NativeMode::Reveal, &[Edge::Bottom], Edge::Bottom));
        assert!(auto_hide_conflicts(NativeMode::Reveal, &[Edge::Top, Edge::Bottom], Edge::Bottom));
        assert!(!auto_hide_conflicts(NativeMode::Reveal, &[Edge::Bottom], Edge::Top));
        assert!(!auto_hide_conflicts(NativeMode::Reveal, &[], Edge::Bottom), "không có taskbar gốc nào");
        assert!(!auto_hide_conflicts(NativeMode::Hide, &[Edge::Bottom], Edge::Bottom));
        assert!(!auto_hide_conflicts(NativeMode::Keep, &[Edge::Bottom], Edge::Bottom));
    }

    #[test]
    fn horizontal_edge_from_taskbar_rect() {
        use crate::model::Edge;
        assert_eq!(horizontal_edge(Rect { left: 0, top: 1032, right: 1920, bottom: 1080 }, SCREEN), Some(Edge::Bottom));
        // auto-hiding: slid almost off screen
        assert_eq!(horizontal_edge(Rect { left: 0, top: 1078, right: 1920, bottom: 1126 }, SCREEN), Some(Edge::Bottom));
        assert_eq!(horizontal_edge(Rect { left: 0, top: 0, right: 1920, bottom: 48 }, SCREEN), Some(Edge::Top));
        assert_eq!(horizontal_edge(Rect { left: 0, top: 0, right: 48, bottom: 1080 }, SCREEN), None);
    }

    #[test]
    fn auto_hide_state_machine() {
        let mut s = RevealState::HIDDEN;
        // mouse in the middle of the screen: still hidden
        s = auto_hide_step(s, ah(false, false, false, 0));
        assert!(!s.visible);
        // touch the edge: shown immediately
        s = auto_hide_step(s, ah(true, true, false, 40));
        assert!(s.visible);
        // mouse over the bar: stays shown however long
        s = auto_hide_step(s, ah(false, true, false, 80));
        s = auto_hide_step(s, ah(false, true, false, 5000));
        assert!(s.visible);
        // mouse leaves: still shown before 300 ms, hidden once they have passed
        s = auto_hide_step(s, ah(false, false, false, 5100));
        s = auto_hide_step(s, ah(false, false, false, 5399));
        assert!(s.visible);
        s = auto_hide_step(s, ah(false, false, false, 5400));
        assert!(!s.visible);
    }

    #[test]
    fn auto_hide_keeps_the_bar_while_a_popup_is_open() {
        let mut s = auto_hide_step(RevealState::HIDDEN, ah(true, true, false, 0));
        s = auto_hide_step(s, ah(false, false, true, 100));
        s = auto_hide_step(s, ah(false, false, true, 10_000));
        assert!(s.visible, "popup/menu/thumbnail đang mở thì không ẩn");
        // close the popup: the 300 ms count starts from that moment
        s = auto_hide_step(s, ah(false, false, false, 10_100));
        s = auto_hide_step(s, ah(false, false, false, 10_399));
        assert!(s.visible);
        s = auto_hide_step(s, ah(false, false, false, 10_400));
        assert!(!s.visible);
    }

    #[test]
    fn hot_corner_fires_once() {
        let mut s = HotCornerState::default();
        let mut fired = 0;
        for t in (0..=600).step_by(30) {
            let (n, f) = hot_corner_step(s, true, t);
            s = n;
            if f {
                fired += 1;
                assert!(t >= HOT_CORNER_DWELL_MS);
            }
        }
        assert_eq!(fired, 1);
        // leave the corner and re-enter: no fire before 150 ms, fires again once they have passed
        let (s2, f) = hot_corner_step(s, false, 700);
        assert!(!f);
        let (s3, f) = hot_corner_step(s2, true, 730);
        assert!(!f);
        let (_, f) = hot_corner_step(s3, true, 730 + HOT_CORNER_DWELL_MS);
        assert!(f);
    }

    #[test]
    fn corner_hit_test() {
        let mon = Rect { left: 0, top: 0, right: 1920, bottom: 1080 };
        assert!(cursor_in_corner((0, 0), mon, Corner::TopLeft));
        assert!(cursor_in_corner((1919, 0), mon, Corner::TopRight));
        assert!(cursor_in_corner((1919, 1079), mon, Corner::BottomRight));
        assert!(cursor_in_corner((0, 1079), mon, Corner::BottomLeft));
        assert!(!cursor_in_corner((5, 5), mon, Corner::TopLeft));
        assert!(!cursor_in_corner((1919, 0), mon, Corner::TopLeft));
    }
}
