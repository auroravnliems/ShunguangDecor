//! Calls ntdll the way ViVe does (GPL-3.0, @thebookisclosed).
//!
//! WARNING: the functions `set`, `fix_priority`, `add_subscriptions`, `remove_subscriptions` and
//! `notify_usage` write to the real system. Only call them after the user has confirmed in the UI.

mod windows;
pub use self::windows::*;

/// `STATUS_NOT_SUPPORTED`.
pub const STATUS_NOT_SUPPORTED: i32 = 0xC000_00BB_u32 as i32;
