//! YASB taskbar. Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! Pure logic (testable without the Windows shell): the friendly configuration model and the `config.yaml` + `styles.css` generator.
//! Windows part: `engine` — the bar runs on its own thread, controlled through `TaskbarHandle`.

mod engine;
pub mod fonts;
pub mod generate;
pub mod model;
pub mod module;
pub mod native;
pub mod raw;
pub mod reveal;

pub use engine::{TaskbarEvent, TaskbarHandle, validate as validate_files};
pub use generate::{Generated, generate};
pub use module::{BarEvent, Hooks, TaskbarController, TaskbarModule, TaskbarStatus, WidgetCatalogItem, widget_catalog};
pub use native::{NativePort, NativeTaskbar};
pub use raw::RawState;
pub use native::Win32Port;
pub use reveal::{HotCornerState, Rect, RevealInput, RevealState, hot_corner_step, is_shell_flyout, reveal_step};
pub use model::{BarStyle, Blur, Corner, Edge, NativeMode, TaskbarConfig, WidgetEntry, WidgetKind, WinButtonPos, Zones};
