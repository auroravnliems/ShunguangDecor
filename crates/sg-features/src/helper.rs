//! Helper process with admin rights (`ShunguangDecor.exe --elevated-helper --pipe <name> --parent <pid>`).
//!
//! The read/write loop ([`serve`]) is shared by all platforms and runs commands through the [`Backend`] trait:
//! the real one calls `platform::*` ([`PlatformBackend`]), tests use fakes — tests NEVER touch the real write functions.
//! The pipe connection + PID check exist on Windows only.

use std::io::{Read, Write};

use crate::model::{Store, Subscription, Update, UsageReport};
use crate::protocol::{
    FrameError, Request, Response, decode_request, encode_response, read_frame, write_frame,
};

/// If no command arrives within this time, the helper process exits on its own.
pub const IDLE_LIMIT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Where real commands are executed. Only the defined write operations exist, nothing else.
pub trait Backend {
    fn set(&mut self, store: Store, updates: &[Update]) -> Result<(), i32>;
    fn fix_priority(&mut self) -> Result<usize, i32>;
    fn add_subs(&mut self, subs: &[Subscription]) -> Result<(), i32>;
    fn remove_subs(&mut self, subs: &[Subscription]) -> Result<(), i32>;
    fn notify_usage(&mut self, report: &UsageReport) -> Result<(), i32>;
}

/// The real one: calls `platform::*` (writes to the system — only runs in the elevated helper process).
pub struct PlatformBackend;

impl Backend for PlatformBackend {
    fn set(&mut self, store: Store, updates: &[Update]) -> Result<(), i32> {
        crate::platform::set(updates, store)
    }
    fn fix_priority(&mut self) -> Result<usize, i32> {
        crate::platform::fix_priority()
    }
    fn add_subs(&mut self, subs: &[Subscription]) -> Result<(), i32> {
        crate::platform::add_subscriptions(subs)
    }
    fn remove_subs(&mut self, subs: &[Subscription]) -> Result<(), i32> {
        crate::platform::remove_subscriptions(subs)
    }
    fn notify_usage(&mut self, report: &UsageReport) -> Result<(), i32> {
        crate::platform::notify_usage(report)
    }
}

fn status_to_response(r: Result<(), i32>) -> Response {
    match r {
        Ok(()) => Response::Ok,
        Err(status) => Response::Err {
            status,
            message: format!("NTSTATUS 0x{:08X}", status as u32),
        },
    }
}

/// Runs one command (except `Bye`, which the loop handles).
pub fn handle<B: Backend>(backend: &mut B, req: &Request) -> Response {
    match req {
        Request::Set { store, updates } => status_to_response(backend.set(*store, updates)),
        Request::FixPriority => status_to_response(backend.fix_priority().map(|_| ())),
        Request::AddSubs(subs) => status_to_response(backend.add_subs(subs)),
        Request::RemoveSubs(subs) => status_to_response(backend.remove_subs(subs)),
        Request::NotifyUsage(report) => status_to_response(backend.notify_usage(report)),
        Request::Ping | Request::Bye => Response::Ok,
    }
}

/// Why the serving loop stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum ServeEnd {
    /// The app closed the pipe.
    Eof,
    /// Received `Bye`.
    Bye,
    /// Frame too large or I/O error: the stream is no longer in sync, so stop for good.
    Aborted,
}

/// Loop: read a command → run it → return the result. Bad JSON returns `Err` and keeps reading, no panic.
/// `on_activity` is called each time a command starts/finishes processing (to watch for 10 idle minutes).
pub fn serve<S: Read + Write, B: Backend>(
    stream: &mut S,
    backend: &mut B,
    on_activity: &dyn Fn(),
) -> ServeEnd {
    loop {
        let bytes = match read_frame(stream) {
            Ok(b) => b,
            Err(FrameError::Eof) => return ServeEnd::Eof,
            Err(FrameError::TooLarge(n)) => {
                // The body has not been read, so the stream is out of frame: report the error and stop.
                let resp = Response::Err {
                    status: -1,
                    message: format!("tin nhắn quá lớn ({n} byte)"),
                };
                let _ = write_frame(stream, &encode_response(&resp));
                return ServeEnd::Aborted;
            }
            Err(_) => return ServeEnd::Aborted,
        };
        on_activity();
        let (resp, bye) = match decode_request(&bytes) {
            Ok(Request::Bye) => (Response::Ok, true),
            Ok(req) => (handle(backend, &req), false),
            Err(e) => (
                Response::Err {
                    status: -1,
                    message: e.to_string(),
                },
                false,
            ),
        };
        let wrote = write_frame(stream, &encode_response(&resp));
        on_activity();
        if bye {
            return ServeEnd::Bye;
        }
        if wrote.is_err() {
            return ServeEnd::Aborted;
        }
    }
}

/// First argument of the helper process.
pub const HELPER_ARG: &str = "--elevated-helper";

