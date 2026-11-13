//! SERVER end of the named pipe (app side): an overlapped pipe so every read/write/wait-for-connect has a deadline.
//!
//! The protections live here:
//! - The DACL contains only the current user's SID + BUILTIN\Administrators ([`pipe_sddl`]) — a different admin (over-the-shoulder)
//!   can still connect, but another user's processes cannot.
//! - `FILE_FLAG_FIRST_PIPE_INSTANCE`: creation fails if someone already took the name.
//! - `PIPE_REJECT_REMOTE_CLIENTS` + at most 1 instance.

use std::ffi::c_void;
use std::io;
use std::time::{Duration, Instant};

use ::windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, HANDLE,
    HLOCAL, LocalFree, WAIT_OBJECT_0,
};
use ::windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use ::windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use ::windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use ::windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use ::windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use ::windows::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, OpenProcessToken, WaitForSingleObject,
};
use ::windows::core::{HSTRING, PCWSTR, PWSTR};

use crate::helper::PIPE_PREFIX;

/// SDDL string of the DACL: grants the user SID and the Administrators group generic read/write, no one else.
/// `P` = blocks inheritance. A pure function, for testing.
pub fn pipe_sddl(user_sid: &str) -> String {
    format!("D:P(A;;GRGW;;;{user_sid})(A;;GRGW;;;BA)")
}

/// A new pipe name: `\\.\pipe\ShunguangDecor-elev-<pid>-<random>`.
pub fn new_pipe_name() -> String {
    use std::hash::{BuildHasher, Hasher};
    // RandomState is seeded randomly by the operating system on each creation.
    let r = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    format!("{PIPE_PREFIX}{}-{r:016x}", std::process::id())
}

/// Closes the handle on drop.
pub(crate) struct OwnedHandle(pub HANDLE);

// SAFETY: a kernel handle is usable from other threads; we close it only once.
unsafe impl Send for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0.0.is_null() {
            // SAFETY: we own the handle.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

fn user_sid_string() -> io::Result<String> {
    // SAFETY: calls the Win32 API with buffers we provide; the token is closed at the end.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(io::Error::other)?;
        let token = OwnedHandle(token);
        let mut len = 0u32;
        let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut len);
        if len == 0 {
            return Err(io::Error::other("GetTokenInformation không báo kích thước"));
        }
        // u64 to guarantee alignment for TOKEN_USER.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buf.as_mut_ptr().cast::<c_void>()),
            len,
            &mut len,
        )
        .map_err(io::Error::other)?;
        let tu = &*(buf.as_ptr().cast::<TOKEN_USER>());
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(tu.User.Sid, &mut s).map_err(io::Error::other)?;
        let out = s.to_string().map_err(io::Error::other);
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        out
    }
}

/// Server end of the pipe: overlapped, one instance.
pub struct ServerPipe {
    handle: OwnedHandle,
    event: OwnedHandle,
    /// Deadline for the next reads/writes; reset before each call.
    deadline: Instant,
}

// SAFETY: only used from one thread at a time (via &mut).
unsafe impl Send for ServerPipe {}

