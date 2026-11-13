//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! Apps pinned to the Windows taskbar (the `User Pinned\TaskBar` folder, ordered by the registry
//! `Taskband\Favorites`) and the running windows (filtered like Alt+Tab), which the taskbar widget matches up.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use windows::core::{Interface, BOOL, GUID, HSTRING};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::engine::ui::tree::Pixmap;

/// System.AppUserModel.ID
const PKEY_AUMID: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3), pid: 5 };

#[derive(Clone, Debug)]
pub struct Pinned {
    pub name: String,
    pub lnk: String,
    /// target exe path (lowercase)
    pub target: String,
    pub aumid: String,
}

#[derive(Clone, Debug)]
pub struct AppWindow {
    pub hwnd: isize,
    pub title: String,
    /// exe path (lowercase)
    pub exe: String,
    pub aumid: String,
    pub minimized: bool,
}

fn pinned_dir() -> PathBuf {
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    appdata.join("Microsoft").join("Internet Explorer").join("Quick Launch").join("User Pinned").join("TaskBar")
}

fn favorites_blob() -> Vec<u8> {
    use windows::Win32::System::Registry::*;
    unsafe {
        let key = HSTRING::from("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Taskband");
        let name = HSTRING::from("Favorites");
        let mut len = 0u32;
        if RegGetValueW(HKEY_CURRENT_USER, &key, &name, RRF_RT_REG_BINARY, None, None, Some(&mut len)).is_err() || len == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u8; len as usize];
        if RegGetValueW(HKEY_CURRENT_USER, &key, &name, RRF_RT_REG_BINARY, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut len)).is_err() {
            return Vec::new();
        }
        buf.truncate(len as usize);
        buf
    }
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w.eq_ignore_ascii_case(needle))
}

fn read_lnk(path: &str) -> (String, String) {
    unsafe {
        let Ok(link) = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER) else { return Default::default() };
        let Ok(pf) = link.cast::<IPersistFile>() else { return Default::default() };
        if pf.Load(&HSTRING::from(path), STGM_READ).is_err() {
            return Default::default();
        }
        let mut buf = [0u16; 1024];
        let target = if link.GetPath(&mut buf, std::ptr::null_mut(), 0).is_ok() { wstr(&buf) } else { String::new() };
        let aumid = link.cast::<IPropertyStore>().ok().map(|ps| prop_string(&ps)).unwrap_or_default();
        (target, aumid)
    }
}

fn prop_string(ps: &IPropertyStore) -> String {
    unsafe {
        let Ok(mut v) = ps.GetValue(&PKEY_AUMID) else { return String::new() };
        let s = PropVariantToStringAlloc(&v).ok().map(|p| {
            let r = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            r
        });
        let _ = PropVariantClear(&mut v);
        s.unwrap_or_default()
    }
}

fn wstr(s: &[u16]) -> String {
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..n])
}

/// Pinned apps, in the same order as on the taskbar.
pub fn pinned() -> Vec<Pinned> {
    let dir = pinned_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let blob = favorites_blob();
    let mut items: Vec<(usize, Pinned)> = Vec::new();
    let dup_suffix = regex::Regex::new(r"\s*\(\d+\)$").unwrap();
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("lnk")) != Some(true) {
            continue;
        }
        let file = p.file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
        let needle: Vec<u8> = file.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        let order = match find_bytes(&blob, &needle) {
            Some(o) => o,
            // a blob exists but no name was found → orphaned file (unpinned but the lnk remains)
            None if !blob.is_empty() => continue,
            None => usize::MAX,
        };
        let lnk = p.display().to_string();
        let (target, aumid) = read_lnk(&lnk);
        let stem = p.file_stem().and_then(|x| x.to_str()).unwrap_or("").to_string();
        let name = dup_suffix.replace(&stem, "").into_owned();
        items.push((order, Pinned { name, lnk, target: target.to_ascii_lowercase(), aumid }));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    items.into_iter().map(|x| x.1).collect()
}

type IconCache = HashMap<(String, u32), Option<Rc<Pixmap>>>;

thread_local! {
    static ICONS: RefCell<IconCache> = RefCell::new(HashMap::new());
    static EXES: RefCell<HashMap<u32, String>> = RefCell::new(HashMap::new());
}

/// High-quality icon image of a file/lnk via IShellItemImageFactory (like the taskbar).
pub fn shell_icon(path: &str, px: u32) -> Option<Rc<Pixmap>> {
    let key = (path.to_ascii_lowercase(), px);
    if let Some(v) = ICONS.with(|m| m.borrow().get(&key).cloned()) {
        return v;
    }
    let pix = unsafe {
        SHCreateItemFromParsingName::<_, _, IShellItemImageFactory>(&HSTRING::from(path), None)
            .ok()
            .and_then(|f| {
                // request a large image then downscale with WIC for sharpness
                let ask = if px <= 32 { 48 } else { 96 }.max(px as i32);
                f.GetImage(SIZE { cx: ask, cy: ask }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK).ok()
            })
            .and_then(|hbm| crate::engine::sys::image::from_hbitmap(hbm, px))
    }
    .or_else(|| crate::engine::sys::image::exe_icon(path, px));
    ICONS.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() > 200 {
            m.clear();
        }
        m.insert(key, pix.clone());
    });
    pix
}

