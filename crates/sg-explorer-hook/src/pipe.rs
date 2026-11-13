//! The DLL's background thread: connects the pipe to the app, reads commands, sends layout/state. Never runs on the UI thread.
//!
//! - Overlapped I/O on one thread: waits at the same time for "bytes arrived" and "desktop thread wake-up".
//! - App dies → Windows closes the server end → `ReadFile` fails immediately → [`Session::on_disconnected`] restores defaults
//!   (pushed to the desktop thread via `PostMessage`, < 1 s) → retries connecting every [`RETRY_MS`].
//! - The buffers and `OVERLAPPED` live in [`Conn`]; `Drop` always cancels pending I/O and **waits for the cancel to finish** before
//!   freeing (even on panic), so the kernel never writes into released memory.

use crate::desktop;
use crate::guard::guarded;
use crate::log;
use crate::session::{DesktopPort, Session};
use sg_explorer_proto::{LOADED_MARKER, LabelRect, LabelsPaint, MAX_ITEMS, encode, pipe_name};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE, LPARAM, POINT, RECT, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
};
use windows::Win32::Graphics::Gdi::MapWindowPoints;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, ReadFile, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, GetCurrentProcessId, WaitForMultipleObjects, WaitForSingleObject};
use windows::Win32::UI::Controls::{LVIR_LABEL, LVM_GETITEMCOUNT, LVM_GETITEMRECT};
use windows::Win32::UI::WindowsAndMessaging::{IsWindowVisible, SMTO_ABORTIFHUNG, SMTO_ERRORONEXIT, SendMessageTimeoutW};
use windows::core::PCWSTR;

pub const RETRY_MS: u64 = 2000;
const READ_BUF: usize = 64 * 1024;
const WRITE_TIMEOUT_MS: u32 = 2000;
const MAX_THREAD_FAULTS: u32 = 3;

