//! Protocol between the app (no admin rights) and the `--elevated-helper` helper process.
//!
//! Message frame: 4 length bytes (little-endian) + JSON, at most [`MAX_FRAME`] (1 MiB).
//! Only the commands in [`Request`] are accepted: there is no command to run a program or read an arbitrary path
//! (to import a file, the app reads the file and sends the DATA). This part is shared by all platforms.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

use crate::model::{Store, Subscription, Update, UsageReport};

/// Maximum size of the JSON part of a frame.
pub const MAX_FRAME: usize = 1024 * 1024;

/// Commands the app sends to the helper process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]
pub enum Request {
    Set {
        store: Store,
        updates: Vec<Update>,
    },
    FixPriority,
    AddSubs(Vec<Subscription>),
    RemoveSubs(Vec<Subscription>),
    NotifyUsage(UsageReport),
    Ping,
    /// Tells the helper process to exit.
    Bye,
}

/// Result the helper process returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", content = "detail", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Err { status: i32, message: String },
}

/// Frame read/write error.
#[derive(Debug)]
pub enum FrameError {
    /// The other end closed exactly on a frame boundary (a clean end of data).
    Eof,
    /// The declared length exceeds [`MAX_FRAME`].
    TooLarge(u32),
    /// Closed midway or another I/O error (including a timeout: `ErrorKind::TimedOut`).
    Io(io::Error),
    /// The content is not valid JSON for a defined command/result.
    Json(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eof => write!(f, "kênh đã đóng"),
            Self::TooLarge(n) => write!(f, "tin nhắn quá lớn ({n} byte, tối đa {MAX_FRAME})"),
            Self::Io(e) => write!(f, "lỗi kênh: {e}"),
            Self::Json(e) => write!(f, "tin nhắn sai định dạng: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Writes one frame. Combines header + body into ONE write so it is not split on the pipe.
pub fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> Result<(), FrameError> {
    if payload.len() > MAX_FRAME {
        return Err(FrameError::TooLarge(
            payload.len().min(u32::MAX as usize) as u32
        ));
    }
    let mut buf = Vec::with_capacity(4 + payload.len());
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf).map_err(FrameError::Io)?;
    w.flush().map_err(FrameError::Io)
}

/// Reads one frame. Checks the length BEFORE allocating memory.
pub fn read_frame<R: Read>(r: &mut R) -> Result<Vec<u8>, FrameError> {
    let mut head = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut head[got..]) {
            Ok(0) if got == 0 => return Err(FrameError::Eof),
            Ok(0) => {
                return Err(FrameError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "đứt giữa đầu khung",
                )));
            }
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FrameError::Io(e)),
        }
    }
    let len = u32::from_le_bytes(head);
    if len as usize > MAX_FRAME {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).map_err(FrameError::Io)?;
    Ok(body)
}

pub fn encode_request(req: &Request) -> Vec<u8> {
    serde_json::to_vec(req).expect("Request luôn tuần tự hoá được")
}

pub fn decode_request(bytes: &[u8]) -> Result<Request, FrameError> {
    serde_json::from_slice(bytes).map_err(|e| FrameError::Json(e.to_string()))
}

pub fn encode_response(resp: &Response) -> Vec<u8> {
    serde_json::to_vec(resp).expect("Response luôn tuần tự hoá được")
}