fn window_aumid(h: HWND) -> String {
    unsafe { SHGetPropertyStoreForWindow::<IPropertyStore>(h).ok().map(|ps| prop_string(&ps)).unwrap_or_default() }
}

fn is_cloaked(h: HWND) -> bool {
    let mut v = 0u32;
    unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut v as *mut _ as *mut _, 4).is_ok() && v != 0 }
}

const SKIP_CLASSES: &[&str] = &["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "Windows.UI.Core.CoreWindow", "TachyonTaskbarWnd", "XamlExplorerHostIslandWindow"];

/// Open application windows in z-order (filtered like Alt+Tab / the taskbar).
pub fn windows() -> Vec<AppWindow> {
    struct Ctx(Vec<HWND>);
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let v = &mut *(lp.0 as *mut Ctx);
        v.0.push(h);
        BOOL(1)
    }
    let mut ctx = Ctx(Vec::new());
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut ctx as *mut _ as isize));
    }
    let me = std::process::id();
    let mut out = Vec::new();
    for h in ctx.0 {
        unsafe {
            if !IsWindowVisible(h).as_bool() {
                continue;
            }
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            let owner = GetWindow(h, GW_OWNER).unwrap_or_default();
            if ex & WS_EX_TOOLWINDOW.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0 {
                continue;
            }
            if !owner.0.is_null() && ex & WS_EX_APPWINDOW.0 == 0 {
                continue;
            }
            if ex & WS_EX_NOACTIVATE.0 != 0 && ex & WS_EX_APPWINDOW.0 == 0 {
                continue;
            }
            if is_cloaked(h) {
                continue;
            }
        }
        let title = crate::engine::sys::process::window_title(h);
        if title.trim().is_empty() {
            continue;
        }
        let class = crate::engine::sys::process::class_name(h);
        if SKIP_CLASSES.contains(&class.as_str()) {
            continue;
        }
        let pid = crate::engine::sys::process::window_pid(h);
        if crate::engine::win::is_engine_window(&class, pid, me) {
            continue;
        }
        let exe = EXES.with(|m| {
            let mut m = m.borrow_mut();
            if m.len() > 500 {
                m.clear();
            }
            m.entry(pid).or_insert_with(|| crate::engine::sys::process::exe_path(pid).unwrap_or_default().to_ascii_lowercase()).clone()
        });
        out.push(AppWindow { hwnd: h.0 as isize, title, exe, aumid: window_aumid(h), minimized: unsafe { IsIconic(h).as_bool() } });
    }
    out
}

fn file_name(p: &str) -> &str {
    p.rsplit(['\\', '/']).next().unwrap_or(p)
}

/// Whether the window belongs to this pinned app: same AUMID, or same target exe.
pub fn matches(p: &Pinned, w: &AppWindow) -> bool {
    if !p.aumid.is_empty() && !w.aumid.is_empty() {
        return p.aumid.eq_ignore_ascii_case(&w.aumid);
    }
    if p.target.is_empty() || w.exe.is_empty() {
        return false;
    }
    if p.target == w.exe {
        return true;
    }
    // Squirrel (Discord...): the lnk points to Update.exe in the same app folder
    let pn = file_name(&p.target);
    if pn == "update.exe" {
        let dir = &p.target[..p.target.len() - pn.len()];
        return w.exe.starts_with(dir);
    }
    pn == file_name(&w.exe)
}

// ------------------------------------------------------------------ actions

pub fn launch(lnk: &str) {
    crate::engine::sys::shell::startfile(lnk);
}


/// Brings the window to the front (restores it if minimized).
pub fn activate(h: isize) {
    let hwnd = HWND(h as *mut _);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if !SetForegroundWindow(hwnd).as_bool() {
            // foreground lock: press a fake Alt so Windows allows the switch
            let inp = [
                INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MENU, ..Default::default() } } },
                INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MENU, dwFlags: KEYEVENTF_KEYUP, ..Default::default() } } },
            ];
            SendInput(&inp, std::mem::size_of::<INPUT>() as i32);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

pub fn minimize(h: isize) {
    unsafe {
        let _ = ShowWindow(HWND(h as *mut _), SW_MINIMIZE);
    }
}

pub fn close(h: isize) {
    unsafe {
        let _ = PostMessageW(Some(HWND(h as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}
