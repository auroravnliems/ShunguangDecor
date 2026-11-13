//! App-side pipe server (single thread, overlapped I/O). One client at a time: the DLL inside Explorer.
//!
//! - DACL grants only the current user (`D:P(A;;GA;;;<SID>)`), Medium integrity label (the DLL in Medium-level Explorer can
//!   still connect when the app runs as admin), `PIPE_REJECT_REMOTE_CLIENTS`, `FILE_FLAG_FIRST_PIPE_INSTANCE` (if the name
//!   is already taken by another process we fail rather than share it).
//! - The client must be `explorer.exe` (checked via `GetNamedPipeClientProcessId` + exe name; process memory is not read).
//! - On connect → send `Hello` + the desired state; on disconnect → `DisconnectNamedPipe`, wait for the next connection (same handle).

use crate::{Shared, lock};
use sg_explorer_proto::{FrameBuf, FromDll, encode};
use std::sync::Arc;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, HLOCAL, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, GetCurrentProcessId, OpenProcessToken, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::{PCWSTR, PWSTR};

const BUF: u32 = 64 * 1024;
const WRITE_TIMEOUT_MS: u32 = 2000;

pub fn session_id() -> u32 {
    let mut sid = 0u32;
    // SAFETY: reads the session number of our own process.
    unsafe {
        let _ = ProcessIdToSessionId(GetCurrentProcessId(), &mut sid);
    }
    sid
}

/// Event that wakes the server thread (a new frame needs writing). Lives for the whole app lifetime.
pub struct WakeEvent(isize);

impl WakeEvent {
    pub fn new() -> WakeEvent {
        // SAFETY: creates an unnamed auto-reset event.
        WakeEvent(unsafe { CreateEventW(None, false, false, None) }.map(|h| h.0 as isize).unwrap_or(0))
    }
    pub fn handle(&self) -> HANDLE {
        HANDLE(self.0 as *mut core::ffi::c_void)
    }
    pub fn set(&self) {
        if self.0 != 0 {
            // SAFETY: our own event.
            unsafe {
                let _ = SetEvent(self.handle());
            }
        }
    }
}

impl Default for WakeEvent {
    fn default() -> Self {
        WakeEvent::new()
    }
}

/// SDDL for the current user only, Medium label.
fn current_user_sddl() -> Result<String, String> {
    // SAFETY: reads our own process token; the buffer has the size Windows reported; the returned SID string is freed with LocalFree.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|e| e.to_string())?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len.max(64) as usize];
        let r = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), buf.len() as u32, &mut len);
        let _ = CloseHandle(token);
        r.map_err(|e| e.to_string())?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).map_err(|e| e.to_string())?;
        let sid = s.to_string().map_err(|e| e.to_string());
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        Ok(format!("D:P(A;;GA;;;{})S:(ML;;NW;;;ME)", sid?))
    }
}

struct Security {
    sd: PSECURITY_DESCRIPTOR,
}

impl Security {
    fn new() -> Result<Security, String> {
        let sddl: Vec<u16> = current_user_sddl()?.encode_utf16().chain(Some(0)).collect();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: the SDDL string is zero-terminated; sd is freed with LocalFree in Drop.
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None) }
            .map_err(|e| e.to_string())?;
        Ok(Security { sd })
    }
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: self.sd.0, bInheritHandle: false.into() }
    }
}

impl Drop for Security {
    fn drop(&mut self) {
        // SAFETY: sd was allocated by ConvertStringSecurityDescriptor....
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.sd.0)));
        }
    }
}

