//! Finds the REAL shell window belonging to the process running the DLL (explorer.exe).
//!
//! A bare `FindWindowW` returns the first window in z-order. The app's taskbar engine deliberately creates a
//! FAKE `Shell_TrayWnd` inside the app process and pushes it topmost, so a bare FindWindow would return that fake window by mistake.
//! The DLL runs inside explorer.exe, so it only accepts windows whose pid equals its own pid.

/// Pure selection step: from a list of `(window, owner pid)` in z-order, takes the first one that belongs to `my_pid`.
pub fn pick_owned<H: Copy>(candidates: &[(H, u32)], my_pid: u32) -> Option<H> {
    candidates.iter().find(|(_, pid)| *pid == my_pid).map(|(h, _)| *h)
}

pub(crate) use win::find_own_window;

mod win {
    use super::pick_owned;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId};
    use windows::core::PCWSTR;

    /// Maximum number of same-class windows to walk (prevents an infinite loop if the z-order changes midway).
    const MAX_SCAN: usize = 64;

    /// The window of class `class` (a child of `parent`, or top-level if `None`) that belongs to this process.
    ///
    /// # Safety
    /// `class` must point to a NUL-terminated UTF-16 string that lives for the whole call.
    pub(crate) unsafe fn find_own_window(parent: Option<HWND>, class: PCWSTR) -> Option<HWND> {
        // SAFETY: only looks up windows and reads the owner pid; errors stop the walk.
        unsafe {
            let mut found: Vec<(HWND, u32)> = Vec::new();
            let mut prev: Option<HWND> = None;
            for _ in 0..MAX_SCAN {
                let Ok(h) = FindWindowExW(parent, prev, class, PCWSTR::null()) else { break };
                if h.is_invalid() {
                    break;
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(h, Some(&mut pid));
                found.push((h, pid));
                prev = Some(h);
            }
            pick_owned(&found, GetCurrentProcessId())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pick_owned;

    #[test]
    fn skips_fake_window_of_other_process() {
        // The app's fake window (pid 500) comes first (topmost), explorer's real one (pid 42) comes after.
        assert_eq!(pick_owned(&[(1, 500), (2, 42)], 42), Some(2));
    }

    #[test]
    fn none_when_no_window_is_ours() {
        assert_eq!(pick_owned(&[(1, 500), (2, 501)], 42), None);
        assert_eq!(pick_owned::<u32>(&[], 42), None);
    }

    #[test]
    fn first_owned_wins() {
        assert_eq!(pick_owned(&[(1, 42), (2, 42)], 42), Some(1));
    }
}