fn now_ms(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

pub fn build_string() -> String {
    format!("{}+p{}", env!("CARGO_PKG_VERSION"), sg_explorer_proto::PROTO_VERSION)
}

/// Body of the pipe thread (runs until Explorer exits).
pub fn thread_main() {
    // SAFETY: creates the DLL's own event/mutex. Never closed: lives as long as Explorer.
    unsafe {
        if let Ok(e) = CreateEventW(None, false, false, None) {
            desktop::WAKE_EVENT.store(e.0 as isize, Ordering::SeqCst);
        }
        let name: Vec<u16> = LOADED_MARKER.encode_utf16().chain(Some(0)).collect();
        // Marker telling the app "the DLL is already in Explorer" (do not load a second copy).
        let _ = CreateMutexW(None, false, PCWSTR(name.as_ptr()));
    }
    let mut sid = 0u32;
    // SAFETY: reads the session number of this very process.
    let pid = unsafe { GetCurrentProcessId() };
    unsafe {
        let _ = ProcessIdToSessionId(pid, &mut sid);
    }
    let name = pipe_name(sid);
    let start = Instant::now();
    let mut session = Session::new(RealDesk, pid, build_string());
    let mut faults = 0u32;
    loop {
        let ok = guarded("pipe", |_| false, || {
            run_connection(&name, &mut session, start);
            true
        });
        if !ok {
            faults += 1;
            session.on_fault("pipe thread panic".into());
            if faults >= MAX_THREAD_FAULTS {
                log::push("pipe thread stopping after repeated faults");
                log::flush();
                return;
            }
        }
        log::flush();
        std::thread::sleep(Duration::from_millis(RETRY_MS));
    }
}

/// One pipe connection, until it breaks. Always ends with `on_disconnected` (restoring defaults).
fn run_connection(name: &str, session: &mut Session<RealDesk>, start: Instant) {
    let Some(mut conn) = Conn::open(name) else { return };
    log::push("connected to app");
    let result = talk(&mut conn, session, start);
    session.on_disconnected();
    drop(conn);
    match result {
        Ok(()) => log::push("app disconnected; labels back to defaults"),
        Err(e) => log::push(format!("pipe closed: {e}; labels back to defaults")),
    }
    log::flush();
}

fn talk(conn: &mut Conn, session: &mut Session<RealDesk>, start: Instant) -> Result<(), String> {
    for m in session.on_connected() {
        conn.write(&encode(&m))?;
    }
    let wake = HANDLE(desktop::WAKE_EVENT.load(Ordering::SeqCst) as *mut core::ffi::c_void);
    loop {
        conn.start_read()?;
        let wait = session.wait_ms(now_ms(start)).max(10) as u32;
        let handles = if wake.is_invalid() { vec![conn.read_ev] } else { vec![conn.read_ev, wake] };
        // SAFETY: the event handles stay alive for the whole call.
        let w = unsafe { WaitForMultipleObjects(&handles, false, wait) };
        if w == WAIT_OBJECT_0 {
            let n = conn.finish_read()?;
            if n == 0 {
                continue;
            }
            let out = session.on_bytes(&conn.buf[..n], now_ms(start)).map_err(|e| e.to_string())?;
            for m in out {
                conn.write(&encode(&m))?;
            }
        } else if w != WAIT_TIMEOUT && w.0 != WAIT_OBJECT_0.0 + 1 {
            return Err("wait failed".into());
        }
        for m in session.on_tick(now_ms(start)) {
            conn.write(&encode(&m))?;
        }
        log::flush();
    }
}

/// Client end of the pipe + overlapped I/O buffers. Any pending I/O is cancelled and awaited in `Drop`.
struct Conn {
    h: HANDLE,
    read_ev: HANDLE,
    write_ev: HANDLE,
    read_ov: Box<OVERLAPPED>,
    write_ov: Box<OVERLAPPED>,
    buf: Box<[u8]>,
    read_pending: bool,
}

impl Conn {
    fn open(name: &str) -> Option<Conn> {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: opens the app's pipe (same user); SECURITY_IDENTIFICATION so the server cannot impersonate Explorer.
        unsafe {
            let h = CreateFileW(
                PCWSTR(wide.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | FILE_FLAGS_AND_ATTRIBUTES(SECURITY_SQOS_PRESENT.0 | SECURITY_IDENTIFICATION.0),
                None,
            )
            .ok()?;
            let read_ev = CreateEventW(None, true, false, None).ok();
            let write_ev = CreateEventW(None, true, false, None).ok();
            let (Some(read_ev), Some(write_ev)) = (read_ev, write_ev) else {
                let _ = CloseHandle(h);
                if let Some(e) = read_ev {
                    let _ = CloseHandle(e);
                }
                if let Some(e) = write_ev {
                    let _ = CloseHandle(e);
                }
                return None;
            };
            let read_ov = Box::new(OVERLAPPED { hEvent: read_ev, ..Default::default() });
            let write_ov = Box::new(OVERLAPPED { hEvent: write_ev, ..Default::default() });
            Some(Conn { h, read_ev, write_ev, read_ov, write_ov, buf: vec![0u8; READ_BUF].into_boxed_slice(), read_pending: false })
        }
    }

    /// Starts a read if none is pending.
    fn start_read(&mut self) -> Result<(), String> {
        if self.read_pending {
            return Ok(());
        }
        // SAFETY: buf and read_ov belong to Conn and live until Drop cancels and awaits the I/O.
        let r = unsafe { ReadFile(self.h, Some(&mut self.buf[..]), None, Some(&mut *self.read_ov as *mut OVERLAPPED)) };
        match r {
            Ok(()) => {
                self.read_pending = true;
                Ok(())
            }
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                self.read_pending = true;
                Ok(())
            }
            Err(e) => Err(format!("read: {e}")),
        }
    }

    fn finish_read(&mut self) -> Result<usize, String> {
        let mut n = 0u32;
        // SAFETY: the read has completed (the event signalled).
        let r = unsafe { GetOverlappedResult(self.h, &*self.read_ov, &mut n, false) };
        self.read_pending = false;
        r.map_err(|e| format!("read: {e}"))?;
        Ok(n as usize)
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        // SAFETY: bytes lives until the write finishes, or is cancelled and awaited within this function.
        unsafe {
            let r = WriteFile(self.h, Some(bytes), None, Some(&mut *self.write_ov as *mut OVERLAPPED));
            match r {
                Ok(()) => {}
                Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                    if WaitForSingleObject(self.write_ev, WRITE_TIMEOUT_MS) != WAIT_OBJECT_0 {
                        let _ = CancelIoEx(self.h, Some(&*self.write_ov));
                        let mut n = 0u32;
                        let _ = GetOverlappedResult(self.h, &*self.write_ov, &mut n, true);
                        return Err("write timed out".into());
                    }
                }
                Err(e) => return Err(format!("write: {e}")),
            }
            let mut n = 0u32;
            GetOverlappedResult(self.h, &*self.write_ov, &mut n, true).map_err(|e| format!("write: {e}"))?;
            if n as usize != bytes.len() {
                return Err("short write".into());
            }
        }
        Ok(())
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        // SAFETY: cancels the pending I/O of this very handle and waits for the cancel to finish; only then closes the handle/events.
        unsafe {
            if self.read_pending {
                let _ = CancelIoEx(self.h, Some(&*self.read_ov));
                let mut n = 0u32;
                let _ = GetOverlappedResult(self.h, &*self.read_ov, &mut n, true);
                self.read_pending = false;
            }
            let _ = CloseHandle(self.h);
            let _ = CloseHandle(self.read_ev);
            let _ = CloseHandle(self.write_ev);
        }
    }
}

