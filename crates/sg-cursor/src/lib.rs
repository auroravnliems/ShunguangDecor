//! Mouse cursors.

pub mod ani;
pub mod backup;
pub mod config;
pub mod guess;
pub mod image;
pub mod import;
pub mod inf;
pub mod library;
pub mod module;
pub mod platform;
pub mod preview;
pub mod role;
pub mod scale;
pub mod service;
pub mod write;

#[cfg(test)]
pub(crate) mod test_util;

pub use ani::{Animation, parse_cursor_file};
pub use image::{CursorError, CursorImage, largest, parse_ico_cur};
pub use library::{Library, RoleInfo, SchemeDetail, SchemeSummary};
pub use backup::RegistrySnapshot;
pub use config::CursorConfig;
pub use module::CursorModule;
pub use role::Role;
pub use scale::{ScaleMode, image_side, scale};
pub use service::CursorService;
pub use write::{write_ani, write_cur};

/// Largest cursor file accepted (guards against deliberately malicious files).
pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
/// Maximum number of frames in an animated cursor.
pub const MAX_FRAMES: usize = 512;
/// Maximum number of images in a single `.cur`/`.ico` file.
pub const MAX_IMAGES_PER_ICON: usize = 32;
/// Maximum side length of a cursor image.
pub const MAX_IMAGE_SIDE: u32 = 256;
