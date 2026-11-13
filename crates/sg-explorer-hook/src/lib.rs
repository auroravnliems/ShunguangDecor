//! `shunguang_explorer.dll`: the DLL loaded into `explorer.exe`.
//!
//! A "dumb" DLL: it only paints on commands the app sends over a named pipe; if the pipe is lost it resets everything to defaults on its own.
//! Currently used for the desktop label text.
//!
//! Safety (getting this wrong would break the user's desktop):
//! - Every entry point from Windows (hook proc, subclass proc, pipe thread) is wrapped in `catch_unwind` (`guard.rs`).
//! - There is no `DllMain`. No threads are created and no libraries loaded while the DLL loads.
//! - Explorer's UI thread never waits on I/O or holds a lock for long; I/O and logging happen only on the pipe thread.
//! - The subclass can be removed at any time on the desktop thread itself; the DLL stays pinned, so "off" means going back to the resting state.
//! - It never reads or writes another process's memory; it only runs inside the `explorer.exe` that owns `SHELLDLL_DefView`.
//!
//! The pure-logic modules (`state`, `session`, `layout`, `compose`, `log`, `guard`) can be tested without Explorer.

pub mod compose;
pub mod guard;
pub mod layout;
pub mod log;
pub mod own_window;
pub mod session;
pub mod state;

mod desktop;
mod labels_paint;
mod pipe;
#[cfg(test)]
mod paint_harness;

pub use desktop::sg_hook_proc;

/// Name of the hook function the DLL exports (the app gets its address with `GetProcAddress`).
pub const HOOK_PROC_EXPORT: &str = "sg_hook_proc";