/// The real bridge to the desktop thread.
struct RealDesk;

impl DesktopPort for RealDesk {
    fn set_paint(&mut self, paint: Option<Arc<LabelsPaint>>) {
        desktop::set_desired(paint);
    }

    fn take_layout_dirty(&mut self) -> bool {
        desktop::LAYOUT_DIRTY.swap(false, Ordering::SeqCst)
    }

    fn collect_layout(&mut self) -> Option<Vec<LabelRect>> {
        collect_layout()
    }

    fn take_fault(&mut self) -> Option<String> {
        desktop::take_fault()
    }

    fn set_log_path(&mut self, path: Option<String>) {
        log::set_path(path.map(Into::into));
    }
}

/// Reads the text rectangle of every item with `SendMessageTimeout` from this thread (same process, so RECT pointers work);
/// each call takes at most 300 ms, and if Explorer is busy this round is skipped.
fn collect_layout() -> Option<Vec<LabelRect>> {
    let dv = desktop::current_defview()?;
    let lv = desktop::listview_of(dv)?;
    let send = |msg: u32, wp: usize, lp: isize| -> Option<usize> {
        let mut out = 0usize;
        // SAFETY: lv is a window in the same process; lp points to a local variable that lives until the function returns (with a timeout).
        let r = unsafe { SendMessageTimeoutW(lv, msg, WPARAM(wp), LPARAM(lp), SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT, 300, Some(&mut out)) };
        (r.0 != 0).then_some(out)
    };
    // SAFETY: lv is alive (just checked).
    if !unsafe { IsWindowVisible(lv) }.as_bool() {
        return Some(Vec::new());
    }
    let count = send(LVM_GETITEMCOUNT, 0, 0)?.min(MAX_ITEMS);
    let mut items = Vec::with_capacity(count);
    for i in 0..count {
        let mut rc = RECT { left: LVIR_LABEL as i32, ..Default::default() };
        if send(LVM_GETITEMRECT, i, &mut rc as *mut RECT as isize)? == 0 {
            continue;
        }
        let mut pts = [POINT { x: rc.left, y: rc.top }, POINT { x: rc.right, y: rc.bottom }];
        // SAFETY: converts the listview's client coordinates to screen coordinates.
        unsafe {
            MapWindowPoints(Some(lv), None, &mut pts);
        }
        items.push(LabelRect { index: i as u32, left: pts[0].x, top: pts[0].y, right: pts[1].x, bottom: pts[1].y });
    }
    Some(items)
}