/// Create the pipe on the calling thread (so pipe-creation errors are reported to `ensure_loaded`), then run the server thread.
pub fn spawn(shared: Arc<Shared>) -> Result<(), String> {
    let pipe = Pipe::create(&shared.pipe_name)?;
    std::thread::Builder::new()
        .name("sg-explorer-server".into())
        .spawn(move || run(shared, pipe))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

struct Pipe {
    h: HANDLE,
    ev: HANDLE,
    wev: HANDLE,
}

// SAFETY: kernel handles are usable from any thread; Pipe is only used by the server thread after being moved to it.
unsafe impl Send for Pipe {}

impl Pipe {
    fn create(name: &str) -> Result<Pipe, String> {
        let sec = Security::new()?;
        let sa = sec.attributes();
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: the name and security attributes live for the whole call.
        unsafe {
            let h = CreateNamedPipeW(
                PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                BUF,
                BUF,
                0,
                Some(&sa),
            );
            if h.is_invalid() || h == INVALID_HANDLE_VALUE {
                return Err(format!("cannot create pipe {name}: {}", windows::core::Error::from_thread()));
            }
            let ev = CreateEventW(None, true, false, None).map_err(|e| e.to_string())?;
            let wev = CreateEventW(None, true, false, None).map_err(|e| e.to_string())?;
            Ok(Pipe { h, ev, wev })
        }
    }

    /// Wait for a client to connect. `false` = error (the thread sleeps briefly, then retries).
    fn accept(&self) -> bool {
        let mut ov = OVERLAPPED { hEvent: self.ev, ..Default::default() };
        // SAFETY: ov lives until ConnectNamedPipe completes (waited on right in this function).
        unsafe {
            match ConnectNamedPipe(self.h, Some(&mut ov)) {
                Ok(()) => true,
                Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => true,
                Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                    let mut n = 0u32;
                    GetOverlappedResult(self.h, &ov, &mut n, true).is_ok()
                }
                Err(e) => {
                    tracing::warn!(target: "sg::explorer", "ConnectNamedPipe: {e}");
                    false
                }
            }
        }
    }

    fn client_pid(&self) -> Option<u32> {
        let mut pid = 0u32;
        // SAFETY: our own pipe handle.
        unsafe { GetNamedPipeClientProcessId(self.h, &mut pid) }.ok().map(|_| pid)
    }

    fn write(&self, bytes: &[u8]) -> Result<(), String> {
        let mut ov = OVERLAPPED { hEvent: self.wev, ..Default::default() };
        // SAFETY: bytes and ov live until the write completes, or is cancelled and awaited, right in this function.
        unsafe {
            match WriteFile(self.h, Some(bytes), None, Some(&mut ov)) {
                Ok(()) => {}
                Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                    if WaitForSingleObject(self.wev, WRITE_TIMEOUT_MS) != WAIT_OBJECT_0 {
                        let _ = CancelIoEx(self.h, Some(&ov));
                        let mut n = 0u32;
                        let _ = GetOverlappedResult(self.h, &ov, &mut n, true);
                        return Err("write timed out".into());
                    }
                }
                Err(e) => return Err(format!("write: {e}")),
            }
            let mut n = 0u32;
            GetOverlappedResult(self.h, &ov, &mut n, true).map_err(|e| format!("write: {e}"))?;
        }
        Ok(())
    }
}

fn run(shared: Arc<Shared>, pipe: Pipe) {
    tracing::info!(target: "sg::explorer", "pipe server listening on {}", shared.pipe_name);
    loop {
        if !pipe.accept() {
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        }
        let pid = pipe.client_pid().unwrap_or(0);
        if shared.require_explorer_client && !crate::inject::is_explorer_pid(pid) {
            tracing::warn!(target: "sg::explorer", "rejecting pipe client pid {pid}: not explorer.exe");
            // SAFETY: our own pipe handle.
            unsafe {
                let _ = DisconnectNamedPipe(pipe.h);
            }
            continue;
        }
        let r = serve(&shared, &pipe);
        if let Err(e) = r {
            tracing::info!(target: "sg::explorer", "pipe session ended: {e}");
        }
        // SAFETY: our own pipe handle; disconnect the client to accept the next connection.
        unsafe {
            let _ = DisconnectNamedPipe(pipe.h);
        }
        shared.on_client_disconnected();
    }
}