pub fn decode_response(bytes: &[u8]) -> Result<Response, FrameError> {
    serde_json::from_slice(bytes).map_err(|e| FrameError::Json(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Priority;

    #[test]
    fn frame_roundtrip() {
        let reqs = vec![
            Request::Set {
                store: Store::Runtime,
                updates: vec![Update::reset(123, Priority::User)],
            },
            Request::FixPriority,
            Request::AddSubs(vec![Subscription {
                id: 1,
                reporting_kind: 2,
                reporting_options: 3,
                reporting_target: 4,
            }]),
            Request::NotifyUsage(UsageReport {
                id: 9,
                reporting_kind: 1,
                reporting_options: 0,
            }),
            Request::Ping,
            Request::Bye,
        ];
        let mut wire = Vec::new();
        for r in &reqs {
            write_frame(&mut wire, &encode_request(r)).unwrap();
        }
        let mut cur = io::Cursor::new(wire);
        for r in &reqs {
            let got = decode_request(&read_frame(&mut cur).unwrap()).unwrap();
            assert_eq!(&got, r);
        }
        assert!(matches!(read_frame(&mut cur), Err(FrameError::Eof)));

        let resp = Response::Err {
            status: -5,
            message: "x".into(),
        };
        let mut wire = Vec::new();
        write_frame(&mut wire, &encode_response(&resp)).unwrap();
        wire.extend_from_slice(&[0, 0]); // half a frame header left over: must report an error, not Eof
        let mut cur = io::Cursor::new(wire);
        assert_eq!(
            decode_response(&read_frame(&mut cur).unwrap()).unwrap(),
            resp
        );
        assert!(matches!(read_frame(&mut cur), Err(FrameError::Io(_))));
    }

    #[test]
    fn frame_header_is_little_endian_length() {
        let mut wire = Vec::new();
        write_frame(&mut wire, b"abc").unwrap();
        assert_eq!(wire, [3, 0, 0, 0, b'a', b'b', b'c']);
    }

    #[test]
    fn protocol_rejects_unknown_command() {
        for bad in [
            &br#"{"cmd":"run","args":"calc.exe"}"#[..],
            br#"{"cmd":"fix_lkg"}"#,
            br#"{"cmd":"exec","args":{"path":"C:/x.exe"}}"#,
            br#"{"nothing":1}"#,
        ] {
            assert!(
                matches!(decode_request(bad), Err(FrameError::Json(_))),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn protocol_rejects_malformed_json() {
        for bad in [
            &b""[..],
            b"not json",
            b"{\"cmd\":",
            b"\xff\xfe",
            b"null",
            b"[]",
        ] {
            assert!(matches!(decode_request(bad), Err(FrameError::Json(_))));
        }
        // Wrong type in a parameter.
        assert!(
            decode_request(br#"{"cmd":"set","args":{"store":"nowhere","updates":[]}}"#).is_err()
        );
    }

    /// A reader that only has the frame header: if the implementation allocated/read by the declared length it would run dry or blow up memory.
    struct HeaderOnly(Vec<u8>, usize);
    impl Read for HeaderOnly {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = (self.0.len() - self.1).min(buf.len());
            buf[..n].copy_from_slice(&self.0[self.1..self.1 + n]);
            self.1 += n;
            Ok(n)
        }
    }

    #[test]
    fn protocol_rejects_oversized_message() {
        // Declares 4 GiB - 1: must be rejected right at the frame header, without allocating.
        let mut r = HeaderOnly(u32::MAX.to_le_bytes().to_vec(), 0);
        assert!(matches!(
            read_frame(&mut r),
            Err(FrameError::TooLarge(u32::MAX))
        ));
        // 1 byte over is blocked; exactly at the limit passes the first check (then a short body = I/O error, not TooLarge).
        let mut r = HeaderOnly(((MAX_FRAME + 1) as u32).to_le_bytes().to_vec(), 0);
        assert!(matches!(read_frame(&mut r), Err(FrameError::TooLarge(_))));
        let mut r = HeaderOnly((MAX_FRAME as u32).to_le_bytes().to_vec(), 0);
        assert!(matches!(read_frame(&mut r), Err(FrameError::Io(_))));
        // The write side does not send over the limit either.
        let mut sink = Vec::new();
        assert!(matches!(
            write_frame(&mut sink, &vec![0u8; MAX_FRAME + 1]),
            Err(FrameError::TooLarge(_))
        ));
        assert!(sink.is_empty());
    }
}
