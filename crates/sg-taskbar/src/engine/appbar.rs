//! Sends the engine's AppBar commands to the RIGHT Explorer taskbar, even when this process's own fake `Shell_TrayWnd`
//! window (which captures tray icons, `sys/systray.rs`) sits in front of it.
//!
//! Measured on real hardware (Windows 11 26200, fix round 4, experiment in the report `fixB-report.md`): `SHAppBarMessage` finds the taskbar with
//! `FindWindow("Shell_TrayWnd")` — while the engine is capturing tray icons that returns THIS process's own fake window. When the taskbar is
//! in the caller's own process, shell32 does not allocate shared memory for the results (`ABM_QUERYPOS` / `ABM_SETPOS` /
//! `ABM_GETTASKBARPOS`) but writes an in-process POINTER into the `WM_COPYDATA` packet. The fake window forwards the packet to
//! Explorer, which cannot read that pointer: the returned rect is not adjusted, `ABM_GETTASKBARPOS` comes back all zeros, and
//! `ABM_SETPOS` does not make Explorer recompute the work area. A call from ANOTHER process through the same fake window works: shell32
//! gives a shared-memory handle to the process owning the fake window, and Explorer duplicates the handle from that process.
//!
//! The fix: when this process has its own `Shell_TrayWnd`, build a packet IDENTICAL to shell32's cross-process packet (the 64-byte
//! layout copied from a real packet, with a byte-by-byte test) and send it straight to explorer.exe's `Shell_TrayWnd`, allocating the
//! shared memory for the Explorer process the way shell32 does. With no fake window it still calls `SHAppBarMessage` as before (the documented path).

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::{
    SHAllocShared, SHAppBarMessage, SHFreeShared, SHLockShared, SHUnlockShared, ABM_GETTASKBARPOS, ABM_QUERYPOS, ABM_SETPOS, APPBARDATA,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, SendMessageW, WM_COPYDATA};

use crate::engine::sys::process;

/// Size of the common 32/64-bit `APPBARDATA` form that Explorer accepts (32-bit handles, 64-bit `lParam`).
pub(crate) const ABD_LEN: usize = 40;
/// Size of the whole `WM_COPYDATA` packet (`dwData = 0`) of `SHAppBarMessage` on Windows 11.
pub(crate) const PACKET_LEN: usize = 64;

/// The `APPBARDATA` fields that the packet carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AbdFields {
    pub hwnd: u32,
    pub callback: u32,
    pub edge: u32,
    /// left, top, right, bottom
    pub rc: [i32; 4],
    pub lparam: i64,
}

/// Common-form `APPBARDATA`: cbSize(4) hWnd(4) uCallbackMessage(4) uEdge(4) rc(16) lParam(8).
pub(crate) fn encode_abd(f: &AbdFields) -> [u8; ABD_LEN] {
    let mut b = [0u8; ABD_LEN];
    b[0..4].copy_from_slice(&(ABD_LEN as u32).to_le_bytes());
    b[4..8].copy_from_slice(&f.hwnd.to_le_bytes());
    b[8..12].copy_from_slice(&f.callback.to_le_bytes());
    b[12..16].copy_from_slice(&f.edge.to_le_bytes());
    for (i, v) in f.rc.iter().enumerate() {
        b[16 + i * 4..20 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    b[32..40].copy_from_slice(&f.lparam.to_le_bytes());
    b
}

/// The `WM_COPYDATA` packet: common `APPBARDATA` (0..40), `ABM_*` command code (40), 4 padding bytes, 64-bit result-memory handle (48; 0 when
/// the command has no result), PID of the process holding that handle = the target taskbar process (56), 4 padding bytes.
pub(crate) fn encode_packet(msg: u32, f: &AbdFields, output: u64, tray_pid: u32) -> [u8; PACKET_LEN] {
    let mut b = [0u8; PACKET_LEN];
    b[..ABD_LEN].copy_from_slice(&encode_abd(f));
    b[40..44].copy_from_slice(&msg.to_le_bytes());
    b[48..56].copy_from_slice(&output.to_le_bytes());
    b[56..60].copy_from_slice(&tray_pid.to_le_bytes());
    b
}

/// Reads back the edge + rect that Explorer wrote into the result memory (shell32 copies these two fields back too: `ABM_GETTASKBARPOS` returns
/// the taskbar's edge as well).
pub(crate) fn decode_output(b: &[u8; ABD_LEN]) -> (u32, [i32; 4]) {
    let u = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    (u(12), [u(16) as i32, u(20) as i32, u(24) as i32, u(28) as i32])
}

/// Commands whose result is returned through shared memory (exactly the three commands for which shell32 allocates memory).
pub(crate) fn needs_output(msg: u32) -> bool {
    msg == ABM_QUERYPOS || msg == ABM_SETPOS || msg == ABM_GETTASKBARPOS
}

/// A top-level window of class `Shell_TrayWnd`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TrayWnd {
    pub hwnd: isize,
    pub pid: u32,
    /// belongs to explorer.exe
    pub explorer: bool,
}

/// The route of an AppBar command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// This process has no `Shell_TrayWnd`: `SHAppBarMessage` reaches the right place by itself.
    Shell,
    /// This process has a fake `Shell_TrayWnd`: send straight to this Explorer taskbar.
    Direct { hwnd: isize, pid: u32 },
    /// There is a fake window but no Explorer taskbar was found (Explorer is restarting): treated as no taskbar, returns 0.
    NoTray,
}