fn serve(shared: &Shared, pipe: &Pipe) -> Result<(), String> {
    for m in shared.on_client_connected() {
        pipe.write(&encode(&m))?;
    }
    let mut buf = vec![0u8; BUF as usize];
    let mut frames = FrameBuf::new();
    let mut ov = Box::new(OVERLAPPED { hEvent: pipe.ev, ..Default::default() });
    let mut pending = false;
    let result = (|| -> Result<(), String> {
        loop {
            if !pending {
                // SAFETY: buf and ov live until the read completes, or is cancelled and awaited (at the end of `serve`).
                match unsafe { ReadFile(pipe.h, Some(&mut buf[..]), None, Some(&mut *ov as *mut OVERLAPPED)) } {
                    Ok(()) => pending = true,
                    Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => pending = true,
                    Err(e) => return Err(format!("read: {e}")),
                }
            }
            let handles = [pipe.ev, shared.wake.handle()];
            // SAFETY: the event handles are still alive.
            let w = unsafe { WaitForMultipleObjects(&handles, false, 1000) };
            if w == WAIT_OBJECT_0 {
                let mut n = 0u32;
                // SAFETY: the read has completed.
                let r = unsafe { GetOverlappedResult(pipe.h, &*ov, &mut n, false) };
                pending = false;
                r.map_err(|e| format!("read: {e}"))?;
                frames.push(&buf[..n as usize]);
                while let Some(msg) = frames.next_msg::<FromDll>().map_err(|e| e.to_string())? {
                    shared.on_message(msg);
                }
            }
            loop {
                let next = lock(&shared.outgoing).pop_front();
                let Some(frame) = next else { break };
                pipe.write(&frame)?;
            }
        }
    })();
    if pending {
        // SAFETY: cancel this handle's own pending read, then wait for the cancel to finish before releasing buf/ov.
        unsafe {
            let _ = CancelIoEx(pipe.h, Some(&*ov));
            let mut n = 0u32;
            let _ = GetOverlappedResult(pipe.h, &*ov, &mut n, true);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ExplorerBridge;
    use sg_core::paths::Paths;
    use sg_explorer_proto::{FrameBuf, LabelsPaint, PROTO_VERSION, PaintMode, TextStyle, ToDll};
    use std::io::{Read, Write};
    use std::time::Duration;

    /// Fake "DLL": a synchronous pipe client inside the test process itself.
    fn connect(name: &str) -> std::fs::File {
        for _ in 0..100 {
            if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(name) {
                return f;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("cannot connect to {name}");
    }

    fn read_msgs(f: &mut std::fs::File, n: usize) -> Vec<ToDll> {
        let mut fb = FrameBuf::new();
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        while out.len() < n {
            let k = f.read(&mut buf).unwrap();
            assert!(k > 0, "pipe closed early");
            fb.push(&buf[..k]);
            while let Some(m) = fb.next_msg::<ToDll>().unwrap() {
                out.push(m);
            }
        }
        out
    }

    #[test]
    fn server_resends_state_on_reconnect() {
        let d = tempfile::tempdir().unwrap();
        let paths = Paths::resolve(d.path(), true, None, None);
        let name = format!(r"\\.\pipe\ShunguangDecor-test-{}-{}", std::process::id(), d.path().file_name().unwrap().to_string_lossy());
        let b = ExplorerBridge::with_pipe_name(b"", &paths, name.clone(), Some("C:/l.log".into()), false);
        let paint = LabelsPaint { mode: PaintMode::Uniform, uniform: Some(TextStyle::plain(0xFF00FF)), items: vec![] };
        b.send(ToDll::Labels(paint.clone()));
        spawn(b.shared.clone()).unwrap();

        // Connection 1: receive Hello + state; the DLL greets back → the bridge reports connected.
        let mut c = connect(&name);
        let m = read_msgs(&mut c, 2);
        assert_eq!(m[0], ToDll::Hello { version: PROTO_VERSION, log_path: Some("C:/l.log".into()) });
        assert_eq!(m[1], ToDll::Labels(paint.clone()));
        c.write_all(&encode(&FromDll::Hello { version: PROTO_VERSION, pid: 1, build: "t".into() })).unwrap();
        assert!(b.shared.wait_hello(Duration::from_secs(3)).is_some());
        assert!(b.is_connected());
        b.send(ToDll::Ping);
        assert_eq!(read_msgs(&mut c, 1), vec![ToDll::Ping]);
        // State changes while connected.
        let paint2 = LabelsPaint { uniform: Some(TextStyle::plain(0x00FF00)), ..paint.clone() };
        b.send(ToDll::Labels(paint2.clone()));
        assert_eq!(read_msgs(&mut c, 1), vec![ToDll::Labels(paint2.clone())]);

        // Disconnect (Explorer restarted / DLL unloaded) → reconnect → receive Hello + the LATEST state again.
        drop(c);
        for _ in 0..100 {
            if !b.is_connected() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!b.is_connected());
        let mut c = connect(&name);
        let m = read_msgs(&mut c, 2);
        assert!(matches!(m[0], ToDll::Hello { .. }));
        assert_eq!(m[1], ToDll::Labels(paint2));
        // DLL version mismatch → the bridge reports that Explorer needs a restart.
        c.write_all(&encode(&FromDll::Hello { version: PROTO_VERSION + 1, pid: 1, build: "old".into() })).unwrap();
        assert!(b.shared.wait_hello(Duration::from_secs(3)).is_some());
        assert!(b.status().needs_explorer_restart);
        assert!(!b.is_connected());
    }
}