impl ServerPipe {
    /// Creates the pipe with a restricted DACL. Fails if the name already exists (`FILE_FLAG_FIRST_PIPE_INSTANCE`).
    pub fn create(name: &str) -> io::Result<Self> {
        let sddl = HSTRING::from(pipe_sddl(&user_sid_string()?));
        // SAFETY: the pointers only live for the duration of the call; the security descriptor is freed with LocalFree.
        unsafe {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .map_err(io::Error::other)?;
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: sd.0,
                bInheritHandle: false.into(),
            };
            let h = CreateNamedPipeW(
                &HSTRING::from(name),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 * 1024,
                64 * 1024,
                0,
                Some(&sa),
            );
            let _ = LocalFree(Some(HLOCAL(sd.0)));
            if h.is_invalid() {
                return Err(io::Error::last_os_error());
            }
            let handle = OwnedHandle(h);
            let event =
                OwnedHandle(CreateEventW(None, true, false, None).map_err(io::Error::other)?);
            Ok(Self {
                handle,
                event,
                deadline: Instant::now(),
            })
        }
    }

    /// Sets the deadline for subsequent reads/writes.
    pub fn set_deadline(&mut self, d: Instant) {
        self.deadline = d;
    }

    /// PID of the process connected to the pipe.
    pub fn client_pid(&self) -> io::Result<u32> {
        let mut pid = 0u32;
        // SAFETY: the handle is valid.
        unsafe { GetNamedPipeClientProcessId(self.handle.0, &mut pid) }
            .map_err(io::Error::other)?;
        Ok(pid)
    }

    /// Waits for a client to connect, up to `timeout`, but exits early (`Interrupted`) when `abort()` returns true
    /// (for example the helper process died / UAC was declined). Waits in 200 ms slices.
    pub fn accept(&mut self, timeout: Duration, abort: &dyn Fn() -> bool) -> io::Result<()> {
        let end = Instant::now() + timeout;
        let mut ov = self.new_overlapped();
        // SAFETY: `ov` lives until the operation completes or is cancelled and awaited.
        let started = unsafe { ConnectNamedPipe(self.handle.0, Some(&mut ov)) };
        match started {
            Ok(()) => return Ok(()),
            Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => return Ok(()),
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(e) => return Err(io::Error::other(e)),
        }
        loop {
            // SAFETY: the event is valid.
            let w = unsafe { WaitForSingleObject(self.event.0, 200) };
            if w == WAIT_OBJECT_0 {
                return Ok(());
            }
            let stop = if abort() {
                Some(io::ErrorKind::Interrupted)
            } else if Instant::now() >= end {
                Some(io::ErrorKind::TimedOut)
            } else {
                None
            };
            if let Some(kind) = stop {
                self.cancel_and_drain(&mut ov);
                return Err(io::Error::new(kind, "không có client nối vào pipe"));
            }
        }
    }

    fn new_overlapped(&self) -> OVERLAPPED {
        OVERLAPPED {
            hEvent: self.event.0,
            ..Default::default()
        }
    }

    /// Cancels the pending operation and waits for it to really stop (required before the buffer/OVERLAPPED is freed).
    fn cancel_and_drain(&self, ov: &mut OVERLAPPED) {
        let mut n = 0u32;
        // SAFETY: the handle + ov are valid; bwait = true waits until the system is done with `ov`.
        unsafe {
            let _ = CancelIoEx(self.handle.0, Some(ov));
            let _ = GetOverlappedResult(self.handle.0, ov, &mut n, true);
        }
    }

    /// Waits for the overlapped operation until `self.deadline`.
    fn finish(
        &self,
        started: ::windows::core::Result<()>,
        ov: &mut OVERLAPPED,
    ) -> io::Result<usize> {
        let mut n = 0u32;
        match started {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                let left = self.deadline.saturating_duration_since(Instant::now());
                // SAFETY: the event is valid.
                let w = unsafe {
                    WaitForSingleObject(
                        self.event.0,
                        left.as_millis().min(u32::MAX as u128 - 1) as u32,
                    )
                };
                if w != WAIT_OBJECT_0 {
                    self.cancel_and_drain(ov);
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "quá hạn chờ tiến trình phụ",
                    ));
                }
            }
            Err(e) => return map_pipe_error(e),
        }
        // SAFETY: the operation is done (the event signaled), so nothing is left pending.
        match unsafe { GetOverlappedResult(self.handle.0, ov, &mut n, false) } {
            Ok(()) => Ok(n as usize),
            Err(e) => map_pipe_error(e),
        }
    }
}

/// A closed pipe (BROKEN_PIPE/NO_DATA) is treated as end of data; other errors are kept as is.
fn map_pipe_error(e: ::windows::core::Error) -> io::Result<usize> {
    if e.code() == ERROR_BROKEN_PIPE.to_hresult() || e.code() == ERROR_NO_DATA.to_hresult() {
        Ok(0)
    } else {
        Err(io::Error::other(e))
    }
}

impl io::Read for ServerPipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut ov = self.new_overlapped();
        // SAFETY: buf and ov live until the operation finishes (finish waits/cancels).
        let started = unsafe { ReadFile(self.handle.0, Some(buf), None, Some(&mut ov)) };
        self.finish(started, &mut ov)
    }
}

impl io::Write for ServerPipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut ov = self.new_overlapped();
        // SAFETY: same as `read`.
        let started = unsafe { WriteFile(self.handle.0, Some(buf), None, Some(&mut ov)) };
        match self.finish(started, &mut ov)? {
            0 => Err(io::Error::new(io::ErrorKind::BrokenPipe, "pipe đã đóng")),
            n => Ok(n),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sddl_has_only_user_and_admins() {
        let s = pipe_sddl("S-1-5-21-1-2-3-1001");
        assert_eq!(s, "D:P(A;;GRGW;;;S-1-5-21-1-2-3-1001)(A;;GRGW;;;BA)");
        assert_eq!(s.matches("(A;").count(), 2);
        assert!(!s.contains("WD") && !s.contains("AU") && !s.contains("BU")); // no Everyone/Authenticated/Users
    }

    #[test]
    fn pipe_name_shape() {
        let a = new_pipe_name();
        let b = new_pipe_name();
        assert!(a.starts_with(PIPE_PREFIX));
        assert!(a.contains(&format!("-{}-", std::process::id())));
        assert_ne!(a, b);
    }

    #[test]
    fn user_sid_looks_like_a_sid() {
        assert!(user_sid_string().unwrap().starts_with("S-1-5-"));
    }

    #[test]
    fn second_instance_of_same_name_is_refused() {
        let name = new_pipe_name();
        let _first = ServerPipe::create(&name).unwrap();
        assert!(ServerPipe::create(&name).is_err());
    }
}
