//! The "Hidden features" part: a Rust port of ViVe (GPL-3.0, @thebookisclosed).
//!
//! This crate is the shared, platform-independent part: data model, feature-name dictionary,
//! undo log, and export/import format. The ntdll calls live in `platform` (Windows) and `native`.

pub mod client;
pub mod dictionary;
pub mod helper;
pub mod list;
pub mod model;
pub mod module;
pub mod native;
pub mod pipe;
pub mod platform;
pub mod protocol;
pub mod transfer;
pub mod undo;

pub use client::{ClientError, HelperClient, LaunchError};
pub use dictionary::Dictionary;
pub use list::{FeaturePage, ListQuery, SortBy};
pub use module::{Backend, Elevated, FeatureRow, FeaturesModule, FeaturesService, ImportReport, Launcher, SuggestionRow};
pub use model::{
    EnabledState, FeatureConfig, MAX_VARIANT, Operation, PayloadKind, Priority, Store,
    Subscription, Update, UsageReport,
};
pub use protocol::{Request, Response};
pub use undo::{UndoEntry, UndoJournal};
