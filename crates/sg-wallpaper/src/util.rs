// Ported from TachyonWallpaper (idea originally from Lively Wallpaper, GPL-3.0).
//! Shared utilities: UTF-16 strings, logging, short ids, working-set trimming.

/// Writes an English log line to `tracing` (target `sg::wallpaper`).
#[macro_export]
macro_rules! logln {
    ($($t:tt)*) => { tracing::info!(target: "sg::wallpaper", "{}", format!($($t)*)) };
}

/// Creates a short, practically unique id for a new wallpaper (time + counter).
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{:04x}", now, c & 0xffff)
}

mod win {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;

    /// NUL-terminated UTF-16 string, keeping the memory alive for the PCWSTR.
    pub struct WideString(Vec<u16>);

    impl WideString {
        pub fn new(s: impl AsRef<OsStr>) -> Self {
            let mut v: Vec<u16> = s.as_ref().encode_wide().collect();
            v.push(0);
            WideString(v)
        }
        pub fn pcwstr(&self) -> PCWSTR {
            PCWSTR(self.0.as_ptr())
        }
        pub fn as_slice_with_nul(&self) -> &[u16] {
            &self.0
        }
    }

    pub fn wide(s: impl AsRef<OsStr>) -> WideString {
        WideString::new(s)
    }

    /// Converts a u16 array (which may contain a NUL) to a String.
    pub fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    /// Gives memory pages that are no longer in use back to Windows. Only call after large changes.
    pub fn trim_working_set() {
        use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
        // SAFETY: the current process; the special argument `usize::MAX` means "trim as much as possible".
        unsafe {
            let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
        }
    }
}

pub use win::*;