/// Decides the route. If our own fake window exists we go direct, whether it is currently in front of or behind the real taskbar in Z order
/// (its timer raises it every 100 ms, and shell32's `FindWindow` may return it right after we check).
pub(crate) fn route(trays: &[TrayWnd], me: u32) -> Route {
    if !trays.iter().any(|t| t.pid == me) {
        return Route::Shell;
    }
    match trays.iter().find(|t| t.pid != me && t.explorer) {
        Some(t) => Route::Direct { hwnd: t.hwnd, pid: t.pid },
        None => Route::NoTray,
    }
}

/// Every top-level `Shell_TrayWnd` (capped at 32 to be safe). The exe is queried only when this process has a fake window (not needed otherwise).
fn list_trays(me: u32) -> Vec<TrayWnd> {
    let mut out = Vec::new();
    let mut after: Option<HWND> = None;
    for _ in 0..32 {
        // SAFETY: only enumerates top-level windows by class.
        let h = match unsafe { FindWindowExW(None, after, w!("Shell_TrayWnd"), PCWSTR::null()) } {
            Ok(h) if !h.is_invalid() => h,
            _ => break,
        };
        after = Some(h);
        out.push(TrayWnd { hwnd: h.0 as isize, pid: process::window_pid(h), explorer: false });
    }
    if out.iter().any(|t| t.pid == me) {
        for t in &mut out {
            t.explorer = t.pid != me
                && process::exe_path(t.pid).is_some_and(|p| p.to_ascii_lowercase().ends_with("\\explorer.exe"));
        }
    }
    out
}

/// The engine's `SHAppBarMessage`: same meaning, same return value, but the result is not swallowed by this process's own fake
/// `Shell_TrayWnd`.
pub fn message(msg: u32, d: &mut APPBARDATA) -> usize {
    // SAFETY: no arguments.
    let me = unsafe { GetCurrentProcessId() };
    match route(&list_trays(me), me) {
        // SAFETY: `d` is valid, the caller sets cbSize.
        Route::Shell => unsafe { SHAppBarMessage(msg, d) },
        Route::NoTray => {
            crate::engine::log::warn(&format!("AppBar: không thấy taskbar của Explorer, bỏ lệnh ABM {msg}"));
            0
        }
        // SAFETY: the HWND was just enumerated; if the window disappears `SendMessageW` simply returns 0.
        Route::Direct { hwnd, pid } => unsafe { send_direct(HWND(hwnd as *mut _), pid, msg, d) },
    }
}

