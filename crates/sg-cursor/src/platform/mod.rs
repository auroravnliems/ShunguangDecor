//! OS-dependent part: reading/writing the cursor registry, setting the cursor size and reloading.

mod windows;
pub use windows::*;