/// Every pipe of this part starts with this prefix; the helper process rejects other names.
pub const PIPE_PREFIX: &str = r"\\.\pipe\ShunguangDecor-elev-";

/// Command-line arguments for launching the helper process (pure function, tested).
pub fn helper_args(pipe: &str, parent: u32) -> String {
    format!("{HELPER_ARG} --pipe \"{pipe}\" --parent {parent}")
}

/// Reads `--pipe <name> --parent <pid>`; returns `None` if wrong/missing or the pipe name lacks the prefix.
pub fn parse_args(args: &[String]) -> Option<(String, u32)> {
    let get = |key: &str| args.windows(2).find(|w| w[0] == key).map(|w| w[1].clone());
    let pipe = get("--pipe")?;
    let parent: u32 = get("--parent")?.parse().ok()?;
    // Only connect to pipes named by ShunguangDecor, never to an arbitrary pipe.
    if !pipe.starts_with(PIPE_PREFIX) || pipe.len() > 200 || parent == 0 {
        return None;
    }
    Some((pipe, parent))
}

pub use self::win::{connect_and_serve, run};

mod win {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use ::windows::Win32::Foundation::HANDLE;
    use ::windows::Win32::System::Pipes::GetNamedPipeServerProcessId;

    use super::{Backend, IDLE_LIMIT, PlatformBackend, parse_args, serve};

    const SECURITY_SQOS_PRESENT: u32 = 0x0010_0000;
    const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;

