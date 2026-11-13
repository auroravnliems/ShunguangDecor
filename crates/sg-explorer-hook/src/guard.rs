//! Catches panics at every FFI boundary (hook proc, subclass proc, pipe thread).
//!
//! A panic unwinding through an `extern "system"` function = Explorer crashes. Every entry point from Windows goes through [`guarded`]:
//! panic → write a log entry (in-memory queue, no I/O) → the `on_panic` function returns a default result.
//! `on_panic` may only call simple Windows APIs (`DefSubclassProc`, `CallNextHookEx`…) and must not panic.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;

/// Runs `f`; if it panics, logs the panic and returns `on_panic(message)`.
pub fn guarded<R>(what: &str, on_panic: impl FnOnce(String) -> R, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(e) => {
            let msg = panic_message(e.as_ref());
            crate::log::push(format!("panic in {what}: {msg}"));
            on_panic(msg)
        }
    }
}

pub fn panic_message(e: &(dyn Any + Send)) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

static HOOK: Once = Once::new();

/// Replaces the default panic hook (prints to stderr) with writing to the log queue. Only affects this DLL's Rust code
/// (the standard library is statically linked into the DLL); nothing else in Explorer is touched.
pub fn install_panic_hook() {
    HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            crate::log::push(format!("panic at {at}: {}", panic_message(info.payload())));
        }));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_catches_panic_and_returns_fallback() {
        let r = guarded("test", |m| -(m.len() as i32), || -> i32 { panic!("boom") });
        assert_eq!(r, -4);
        assert_eq!(guarded("test", |_| 0, || 7), 7);
        let r = guarded("test", |m| m, || -> String { panic!("{}", String::from("dyn")) });
        assert_eq!(r, "dyn");
    }
}
