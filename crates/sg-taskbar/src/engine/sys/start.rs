//! Opens the Windows Start menu and Search box as if pressing real keys (the bar's Windows / Search buttons).
//! The Win key is always sent both down and up in the same `SendInput`, so it never gets stuck.

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_LWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, PostMessageW, WM_SYSCOMMAND};

/// `SC_TASKLIST`: the system command that opens Start when sent to Explorer's `Shell_TrayWnd`.
const SC_TASKLIST: usize = 0xF130;
const VK_S: VIRTUAL_KEY = VIRTUAL_KEY(0x53);

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: vk, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, ..Default::default() },
        },
    }
}

/// Windows key down then up (opens Start).
pub fn start_input_sequence() -> [INPUT; 2] {
    [key(VK_LWIN, false), key(VK_LWIN, true)]
}

/// Win+S (opens the search box): Win down, S down, S up, Win up.
pub fn search_input_sequence() -> [INPUT; 4] {
    [key(VK_LWIN, false), key(VK_S, false), key(VK_S, true), key(VK_LWIN, true)]
}

/// Send the whole sequence; returns `true` if Windows accepted all of it. If not, release the Win key so it doesn't get stuck.
fn send(seq: &[INPUT]) -> bool {
    let sent = unsafe { SendInput(seq, std::mem::size_of::<INPUT>() as i32) } as usize;
    if sent == seq.len() {
        return true;
    }
    let release = release_for_partial(seq, sent);
    if !release.is_empty() {
        unsafe {
            SendInput(&release, std::mem::size_of::<INPUT>() as i32);
        }
    }
    false
}

/// Keys pressed down but not yet released within `seq[..sent]` (Windows only accepted the first `sent` events):
/// returns the matching release events in the reverse of the press order.
pub fn release_for_partial(seq: &[INPUT], sent: usize) -> Vec<INPUT> {
    let mut held: Vec<VIRTUAL_KEY> = Vec::new();
    for i in &seq[..sent.min(seq.len())] {
        let ki = unsafe { i.Anonymous.ki };
        if ki.dwFlags.contains(KEYEVENTF_KEYUP) {
            held.retain(|k| *k != ki.wVk);
        } else {
            held.push(ki.wVk);
        }
    }
    held.into_iter().rev().map(|vk| key(vk, true)).collect()
}

/// Explorer's real `Shell_TrayWnd`: the engine creates a fake one (topmost) inside this very process
/// (systray), so only accept a window that doesn't belong to our process.
fn explorer_tray() -> Option<HWND> {
    struct Ctx {
        found: Option<HWND>,
        me: u32,
    }
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let c = unsafe { &mut *(lp.0 as *mut Ctx) };
        let class = crate::engine::sys::process::class_name(h);
        if class == "Shell_TrayWnd" && !crate::engine::win::is_engine_window(&class, crate::engine::sys::process::window_pid(h), c.me) {
            c.found = Some(h);
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut ctx = Ctx { found: None, me: std::process::id() };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut _ as isize));
    }
    ctx.found
}

/// Open Start as if pressing the Windows key.
pub fn open_start() {
    if send(&start_input_sequence()) {
        return;
    }
    crate::engine::log::warn("SendInput không gửi đủ phím Windows, thử SC_TASKLIST.");
    if let Some(tray) = explorer_tray() {
        unsafe {
            let _ = PostMessageW(Some(tray), WM_SYSCOMMAND, WPARAM(SC_TASKLIST), LPARAM(0));
        }
    }
}

/// Open the search box (Win+S).
pub fn open_search() {
    if !send(&search_input_sequence()) {
        crate::engine::log::warn("SendInput không gửi đủ Win+S, mở ms-search:.");
        crate::engine::sys::shell::startfile("ms-search:");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vk_flags(i: &INPUT) -> (u16, u32) {
        assert_eq!(i.r#type, INPUT_KEYBOARD);
        let ki = unsafe { i.Anonymous.ki };
        (ki.wVk.0, ki.dwFlags.0)
    }

    #[test]
    fn start_input_sequence_is_win_down_then_up() {
        let s = start_input_sequence();
        assert_eq!(s.len(), 2);
        assert_eq!(vk_flags(&s[0]), (VK_LWIN.0, 0));
        assert_eq!(vk_flags(&s[1]), (VK_LWIN.0, KEYEVENTF_KEYUP.0));
    }

    #[test]
    fn release_for_partial_frees_every_held_key() {
        let rel = |seq: &[INPUT], sent| release_for_partial(seq, sent).iter().map(vk_flags).collect::<Vec<_>>();
        let up = KEYEVENTF_KEYUP.0;
        let s = search_input_sequence();
        assert_eq!(rel(&s, 0), []);
        assert_eq!(rel(&s, 1), [(VK_LWIN.0, up)]);
        assert_eq!(rel(&s, 2), [(0x53, up), (VK_LWIN.0, up)]);
        assert_eq!(rel(&s, 3), [(VK_LWIN.0, up)]);
        assert_eq!(rel(&s, 4), []);
        let st = start_input_sequence();
        assert_eq!(rel(&st, 1), [(VK_LWIN.0, up)]);
        assert_eq!(rel(&st, 2), []);
    }

    #[test]
    fn search_input_sequence_is_win_s() {
        let s = search_input_sequence();
        let v: Vec<_> = s.iter().map(vk_flags).collect();
        assert_eq!(v, [(VK_LWIN.0, 0), (0x53, 0), (0x53, KEYEVENTF_KEYUP.0), (VK_LWIN.0, KEYEVENTF_KEYUP.0)]);
    }
}
