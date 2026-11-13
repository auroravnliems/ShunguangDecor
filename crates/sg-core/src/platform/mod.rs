//! Win32 calls. Code outside this directory does not call Win32 directly.

#[path = "windows.rs"]
mod imp;

pub use imp::*;

#[cfg(test)]
mod tests {
    #[test]
    fn show_native_taskbar_is_harmless() {
        // If the taskbar is already visible, calling again changes nothing; if the machine has no taskbar, report NotFound (so it is kept in the log).
        match super::show_native_taskbar() {
            Ok(()) => {}
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{e}"),
        }
    }
}