    /// Connects to pipe `pipe`, verifies that the pipe's owner (the server) is process `parent`, then serves.
    /// `idle` = `Some(d)`: if no command arrives within `d`, the process exits (only used by `run`).
    /// Returns the exit code: 0 normal, 2 could not connect, 3 wrong PID (another process is posing as the app).
    pub fn connect_and_serve<B: Backend>(
        pipe: &str,
        parent: u32,
        backend: &mut B,
        idle: Option<Duration>,
    ) -> i32 {
        // Impersonation level = Identification: even if the pipe is held by someone else, it cannot borrow our admin rights.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION)
            .open(pipe);
        let mut file = match file {
            Ok(f) => f,
            Err(_) => return 2,
        };
        let mut server_pid = 0u32;
        // SAFETY: the handle is valid for the whole lifetime of `file`; `server_pid` is a local variable.
        let ok =
            unsafe { GetNamedPipeServerProcessId(HANDLE(file.as_raw_handle()), &mut server_pid) }
                .is_ok();
        if !ok || server_pid != parent {
            return 3;
        }
        let last = Arc::new(AtomicU64::new(0));
        let start = Instant::now();
        if let Some(limit) = idle {
            let last = Arc::clone(&last);
            std::thread::spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(5));
                    let quiet = start
                        .elapsed()
                        .saturating_sub(Duration::from_millis(last.load(Ordering::Relaxed)));
                    if quiet >= limit {
                        std::process::exit(0);
                    }
                }
            });
        }
        let touch = {
            let last = Arc::clone(&last);
            move || last.store(start.elapsed().as_millis() as u64, Ordering::Relaxed)
        };
        serve(&mut file, backend, &touch);
        0
    }

    /// Body of `--elevated-helper`. Opens no window, creates no WebView, enables no module.
    pub fn run(args: &[String]) -> i32 {
        let Some((pipe, parent)) = parse_args(args) else {
            return 1;
        };
        connect_and_serve(&pipe, parent, &mut PlatformBackend, Some(IDLE_LIMIT))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::Priority;
    use crate::protocol::{MAX_FRAME, decode_response, encode_request};
    use std::io::{self, Cursor};

    /// Fake backend: only records commands, never touches the system.
    #[derive(Default)]
    pub(crate) struct Fake {
        pub calls: Vec<String>,
        pub fail_set: Option<i32>,
    }

    impl Backend for Fake {
        fn set(&mut self, store: Store, updates: &[Update]) -> Result<(), i32> {
            self.calls.push(format!("set {store:?} {}", updates.len()));
            self.fail_set.map_or(Ok(()), Err)
        }
        fn fix_priority(&mut self) -> Result<usize, i32> {
            self.calls.push("fix_priority".into());
            Ok(3)
        }
        fn add_subs(&mut self, subs: &[Subscription]) -> Result<(), i32> {
            self.calls.push(format!("add_subs {}", subs.len()));
            Ok(())
        }
        fn remove_subs(&mut self, subs: &[Subscription]) -> Result<(), i32> {
            self.calls.push(format!("remove_subs {}", subs.len()));
            Ok(())
        }
        fn notify_usage(&mut self, r: &UsageReport) -> Result<(), i32> {
            self.calls.push(format!("notify {}", r.id));
            Ok(())
        }
    }

    /// In-memory stream: reads from `input`, writes to `output`.
    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl Read for Duplex {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            self.input.read(b)
        }
    }
    impl Write for Duplex {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.output.write(b)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn frames(items: &[&[u8]]) -> Vec<u8> {
        let mut v = Vec::new();
        for i in items {
            write_frame(&mut v, i).unwrap();
        }
        v
    }

    fn responses(out: &[u8]) -> Vec<Response> {
        let mut cur = Cursor::new(out.to_vec());
        let mut v = Vec::new();
        while let Ok(b) = read_frame(&mut cur) {
            v.push(decode_response(&b).unwrap());
        }
        v
    }

    #[test]
    fn serve_runs_commands_and_stops_on_bye() {
        let set = Request::Set {
            store: Store::Runtime,
            updates: vec![Update::reset(7, Priority::User)],
        };
        let input = frames(&[
            &encode_request(&Request::Ping),
            &encode_request(&set),
            &encode_request(&Request::FixPriority),
            &encode_request(&Request::Bye),
            &encode_request(&Request::Ping), // must not run after Bye
        ]);
        let mut s = Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        };
        let mut fake = Fake::default();
        assert_eq!(serve(&mut s, &mut fake, &|| {}), ServeEnd::Bye);
        assert_eq!(fake.calls, ["set Runtime 1", "fix_priority"]);
        assert_eq!(responses(&s.output), vec![Response::Ok; 4]);
    }

    #[test]
    fn serve_reports_backend_error() {
        let input = frames(&[&encode_request(&Request::Set {
            store: Store::Boot,
            updates: vec![],
        })]);
        let mut s = Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        };
        let mut fake = Fake {
            fail_set: Some(0xC000_0022_u32 as i32),
            ..Default::default()
        };
        assert_eq!(serve(&mut s, &mut fake, &|| {}), ServeEnd::Eof);
        match &responses(&s.output)[0] {
            Response::Err { status, message } => {
                assert_eq!(*status as u32, 0xC000_0022);
                assert!(message.contains("C0000022"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn serve_malformed_or_unknown_returns_err_and_keeps_going() {
        let input = frames(&[
            b"{{{ not json",
            br#"{"cmd":"run","args":"calc.exe"}"#,
            &encode_request(&Request::Ping),
        ]);
        let mut s = Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        };
        let mut fake = Fake::default();
        assert_eq!(serve(&mut s, &mut fake, &|| {}), ServeEnd::Eof);
        let r = responses(&s.output);
        assert!(matches!(r[0], Response::Err { .. }));
        assert!(matches!(r[1], Response::Err { .. }));
        assert_eq!(r[2], Response::Ok);
        assert!(fake.calls.is_empty()); // an unknown command runs nothing
    }

    #[test]
    fn serve_oversized_frame_aborts_with_error_reply() {
        let input = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
        let mut s = Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        };
        assert_eq!(
            serve(&mut s, &mut Fake::default(), &|| {}),
            ServeEnd::Aborted
        );
        assert!(matches!(responses(&s.output)[0], Response::Err { .. }));
    }

    #[test]
    fn serve_ends_on_eof_and_truncated_frame() {
        let mut s = Duplex {
            input: Cursor::new(Vec::new()),
            output: Vec::new(),
        };
        assert_eq!(serve(&mut s, &mut Fake::default(), &|| {}), ServeEnd::Eof);
        let mut s = Duplex {
            input: Cursor::new(vec![5, 0, 0, 0, b'x']),
            output: Vec::new(),
        };
        assert_eq!(
            serve(&mut s, &mut Fake::default(), &|| {}),
            ServeEnd::Aborted
        );
    }

    #[test]
    fn serve_reports_activity() {
        use std::cell::Cell;
        let n = Cell::new(0);
        let input = frames(&[&encode_request(&Request::Ping)]);
        let mut s = Duplex {
            input: Cursor::new(input),
            output: Vec::new(),
        };
        serve(&mut s, &mut Fake::default(), &|| n.set(n.get() + 1));
        assert_eq!(n.get(), 2);
    }

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_args_accepts_only_our_pipe_and_valid_pid() {
        let good = r"\\.\pipe\ShunguangDecor-elev-12-abc";
        assert_eq!(
            parse_args(&a(&[
                "exe",
                "--elevated-helper",
                "--pipe",
                good,
                "--parent",
                "12"
            ])),
            Some((good.to_string(), 12))
        );
        for bad in [
            a(&["exe", "--elevated-helper"]),
            a(&["exe", "--pipe", good]),
            a(&["exe", "--pipe", good, "--parent", "x"]),
            a(&["exe", "--pipe", good, "--parent", "0"]),
            a(&["exe", "--pipe", r"\\.\pipe\other", "--parent", "1"]),
            a(&["exe", "--pipe", r"C:\Windows\x", "--parent", "1"]),
        ] {
            assert_eq!(parse_args(&bad), None);
        }
        assert_eq!(
            helper_args(good, 12),
            format!("--elevated-helper --pipe \"{good}\" --parent 12")
        );
    }
}