/// Does what shell32 does when the taskbar is in another process: the result memory is pre-filled with the `APPBARDATA`, the handle is given to the
/// taskbar process; `SendMessageW` (like shell32, no timeout: Explorer sends messages back to the AppBar window and this thread can still
/// process them while waiting); then read back the edge + rect and free the memory.
unsafe fn send_direct(tray: HWND, tray_pid: u32, msg: u32, d: &mut APPBARDATA) -> usize {
    let f = AbdFields {
        hwnd: d.hWnd.0 as usize as u32,
        callback: d.uCallbackMessage,
        edge: d.uEdge,
        rc: [d.rc.left, d.rc.top, d.rc.right, d.rc.bottom],
        lparam: d.lParam.0 as i64,
    };
    let shared = if needs_output(msg) {
        let abd = encode_abd(&f);
        let h = SHAllocShared(Some(abd.as_ptr().cast()), ABD_LEN as u32, tray_pid);
        if h.is_invalid() {
            crate::engine::log::warn(&format!("AppBar: SHAllocShared thất bại (ABM {msg})"));
            return 0;
        }
        Some(h)
    } else {
        None
    };
    let packet = encode_packet(msg, &f, shared.map_or(0, |h| h.0 as usize as u64), tray_pid);
    let cds = COPYDATASTRUCT { dwData: 0, cbData: PACKET_LEN as u32, lpData: packet.as_ptr() as *mut _ };
    let ret = SendMessageW(tray, WM_COPYDATA, Some(WPARAM(d.hWnd.0 as usize)), Some(LPARAM(&cds as *const _ as isize)));
    if let Some(h) = shared {
        let p = SHLockShared(h, tray_pid);
        if p.is_null() {
            crate::engine::log::warn(&format!("AppBar: không đọc được kết quả của Explorer (ABM {msg})"));
        } else {
            let mut out = [0u8; ABD_LEN];
            std::ptr::copy_nonoverlapping(p as *const u8, out.as_mut_ptr(), ABD_LEN);
            let _ = SHUnlockShared(p);
            let (edge, rc) = decode_output(&out);
            d.uEdge = edge;
            d.rc = RECT { left: rc[0], top: rc[1], right: rc[2], bottom: rc[3] };
        }
        let _ = SHFreeShared(h, tray_pid);
    }
    ret.0 as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// A real packet sent by shell32 (Windows 11 26200) of another process to the fake window, copied from the round-4 experiment:
    /// hWnd 0x0020076e, uCallbackMessage 0x8077, target taskbar owned by PID 0x720.
    #[test]
    fn packet_matches_shell32_bytes() {
        let base = AbdFields { hwnd: 0x0020_076e, callback: 0x8077, ..Default::default() };
        // ABM_QUERYPOS bottom edge 48 px, result handle 0xC0A8
        let q = AbdFields { edge: 3, rc: [0, 1032, 1920, 1080], ..base };
        assert_eq!(
            encode_packet(ABM_QUERYPOS, &q, 0xC0A8, 0x720).to_vec(),
            hex("280000006e07200077800000030000000000000008040000800700003804000000000000000000000200000000000000a8c00000000000002007000000000000")
        );
        // ABM_QUERYPOS top edge, rect with negative numbers
        let t = AbdFields { edge: 1, rc: [-7, 0, 1927, 30], ..base };
        assert_eq!(
            encode_packet(ABM_QUERYPOS, &t, 0xC0A8, 0x720).to_vec(),
            hex("280000006e0720007780000001000000f9ffffff00000000870700001e00000000000000000000000200000000000000a8c00000000000002007000000000000")
        );
        // ABM_GETTASKBARPOS, result handle 0xC0CB
        assert_eq!(
            encode_packet(ABM_GETTASKBARPOS, &base, 0xC0CB, 0x720).to_vec(),
            hex("280000006e07200077800000000000000000000000000000000000000000000000000000000000000500000000000000cbc00000000000002007000000000000")
        );
        // ABM_REMOVE (1): no result memory, no callback
        let r = AbdFields { callback: 0, ..base };
        assert_eq!(
            encode_packet(1, &r, 0, 0x720).to_vec(),
            hex("280000006e0720000000000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000002007000000000000")
        );
    }

    #[test]
    fn lparam_is_a_signed_64_bit_field() {
        let b = encode_abd(&AbdFields { lparam: -1, ..Default::default() });
        assert_eq!(&b[32..40], &[0xff; 8]);
        let b = encode_abd(&AbdFields { lparam: 1, ..Default::default() });
        assert_eq!(&b[32..40], &[1, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn output_edge_and_rect_are_read_back() {
        let f = AbdFields { hwnd: 5, callback: 6, edge: 3, rc: [0, 1032, 1920, -4], lparam: 9 };
        assert_eq!(decode_output(&encode_abd(&f)), (3, [0, 1032, 1920, -4]));
    }

    #[test]
    fn only_querypos_setpos_and_gettaskbarpos_need_output() {
        for m in 0..=12u32 {
            assert_eq!(needs_output(m), matches!(m, 2 | 3 | 5), "ABM {m}");
        }
    }

    #[test]
    fn appbar_calls_go_direct_only_while_this_process_owns_a_shell_traywnd() {
        let me = 100;
        let explorer = TrayWnd { hwnd: 0x10, pid: 7, explorer: true };
        let own = TrayWnd { hwnd: 0x20, pid: me, explorer: false };
        let other_fake = TrayWnd { hwnd: 0x30, pid: 55, explorer: false };
        // no fake window of our own: the Windows path (even if another app has a fake window: cross-process still works)
        assert_eq!(route(&[explorer], me), Route::Shell);
        assert_eq!(route(&[other_fake, explorer], me), Route::Shell);
        assert_eq!(route(&[], me), Route::Shell);
        // our own fake window exists, whether in front of or behind the real taskbar
        assert_eq!(route(&[own, explorer], me), Route::Direct { hwnd: 0x10, pid: 7 });
        assert_eq!(route(&[explorer, own], me), Route::Direct { hwnd: 0x10, pid: 7 });
        // another app's fake window is not Explorer
        assert_eq!(route(&[own, other_fake, explorer], me), Route::Direct { hwnd: 0x10, pid: 7 });
        // Explorer has no taskbar yet
        assert_eq!(route(&[own], me), Route::NoTray);
        assert_eq!(route(&[own, other_fake], me), Route::NoTray);
    }

    // ------------------------------------------------------------------ real experiment (run by hand)

    use std::sync::atomic::{AtomicIsize, Ordering};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{ABE_BOTTOM, ABM_NEW, ABM_REMOVE};
    use windows::Win32::UI::WindowsAndMessaging::*;

    static TEST_REAL: AtomicIsize = AtomicIsize::new(0);
    static TEST_FAKE: AtomicIsize = AtomicIsize::new(0);

    /// The experiment's fake window: forwards EVERY `WM_COPYDATA` (other apps' tray icons, `dwData = 1`, none lost) and the
    /// `WM_USER+` messages like `systray.rs`.
    unsafe extern "system" fn test_fake_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> windows::Win32::Foundation::LRESULT {
        let real = HWND(TEST_REAL.load(Ordering::SeqCst) as *mut _);
        match msg {
            WM_COPYDATA => SendMessageW(real, msg, Some(wp), Some(lp)),
            WM_TIMER => {
                let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                windows::Win32::Foundation::LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(h);
                windows::Win32::Foundation::LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                windows::Win32::Foundation::LRESULT(0)
            }
            m if (WM_USER..0xC000).contains(&m) && m != WM_USER + 372 => SendMessageW(real, msg, Some(wp), Some(lp)),
            _ => DefWindowProcW(h, msg, wp, lp),
        }
    }

    /// Guaranteed cleanup (even when an assert fails): remove the test AppBar, destroy the window, close the fake window.
    struct Cleanup {
        bar: HWND,
        fake: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for Cleanup {
        fn drop(&mut self) {
            unsafe {
                if !self.bar.is_invalid() {
                    let mut d = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: self.bar, ..Default::default() };
                    message(ABM_REMOVE, &mut d);
                    let _ = DestroyWindow(self.bar);
                }
                let f = TEST_FAKE.swap(0, Ordering::SeqCst);
                if f != 0 {
                    let _ = PostMessageW(Some(HWND(f as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                if let Some(t) = self.fake.take() {
                    let _ = t.join();
                }
            }
        }
    }

    fn rect4(r: RECT) -> [i32; 4] {
        [r.left, r.top, r.right, r.bottom]
    }

    /// Waits `ms` while still processing the thread's messages (Explorer may send messages to the test AppBar window).
    fn pump(ms: u64) {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        let mut msg = MSG::default();
        while std::time::Instant::now() < end {
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn shell(msg: u32, d: &mut APPBARDATA) -> usize {
        unsafe { SHAppBarMessage(msg, d) }
    }

    /// Test AppBar (hidden window): ABM_NEW, ABM_QUERYPOS bottom edge 48 px, ABM_REMOVE — no ABM_SETPOS, so the work area
    /// is unchanged. Returns (GETTASKBARPOS, QUERYPOS).
    fn probe(send: fn(u32, &mut APPBARDATA) -> usize, bar: HWND, mon: RECT) -> ([i32; 4], [i32; 4]) {
        let mut d = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: bar, uCallbackMessage: WM_USER + 77, ..Default::default() };
        send(ABM_GETTASKBARPOS, &mut d);
        let tb = rect4(d.rc);
        assert_eq!(send(ABM_NEW, &mut d), 1, "ABM_NEW");
        d.uEdge = ABE_BOTTOM;
        d.rc = RECT { top: mon.bottom - 48, ..mon };
        send(ABM_QUERYPOS, &mut d);
        let q = rect4(d.rc);
        assert_eq!(send(ABM_REMOVE, &mut d), 1, "ABM_REMOVE");
        (tb, q)
    }

    /// Round-4 experiment using the engine's own code: a topmost fake `Shell_TrayWnd` in the test process makes
    /// `SHAppBarMessage` lose its result, while `message()` still returns the same as with no fake window, and its ABM_SETPOS makes Explorer
    /// recompute the work area. The work area changes for only ~0.3 s (test AppBar 30 px on the top edge, removed immediately), the original taskbar is not hidden,
    /// the fake window lives ~1 s and forwards all other messages. Run by hand:
    /// `cargo test -p sg-taskbar own_fake_tray_does_not_swallow_appbar_results_real -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn own_fake_tray_does_not_swallow_appbar_results_real() {
        let me = unsafe { GetCurrentProcessId() };
        let mon = crate::engine::win::monitors().into_iter().find(|m| m.primary).expect("màn hình chính").rect;
        let bar = unsafe {
            CreateWindowExW(WS_EX_TOOLWINDOW, w!("STATIC"), w!("sg-test-appbar"), WS_POPUP, 0, 0, 1, 1, None, None, None, None)
        }
        .expect("cửa sổ thử");
        let mut guard = Cleanup { bar, fake: None };
        assert_eq!(route(&list_trays(me), me), Route::Shell, "lúc đầu tiến trình test không có Shell_TrayWnd");
        let base = probe(shell, bar, mon);
        eprintln!("không có cửa sổ giả, SHAppBarMessage: GETTASKBARPOS {:?}, QUERYPOS dưới {:?}", base.0, base.1);

        let real = list_trays(me).into_iter().find(|t| t.pid != me).expect("taskbar của Explorer");
        TEST_REAL.store(real.hwnd, Ordering::SeqCst);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        guard.fake = Some(std::thread::spawn(move || unsafe {
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let wc = WNDCLASSW { lpfnWndProc: Some(test_fake_proc), hInstance: hinst.into(), lpszClassName: w!("Shell_TrayWnd"), ..Default::default() };
            RegisterClassW(&wc);
            if let Ok(h) =
                CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, w!("Shell_TrayWnd"), PCWSTR::null(), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), None)
            {
                TEST_FAKE.store(h.0 as isize, Ordering::SeqCst);
                let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                SetTimer(Some(h), 1, 100, None);
            }
            let _ = ready_tx.send(());
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                DispatchMessageW(&msg);
            }
            let _ = UnregisterClassW(w!("Shell_TrayWnd"), Some(hinst.into()));
        }));
        ready_rx.recv_timeout(std::time::Duration::from_secs(2)).expect("cửa sổ giả");
        let fake = TEST_FAKE.load(Ordering::SeqCst);
        assert_ne!(fake, 0, "không tạo được cửa sổ giả");
        std::thread::sleep(std::time::Duration::from_millis(250));
        let first = unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }.map(|h| h.0 as isize).unwrap_or(0);
        assert_eq!(first, fake, "FindWindow phải ra cửa sổ giả (như lúc engine bắt icon khay)");
        assert_eq!(route(&list_trays(me), me), Route::Direct { hwnd: real.hwnd, pid: real.pid });

        let broken = probe(shell, bar, mon);
        eprintln!("có cửa sổ giả, SHAppBarMessage (lỗi gốc): GETTASKBARPOS {:?}, QUERYPOS dưới {:?}", broken.0, broken.1);
        let fixed = probe(message, bar, mon);
        eprintln!("có cửa sổ giả, appbar::message: GETTASKBARPOS {:?}, QUERYPOS dưới {:?}", fixed.0, fixed.1);
        assert_eq!(fixed, base, "appbar::message phải cho đúng kết quả như khi không có cửa sổ giả");

        // ABM_SETPOS on the top edge, 30 px, via appbar::message: Explorer must recompute the work area (the original bug: it did not). The
        // work area changes for ~0.3 s then is restored by ABM_REMOVE.
        let work = |r: RECT| crate::engine::win::work_area_of(r).map(rect4).expect("vùng làm việc");
        let before = work(mon);
        let mut d = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: bar, uCallbackMessage: WM_USER + 77, ..Default::default() };
        assert_eq!(message(ABM_NEW, &mut d), 1, "ABM_NEW");
        d.uEdge = windows::Win32::UI::Shell::ABE_TOP;
        d.rc = RECT { bottom: mon.top + 30, ..mon };
        message(ABM_QUERYPOS, &mut d);
        d.rc.bottom = d.rc.top + 30;
        message(ABM_SETPOS, &mut d);
        pump(300);
        let during = work(mon);
        message(ABM_REMOVE, &mut d);
        pump(300);
        let after = work(mon);
        eprintln!("ABM_SETPOS trên 30 px qua appbar::message: rect {:?}, vùng làm việc {before:?} → {during:?} → {after:?}", rect4(d.rc));
        assert_eq!(during[1], d.rc.bottom, "vùng làm việc phải chừa tới đáy rect SETPOS");
        assert_eq!(after, before, "ABM_REMOVE phải trả lại vùng làm việc");
        drop(guard);
        assert_eq!(route(&list_trays(me), me), Route::Shell, "cửa sổ giả phải được huỷ");
    }
}
