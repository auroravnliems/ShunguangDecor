//! App side: launches the elevated helper process (UAC) and talks to it over a named pipe.
//!
//! Timeouts: connecting the pipe takes at most 60 s ([`CONNECT_TIMEOUT`]); EVERY [`HelperClient::call`] has a 10 s limit
//! ([`CALL_TIMEOUT`]) — once exceeded the pipe is considered broken (frames are no longer in sync), later calls return
//! [`ClientError::Closed`] and the page must prompt "Grant permission" again with a fresh `launch`.

use std::fmt;
use std::time::Duration;

use crate::protocol::{FrameError, Request, Response};

/// Time limit for each call over the pipe.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Time limit for the helper process to connect to the pipe (includes the time the user spends reading the UAC dialog).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// Launching the helper process failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchError {
    /// The user clicked "No" in the UAC dialog.
    Declined,
    Failed(String),
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Declined => write!(f, "chưa có quyền quản trị (đã từ chối hộp UAC)"),
            Self::Failed(m) => write!(f, "không mở được tiến trình quản trị: {m}"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// Error from a single call over the pipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// Timed out waiting for the helper process.
    Timeout,
    /// The pipe is closed (helper exited/died) or was already broken by a previous call.
    Closed,
    Protocol(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => write!(f, "tiến trình quản trị không trả lời kịp"),
            Self::Closed => write!(f, "kênh tới tiến trình quản trị đã đóng"),
            Self::Protocol(m) => write!(f, "lỗi giao thức: {m}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<FrameError> for ClientError {
    fn from(e: FrameError) -> Self {
        match e {
            FrameError::Eof => Self::Closed,
            FrameError::Io(io) => match io.kind() {
                std::io::ErrorKind::TimedOut => Self::Timeout,
                std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::ConnectionReset => Self::Closed,
                _ => Self::Protocol(io.to_string()),
            },
            other => Self::Protocol(other.to_string()),
        }
    }
}

pub use self::win::HelperClient;



mod win {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use ::windows::Win32::Foundation::{ERROR_CANCELLED, HANDLE, WAIT_OBJECT_0};
    use ::windows::Win32::System::Threading::{GetProcessId, WaitForSingleObject};
    use ::windows::Win32::UI::Shell::{
        SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use ::windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use ::windows::core::{HSTRING, PCWSTR, w};

    use super::*;
    use crate::helper::helper_args;
    use crate::pipe::{OwnedHandle, ServerPipe, new_pipe_name};
    use crate::protocol::{decode_response, encode_request, read_frame, write_frame};

    /// Channel to the elevated helper process.
    pub struct HelperClient {
        /// None after `close()`: the pipe handle is closed.
        pipe: Option<ServerPipe>,
        /// Helper process handle (only to tell whether it is alive / wait for it to exit); None in tests.
        process: Option<OwnedHandle>,
        call_timeout: Duration,
        broken: bool,
    }

    impl HelperClient {
        /// Launches `exe --elevated-helper …` via `ShellExecuteExW` with the `runas` verb (shows the UAC dialog) and waits for the pipe connection.
        /// After connecting, confirms the pipe's client IS the process just launched (PID) before allowing commands.
        pub fn launch(exe: &Path) -> Result<Self, LaunchError> {
            let name = new_pipe_name();
            let mut pipe = ServerPipe::create(&name)
                .map_err(|e| LaunchError::Failed(format!("tạo pipe: {e}")))?;
            let file = HSTRING::from(exe.as_os_str());
            let params = HSTRING::from(helper_args(&name, std::process::id()));
            let mut info = SHELLEXECUTEINFOW {
                cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask: SEE_MASK_NOCLOSEPROCESS,
                lpVerb: w!("runas"),
                lpFile: PCWSTR(file.as_ptr()),
                lpParameters: PCWSTR(params.as_ptr()),
                nShow: SW_HIDE.0,
                ..Default::default()
            };
            // SAFETY: `info` and the strings live for the whole call.
            if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
                return Err(if e.code() == ERROR_CANCELLED.to_hresult() {
                    LaunchError::Declined
                } else {
                    LaunchError::Failed(e.to_string())
                });
            }
            if info.hProcess.is_invalid() || info.hProcess.0.is_null() {
                return Err(LaunchError::Failed(
                    "Windows không trả handle tiến trình phụ".into(),
                ));
            }
            let process = OwnedHandle(info.hProcess);
            // SAFETY: valid process handle.
            let pid = unsafe { GetProcessId(process.0) };
            let proc_handle: HANDLE = process.0;
            // If the helper process dies early (error/blocked), stop waiting.
            let exited = move || unsafe { WaitForSingleObject(proc_handle, 0) } == WAIT_OBJECT_0;
            match pipe.accept(CONNECT_TIMEOUT, &exited) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    return Err(LaunchError::Failed(
                        "tiến trình quản trị đã thoát trước khi nối pipe".into(),
                    ));
                }
                Err(e) => return Err(LaunchError::Failed(format!("chờ nối pipe: {e}"))),
            }
            match pipe.client_pid() {
                Ok(p) if p == pid && pid != 0 => {}
                Ok(p) => {
                    return Err(LaunchError::Failed(format!(
                        "tiến trình nối vào pipe (PID {p}) không phải tiến trình vừa mở (PID {pid})"
                    )));
                }
                Err(e) => {
                    return Err(LaunchError::Failed(format!(
                        "không kiểm được PID client: {e}"
                    )));
                }
            }
            Ok(Self {
                pipe: Some(pipe),
                process: Some(process),
                call_timeout: CALL_TIMEOUT,
                broken: false,
            })
        }

        /// Build from an already-connected pipe (tests use a real pipe but skip UAC).
        #[cfg(test)]
        pub(crate) fn from_pipe(pipe: ServerPipe, call_timeout: Duration) -> Self {
            Self {
                pipe: Some(pipe),
                process: None,
                call_timeout,
                broken: false,
            }
        }

        /// Send one command and wait for the reply, at most `call_timeout` (10 s) for the whole round trip.
        pub fn call(&mut self, req: Request) -> Result<Response, ClientError> {
            if self.broken || self.pipe.is_none() {
                return Err(ClientError::Closed);
            }
            let r = self.exchange(&req);
            if r.is_err() {
                // After a timeout/mid-stream break the stream is out of frame sync; don't reuse it.
                self.broken = true;
            }
            r
        }

        fn exchange(&mut self, req: &Request) -> Result<Response, ClientError> {
            let pipe = self.pipe.as_mut().ok_or(ClientError::Closed)?;
            pipe.set_deadline(Instant::now() + self.call_timeout);
            write_frame(pipe, &encode_request(req))?;
            let bytes = read_frame(pipe)?;
            Ok(decode_response(&bytes)?)
        }

        /// The channel is healthy and the helper process hasn't exited (it exits on its own after 10 idle minutes). Does not wait.
        pub fn is_alive(&self) -> bool {
            if self.broken || self.pipe.is_none() {
                return false;
            }
            match &self.process {
                // SAFETY: the process handle is valid until `close()` takes it.
                Some(p) => (unsafe { WaitForSingleObject(p.0, 0) }) != WAIT_OBJECT_0,
                None => true,
            }
        }

        /// Tell the helper process to exit (1 s limit if the channel is healthy), then CLOSE the pipe handle right away (even if the
        /// channel is broken) so the helper sees EOF and exits by itself, and only then wait for it. Safe to call multiple times.
        pub fn close(&mut self) {
            if !self.broken && self.pipe.is_some() {
                let saved = self.call_timeout;
                self.call_timeout = Duration::from_secs(1);
                let _ = self.call(Request::Bye);
                self.call_timeout = saved;
            }
            self.broken = true;
            // Drop (close) the pipe BEFORE waiting for the process.
            drop(self.pipe.take());
            if let Some(p) = self.process.take() {
                // Wait briefly for it to exit; don't force it (we lack the rights, and once the pipe is closed it exits by itself).
                // SAFETY: valid handle.
                unsafe {
                    let _ = WaitForSingleObject(p.0, 1500);
                }
                drop(p);
            }
        }
    }

    impl Drop for HelperClient {
        fn drop(&mut self) {
            self.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helper::tests::Fake;
    use crate::helper::{self, Backend};
    use crate::model::{Priority, Store, Update};
    use crate::pipe::{ServerPipe, new_pipe_name};
    use std::fs::OpenOptions;
    use std::io::Read;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    /// Fake backend shared between the test thread and the "helper process" thread.
    struct Shared(Arc<Mutex<Fake>>);
    impl Backend for Shared {
        fn set(&mut self, s: Store, u: &[Update]) -> Result<(), i32> {
            self.0.lock().unwrap().set(s, u)
        }
        fn fix_priority(&mut self) -> Result<usize, i32> {
            self.0.lock().unwrap().fix_priority()
        }
        fn add_subs(&mut self, s: &[crate::Subscription]) -> Result<(), i32> {
            self.0.lock().unwrap().add_subs(s)
        }
        fn remove_subs(&mut self, s: &[crate::Subscription]) -> Result<(), i32> {
            self.0.lock().unwrap().remove_subs(s)
        }
        fn notify_usage(&mut self, r: &crate::UsageReport) -> Result<(), i32> {
            self.0.lock().unwrap().notify_usage(r)
        }
    }

    fn no_abort() -> bool {
        false
    }

    /// A real pipe inside the test process; a fake thread plays the helper process via `f`.
    fn with_helper<F>(f: F) -> (ServerPipe, std::thread::JoinHandle<i32>)
    where
        F: FnOnce(String) -> i32 + Send + 'static,
    {
        let name = new_pipe_name();
        let pipe = ServerPipe::create(&name).unwrap();
        let t = std::thread::spawn(move || f(name));
        (pipe, t)
    }

    #[test]
    fn real_pipe_roundtrip_with_fake_backend() {
        let fake = Arc::new(Mutex::new(Fake::default()));
        let f2 = Arc::clone(&fake);
        let me = std::process::id();
        let (mut pipe, t) =
            with_helper(move |name| helper::connect_and_serve(&name, me, &mut Shared(f2), None));
        pipe.accept(Duration::from_secs(5), &no_abort).unwrap();
        assert_eq!(pipe.client_pid().unwrap(), me); // pipe client = this process (the PID check works)
        let mut c = HelperClient::from_pipe(pipe, Duration::from_secs(5));
        assert_eq!(c.call(Request::Ping).unwrap(), Response::Ok);
        let set = Request::Set {
            store: Store::Runtime,
            updates: vec![Update::reset(5, Priority::User)],
        };
        assert_eq!(c.call(set).unwrap(), Response::Ok);
        c.close();
        assert_eq!(t.join().unwrap(), 0);
        assert_eq!(fake.lock().unwrap().calls, ["set Runtime 1"]);
        // After close() every call reports closed.
        assert_eq!(c.call(Request::Ping), Err(ClientError::Closed));
    }

    #[test]
    fn helper_rejects_server_with_wrong_pid() {
        let fake = Arc::new(Mutex::new(Fake::default()));
        let f2 = Arc::clone(&fake);
        let wrong = std::process::id().wrapping_add(1);
        let (mut pipe, t) =
            with_helper(move |name| helper::connect_and_serve(&name, wrong, &mut Shared(f2), None));
        pipe.accept(Duration::from_secs(5), &no_abort).unwrap();
        assert_eq!(t.join().unwrap(), 3);
        assert!(fake.lock().unwrap().calls.is_empty());
    }

    #[test]
    fn client_times_out_when_helper_gone() {
        // A fake helper connects, reads the command, then goes silent (as if hung).
        let (mut pipe, t) = with_helper(|name| {
            let mut f = OpenOptions::new()
                .read(true)
                .write(true)
                .open(name)
                .unwrap();
            let mut buf = [0u8; 4];
            let _ = f.read(&mut buf);
            std::thread::sleep(Duration::from_millis(1500));
            0
        });
        pipe.accept(Duration::from_secs(5), &no_abort).unwrap();
        let mut c = HelperClient::from_pipe(pipe, Duration::from_millis(300));
        let t0 = Instant::now();
        assert_eq!(c.call(Request::Ping), Err(ClientError::Timeout));
        assert!(
            t0.elapsed() < Duration::from_millis(1400),
            "phải trả về đúng hạn, không đợi helper"
        );
        // Once broken it stays locked: don't keep sending on an out-of-sync stream.
        assert_eq!(c.call(Request::Ping), Err(ClientError::Closed));
        t.join().unwrap();
    }

    #[test]
    fn client_reports_closed_when_helper_dies() {
        let (mut pipe, t) = with_helper(|name| {
            let f = OpenOptions::new()
                .read(true)
                .write(true)
                .open(name)
                .unwrap();
            drop(f); // dies right after connecting
            0
        });
        pipe.accept(Duration::from_secs(5), &no_abort).unwrap();
        t.join().unwrap();
        let mut c = HelperClient::from_pipe(pipe, Duration::from_secs(5));
        let t0 = Instant::now();
        assert_eq!(c.call(Request::Ping), Err(ClientError::Closed));
        assert!(t0.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn accept_times_out_and_can_be_aborted() {
        let mut pipe = ServerPipe::create(&new_pipe_name()).unwrap();
        let t0 = Instant::now();
        let e = pipe
            .accept(Duration::from_millis(300), &no_abort)
            .unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::TimedOut);
        assert!(t0.elapsed() < Duration::from_secs(2));
        let e = pipe.accept(Duration::from_secs(30), &|| true).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::Interrupted);
    }

    #[test]
    fn real_helper_exits_on_eof() {
        // The client closes outright (no Bye): the serve loop must finish.
        let me = std::process::id();
        let (mut pipe, t) = with_helper(move |name| {
            helper::connect_and_serve(&name, me, &mut Fake::default(), None)
        });
        pipe.accept(Duration::from_secs(5), &no_abort).unwrap();
        drop(pipe);
        assert_eq!(t.join().unwrap(), 0);
    }

    #[test]
    fn close_closes_pipe_even_when_channel_broken() {
        // Fake helper: reads the command then goes silent; afterwards keeps reading and reports when it sees EOF.
        let (mut pipe, t) = with_helper(|name| {
            let mut f = OpenOptions::new()
                .read(true)
                .write(true)
                .open(name)
                .unwrap();
            let mut buf = [0u8; 64];
            let _ = f.read(&mut buf); // the Ping command
            let t0 = Instant::now();
            // Block until the pipe closes; a read returns 0 (EOF) or an error.
            while matches!(f.read(&mut buf), Ok(n) if n > 0) {}
            t0.elapsed().as_millis() as i32
        });
        pipe.accept(Duration::from_secs(5), &no_abort).unwrap();
        let mut c = HelperClient::from_pipe(pipe, Duration::from_millis(300));
        assert_eq!(c.call(Request::Ping), Err(ClientError::Timeout)); // channel broken, close will skip Bye
        let t0 = Instant::now();
        c.close();
        assert!(t0.elapsed() < Duration::from_millis(500));
        let waited_ms = t.join().unwrap();
        assert!(
            waited_ms < 2000,
            "helper phải thấy EOF ngay sau close(), chờ {waited_ms} ms"
        );
        // A second close is harmless; a call after close reports Closed without touching the handle.
        c.close();
        assert_eq!(c.call(Request::Ping), Err(ClientError::Closed));
    }
}
