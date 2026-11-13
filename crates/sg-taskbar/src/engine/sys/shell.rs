//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! Opens files/URLs, runs commands, simulates Windows hotkeys (Win+N, Win+A...) and power commands.

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow, SW_SHOWNORMAL};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// ShellExecute on its own thread so the UI isn't blocked (like YASB's `shell_open`).
pub fn shell_open(file: &str, params: Option<&str>) {
    let file = file.to_string();
    let params = params.map(|s| s.to_string());
    std::thread::spawn(move || unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        let f = HSTRING::from(file.as_str());
        let p = params.as_deref().map(HSTRING::from);
        let r = ShellExecuteW(
            None,
            w!("open"),
            &f,
            p.as_ref().map(|x| PCWSTR(x.as_ptr())).unwrap_or(PCWSTR::null()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if (r.0 as isize) <= 32 {
            crate::engine::log::error(&format!("Không mở được '{file}' (mã {})", r.0 as isize));
        }
    });
}

/// Like `os.startfile`.
pub fn startfile(path: &str) {
    shell_open(path, None);
}

/// `subprocess.Popen(cmd, shell=True)`.
pub fn run_shell(cmd: &str, show_window: bool) {
    let flags = if show_window { CREATE_NEW_CONSOLE } else { CREATE_NO_WINDOW };
    let r = Command::new("cmd.exe")
        .raw_arg(format!("/C {cmd}"))
        .creation_flags(flags)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(e) = r {
        crate::engine::log::error(&format!("Không chạy được lệnh '{cmd}': {e}"));
    }
}

pub fn run(program: &str, args: &[&str], show_window: bool) {
    let flags = if show_window { CREATE_NEW_CONSOLE } else { CREATE_NO_WINDOW };
    if let Err(e) = Command::new(program).args(args).creation_flags(flags).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        crate::engine::log::error(&format!("Không chạy được '{program}': {e}"));
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP | KEYEVENTF_EXTENDEDKEY } else { KEYEVENTF_EXTENDEDKEY },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Bring the desktop to the foreground so UIPI doesn't block synthetic keys while an admin window has focus.
fn ensure_non_elevated_foreground() {
    unsafe {
        if let Ok(h) = FindWindowW(w!("Progman"), PCWSTR::null()) {
            let _ = SetForegroundWindow(h);
        }
    }
}

pub fn send_combo(mods: &[VIRTUAL_KEY], k: Option<VIRTUAL_KEY>) {
    ensure_non_elevated_foreground();
    let mut v = Vec::new();
    for m in mods {
        v.push(key(*m, false));
    }
    if let Some(k) = k {
        v.push(key(k, false));
        v.push(key(k, true));
    }
    for m in mods.iter().rev() {
        v.push(key(*m, true));
    }
    unsafe {
        SendInput(&v, std::mem::size_of::<INPUT>() as i32);
    }
}

/// YASB's `function_map` for the `exec <name>` callback.
pub fn system_function(name: &str) -> bool {
    match name {
        "notification_center" => send_combo(&[VK_LWIN], Some(VIRTUAL_KEY(b'N' as u16))),
        "quick_settings" => send_combo(&[VK_LWIN], Some(VIRTUAL_KEY(b'A' as u16))),
        "search" => send_combo(&[VK_LWIN], Some(VIRTUAL_KEY(b'S' as u16))),
        "widget" => send_combo(&[VK_LWIN], Some(VIRTUAL_KEY(b'W' as u16))),
        "start_menu" => send_combo(&[VK_LWIN], None),
        "launcher" => send_combo(&[VK_LMENU], Some(VK_SPACE)),
        _ => return false,
    }
    true
}

// ------------------------------------------------------------------ power (PowerOperations)

pub mod power {
    use super::*;

    pub fn lock() {
        run("rundll32.exe", &["user32.dll,LockWorkStation"], false);
    }

    pub fn sleep() {
        use windows::Win32::System::Power::SetSuspendState;
        enable_shutdown_privilege();
        unsafe {
            if !SetSuspendState(false, true, false) {
                run("rundll32.exe", &["powrprof.dll,SetSuspendState", "Sleep"], false);
            }
        }
    }

    pub fn hibernate() {
        run("shutdown", &["/h"], false);
    }

    pub fn restart() {
        run("shutdown", &["/r", "/t", "0"], false);
        crate::engine::app::request_exit();
    }

    pub fn shutdown() {
        run("shutdown", &["/s", "/hybrid", "/t", "0"], false);
        crate::engine::app::request_exit();
    }

    pub fn force_shutdown() {
        run("shutdown", &["/s", "/f", "/t", "0"], false);
        crate::engine::app::request_exit();
    }

    pub fn force_restart() {
        run("shutdown", &["/r", "/f", "/t", "0"], false);
        crate::engine::app::request_exit();
    }

    pub fn signout() {
        run("shutdown", &["/l"], false);
        crate::engine::app::request_exit();
    }

    fn enable_shutdown_privilege() {
        use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
        use windows::Win32::Security::*;
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        unsafe {
            let mut tok = HANDLE::default();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut tok).is_err() {
                return;
            }
            let mut luid = LUID::default();
            if LookupPrivilegeValueW(PCWSTR::null(), w!("SeShutdownPrivilege"), &mut luid).is_ok() {
                let tp = TOKEN_PRIVILEGES {
                    PrivilegeCount: 1,
                    Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
                };
                let _ = AdjustTokenPrivileges(tok, false, Some(&tp), 0, None, None);
            }
            let _ = CloseHandle(tok);
        }
    }
}

pub fn expand_user(p: &str) -> String {
    if let Some(rest) = p.strip_prefix('~') {
        let home = crate::engine::config::home_dir();
        let rest = rest.trim_start_matches(['\\', '/']);
        if rest.is_empty() {
            return home.display().to_string();
        }
        return home.join(rest).display().to_string();
    }
    p.to_string()
}
