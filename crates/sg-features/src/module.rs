//! Hidden features: `FeaturesService` (shared with the Tauri commands) + `FeaturesModule` (plugged into the ModuleHost).
//!
//! - Reading (no admin needed): dictionary lookup, querying existing configurations, registering watches, the undo log, file export.
//! - Writing: only through the elevated helper process (`Elevated`, the real one = `HelperClient`). On each write: ask for elevation if not held
//!   → write the OLD value (exactly the `(id, store, priority)` about to be written) to the undo log → only then send the command. Any error in
//!   `call` closes + discards the helper process (it asks for elevation again next time).
//! - This part has no background configuration; `disable`/quitting the app only closes the helper process and does NOT undo any change.
//!   Undoing is an explicit user action (`undo_all`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use sg_core::ids::ModuleId;
use sg_core::module::{Availability, Module, ModuleStatus};
use sg_core::text::I18nText;

use crate::client::{ClientError, HelperClient, LaunchError};
use crate::dictionary::Dictionary;
use crate::model::{
    EnabledState, FeatureConfig, MAX_VARIANT, Operation, PayloadKind, Priority, Store, Subscription, Update, UsageReport,
};
use crate::list::{self, Entry, FeaturePage, ListQuery};
use crate::protocol::{Request, Response};
use crate::transfer;
use crate::undo::{UndoEntry, UndoJournal};

/// Suggestions (content filled in later): `[[s]] id = …`.
const SUGGESTIONS_TOML: &str = include_str!("../data/suggestions.toml");

/// Channel to the elevated helper process (real: `HelperClient`; tests: a fake that records the order).
pub trait Elevated: Send {
    fn call(&mut self, req: Request) -> Result<Response, ClientError>;
    fn close(&mut self);
    /// Whether the helper process is still running and the channel is still usable. The helper exits by itself after 10 idle minutes.
    fn is_alive(&self) -> bool;
}

impl Elevated for HelperClient {
    fn call(&mut self, req: Request) -> Result<Response, ClientError> {
        HelperClient::call(self, req)
    }
    fn close(&mut self) {
        HelperClient::close(self)
    }
    fn is_alive(&self) -> bool {
        HelperClient::is_alive(self)
    }
}

/// How to launch the helper process. Call OFF the UI thread: `runas` blocks until the user answers UAC.
pub type Launcher = Box<dyn Fn() -> Result<Box<dyn Elevated>, LaunchError> + Send + Sync>;

/// The system-reading side (real: `platform::*`; tests: fake).
pub trait Backend: Send + Sync {
    fn supported(&self) -> bool;
    fn windows_build(&self) -> u32;
    /// The EFFECTIVE configuration of `id` (it may sit in a different priority tier).
    fn query(&self, id: u32, store: Store) -> Option<FeatureConfig>;
    /// Every configuration of the store, one priority tier at a time.
    /// An ntdll read error returns `Err(status)`: the caller must refuse to write/export, not treat it as "no configuration".
    fn query_all(&self, store: Store) -> Result<Vec<FeatureConfig>, i32>;
    fn query_subscriptions(&self) -> Vec<Subscription>;
}

pub struct RealBackend;

impl Backend for RealBackend {
    fn supported(&self) -> bool {
        crate::platform::supported()
    }
    fn windows_build(&self) -> u32 {
        windows_build()
    }
    fn query(&self, id: u32, store: Store) -> Option<FeatureConfig> {
        crate::platform::query(id, store)
    }
    fn query_all(&self, store: Store) -> Result<Vec<FeatureConfig>, i32> {
        crate::platform::query_all(store)
    }
    fn query_subscriptions(&self) -> Vec<Subscription> {
        crate::platform::query_subscriptions()
    }
}

/// The real Windows build number (`RtlGetVersion`); 0 if not Windows.
fn windows_build() -> u32 {
    use ::windows::Wdk::System::SystemServices::RtlGetVersion;
    use ::windows::Win32::System::SystemInformation::OSVERSIONINFOW;
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: the pointer is to a struct whose size is set correctly.
    let status = unsafe { RtlGetVersion(&mut info) };
    if status.is_ok() { info.dwBuildNumber } else { 0 }
}


/// One list row: name + the effective configuration in the two stores.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureRow {
    pub id: u32,
    pub name: Option<String>,
    pub runtime: Option<FeatureConfig>,
    pub boot: Option<FeatureConfig>,
    pub changed_by_app: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionRow {
    pub id: u32,
    pub name_key: String,
    pub desc_key: String,
    pub row: FeatureRow,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub applied: u32,
    pub skipped: u32,
    pub warnings: Vec<String>,
}

struct JournalState {
    /// `None` = the log could not be opened (I/O error): every write command is refused, because the old value cannot be recorded.
    journal: Option<UndoJournal>,
    /// The corrupt file was renamed to this (a new log has been opened).
    reset_note: Option<PathBuf>,
}

pub struct FeaturesService {
    backend: Box<dyn Backend>,
    launcher: Launcher,
    dict: &'static Dictionary,
    suggestions: Vec<u32>,
    journal: Mutex<JournalState>,
    /// The helper process. The lock is held for a whole write pass so writes do not interleave.
    client: Mutex<Option<Box<dyn Elevated>>>,
    /// A copy so `status()` does not have to wait for the `client` lock (held while waiting for UAC / calling the pipe).
    elevated: AtomicBool,
    /// Number of distinct features in the log (also so `status()` does not wait for a lock).
    changed: AtomicU32,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn err(key: &str) -> I18nText {
    I18nText::new(key)
}

fn detail(key: &str, d: impl std::fmt::Display) -> I18nText {
    I18nText::new(key).arg("detail", d)
}

/// Opens the log; a corrupt file (`InvalidData`) is NOT overwritten: it is renamed to `.bak-<unix>` and a new log is opened.
fn open_journal(path: &Path) -> JournalState {
    match UndoJournal::open(path) {
        Ok(j) => JournalState { journal: Some(j), reset_note: None },
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            let mut bak = path.as_os_str().to_owned();
            bak.push(format!(".bak-{}", now_unix()));
            let bak = PathBuf::from(bak);
            match std::fs::rename(path, &bak) {
                Ok(()) => {
                    tracing::warn!(
                        target: "sg::features",
                        "nhật ký hoàn tác hỏng ({e}); đã đổi tên thành {} và mở nhật ký mới",
                        bak.display()
                    );
                    match UndoJournal::open(path) {
                        Ok(j) => JournalState { journal: Some(j), reset_note: Some(bak) },
                        Err(e2) => {
                            tracing::error!(target: "sg::features", "không mở được nhật ký mới: {e2}");
                            JournalState { journal: None, reset_note: Some(bak) }
                        }
                    }
                }
                Err(e2) => {
                    tracing::error!(
                        target: "sg::features",
                        "nhật ký hoàn tác hỏng ({e}) và không đổi tên được ({e2}); tạm khoá việc ghi"
                    );
                    JournalState { journal: None, reset_note: Some(path.to_path_buf()) }
                }
            }
        }
        Err(e) => {
            tracing::error!(target: "sg::features", "không mở được nhật ký hoàn tác: {e}");
            JournalState { journal: None, reset_note: None }
        }
    }
}

fn parse_suggestions(text: &str) -> Vec<u32> {
    #[derive(serde::Deserialize, Default)]
    struct S {
        id: u32,
    }
    #[derive(serde::Deserialize, Default)]
    struct F {
        #[serde(default)]
        s: Vec<S>,
    }
    toml::from_str::<F>(text)
        .unwrap_or_default()
        .s
        .into_iter()
        .map(|s| s.id)
        .collect()
}

fn read_err(status: i32) -> I18nText {
    detail("features.err.read", format!("{status:#x}"))
}

fn distinct_ids(j: &UndoJournal) -> u32 {
    let mut ids: Vec<u32> = j.entries().iter().map(|e| e.id).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.len() as u32
}

/// Groups updates by store, keeping the first-appearance order of the stores and the order within each store.
fn group_by_store(ops: Vec<(Store, Update)>) -> Vec<(Store, Vec<Update>)> {
    let mut groups: Vec<(Store, Vec<Update>)> = Vec::new();
    for (store, u) in ops {
        match groups.iter_mut().find(|(s, _)| *s == store) {
            Some((_, v)) => v.push(u),
            None => groups.push((store, vec![u])),
        }
    }
    groups
}

impl FeaturesService {
    /// `journal_path` = `paths.backup_dir()/features-undo.toml`.
    pub fn new(journal_path: &Path, backend: Box<dyn Backend>, launcher: Launcher) -> Arc<Self> {
        let state = open_journal(journal_path);
        let changed = state.journal.as_ref().map_or(0, distinct_ids);
        Arc::new(FeaturesService {
            backend,
            launcher,
            dict: Dictionary::embedded(),
            suggestions: parse_suggestions(SUGGESTIONS_TOML),
            journal: Mutex::new(state),
            client: Mutex::new(None),
            elevated: AtomicBool::new(false),
            changed: AtomicU32::new(changed),
        })
    }

    /// The real build: reads ntdll, launches the helper process using the running exe itself.
    pub fn new_real(journal_path: &Path) -> Arc<Self> {
        let launcher: Launcher = Box::new(|| {
            let exe = std::env::current_exe()
                .map_err(|e| LaunchError::Failed(format!("không biết đường dẫn app: {e}")))?;
            HelperClient::launch(&exe).map(|c| Box::new(c) as Box<dyn Elevated>)
        });
        Self::new(journal_path, Box::new(RealBackend), launcher)
    }

    pub fn availability(&self) -> Availability {
        if !self.backend.supported() {
            return Availability::Unsupported { reason: err("features.reason.needs_2004") };
        }
        Availability::Available
    }

    pub fn is_elevated(&self) -> bool {
        // If the client is dead (the helper exited on its own), lower the flag; if it is busy (lock held), leave it as is.
        if let Ok(mut g) = self.client.try_lock()
            && g.as_ref().is_some_and(|c| !c.is_alive())
        {
            if let Some(mut c) = g.take() {
                c.close();
            }
            self.elevated.store(false, Ordering::SeqCst);
        }
        self.elevated.load(Ordering::SeqCst)
    }

    pub fn changed_count(&self) -> u32 {
        self.changed.load(Ordering::SeqCst)
    }

    /// What the corrupt log file was renamed to (if any) — so the UI can report it.
    pub fn journal_reset_note(&self) -> Option<PathBuf> {
        lock(&self.journal).reset_note.clone()
    }

    // ---- Reading ----

    fn row(&self, id: u32, changed: &[u32]) -> FeatureRow {
        FeatureRow {
            id,
            name: self.dict.name_of(id).map(str::to_owned),
            runtime: self.backend.query(id, Store::Runtime),
            boot: self.backend.query(id, Store::Boot),
            changed_by_app: changed.contains(&id),
        }
    }

    fn changed_ids(&self) -> Vec<u32> {
        lock(&self.journal)
            .journal
            .as_ref()
            .map(|j| j.entries().iter().map(|e| e.id).collect())
            .unwrap_or_default()
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<FeatureRow> {
        let changed = self.changed_ids();
        self.dict
            .search(query, limit)
            .into_iter()
            .map(|(id, _)| self.row(id, &changed))
            .collect()
    }

    /// One page of the list of all features: dictionary ∪ IDs with a configuration in Runtime/Boot. An ntdll read error is propagated (an
    /// incomplete list is never returned as if complete). Configuration is looked up only for the rows of the returned page.
    pub fn list(&self, q: &ListQuery) -> Result<FeaturePage<FeatureRow>, I18nText> {
        let mut configured = HashSet::new();
        for store in [Store::Runtime, Store::Boot] {
            configured.extend(self.backend.query_all(store).map_err(read_err)?.into_iter().map(|c| c.id));
        }
        let mut ids: HashSet<u32> = self.dict.ids().collect();
        ids.extend(configured.iter().copied());
        let entries = ids
            .into_iter()
            .map(|id| Entry { id, name: self.dict.name_of(id), configured: configured.contains(&id) })
            .collect();
        let (total, page, page_size, page_ids) = list::select_page(entries, q);
        let changed = self.changed_ids();
        Ok(FeaturePage {
            total,
            page,
            page_size,
            rows: page_ids.into_iter().map(|id| self.row(id, &changed)).collect(),
        })
    }

    /// Web search URL for an ID (the name comes from the dictionary if available).
    pub fn web_search_url(&self, id: u32) -> String {
        list::web_search_url(id, self.dict.name_of(id))
    }

    pub fn get(&self, id: u32) -> FeatureRow {
        self.row(id, &self.changed_ids())
    }

    pub fn suggestions(&self) -> Vec<SuggestionRow> {
        let changed = self.changed_ids();
        self.suggestions
            .iter()
            .map(|&id| SuggestionRow {
                id,
                name_key: format!("features.suggest.{id}.name"),
                desc_key: format!("features.suggest.{id}.desc"),
                row: self.row(id, &changed),
            })
            .collect()
    }

    pub fn subscriptions(&self) -> Vec<Subscription> {
        self.backend.query_subscriptions()
    }

    pub fn undo_list(&self) -> Vec<UndoEntry> {
        lock(&self.journal)
            .journal
            .as_ref()
            .map(|j| j.entries().to_vec())
            .unwrap_or_default()
    }

    /// Export JSON of both stores (the configurations currently present, one priority tier at a time).
    /// An ntdll read error is reported as an error; an empty file is never written.
    pub fn export_json(&self) -> Result<String, I18nText> {
        let runtime = self.backend.query_all(Store::Runtime).map_err(read_err)?;
        let boot = self.backend.query_all(Store::Boot).map_err(read_err)?;
        Ok(transfer::export(&runtime, &boot))
    }

    // ---- Admin rights ----

    /// Ensures the helper process exists and returns the guard holding it (always `Some`). The UAC dialog blocks until the user
    /// answers, so the `client` lock is NOT held while launching. `Err(Declined)` is turned into a "no permission yet" error.
    fn session(&self) -> Result<MutexGuard<'_, Option<Box<dyn Elevated>>>, I18nText> {
        {
            let mut g = lock(&self.client);
            // The helper exits by itself after 10 idle minutes: if the old client is dead, close it, discard it and reopen (UAC) BEFORE
            // any log line is written for this write pass.
            if g.as_ref().is_some_and(|c| !c.is_alive()) {
                if let Some(mut c) = g.take() {
                    c.close();
                }
                self.elevated.store(false, Ordering::SeqCst);
            }
            if g.is_some() {
                return Ok(g);
            }
        }
        let fresh = (self.launcher)().map_err(|e| match e {
            LaunchError::Declined => err("features.err.no_admin"),
            LaunchError::Failed(m) => detail("features.err.launch", m),
        })?;
        let mut g = lock(&self.client);
        if g.as_ref().is_some_and(|c| c.is_alive()) {
            // Another thread already opened one while we waited for UAC: use that one and close the one we just opened.
            let mut extra = fresh;
            extra.close();
        } else {
            if let Some(mut stale) = g.take() {
                stale.close();
            }
            *g = Some(fresh);
            self.elevated.store(true, Ordering::SeqCst);
        }
        Ok(g)
    }

    /// Requests admin rights (UAC dialog). `Ok(false)` = the user clicked "No".
    pub fn elevate(&self) -> Result<bool, I18nText> {
        match self.session() {
            Ok(_) => Ok(true),
            Err(e) if e.key == "features.err.no_admin" => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Closes the helper process (leaving the page / disabling the part). Undoes nothing.
    pub fn release(&self) {
        let mut g = lock(&self.client);
        if let Some(mut c) = g.take() {
            c.close();
        }
        self.elevated.store(false, Ordering::SeqCst);
    }

    /// Sends the `Set` groups; a `call` error closes + discards the helper process.
    fn send_sets(
        &self,
        guard: &mut MutexGuard<'_, Option<Box<dyn Elevated>>>,
        groups: Vec<(Store, Vec<Update>)>,
    ) -> Result<(), I18nText> {
        for (store, updates) in groups {
            let Some(client) = guard.as_mut() else {
                return Err(err("features.err.no_admin"));
            };
            match client.call(Request::Set { store, updates }) {
                Ok(Response::Ok) => {}
                Ok(Response::Err { status, message }) => {
                    return Err(I18nText::new("features.err.helper")
                        .arg("detail", message)
                        .arg("status", format!("{status:#x}")));
                }
                Err(e) => {
                    if let Some(mut c) = guard.take() {
                        c.close();
                    }
                    self.elevated.store(false, Ordering::SeqCst);
                    return Err(detail("features.err.channel", e));
                }
            }
        }
        Ok(())
    }

    // ---- Ghi ----

    fn check(id: u32, variant: u8, priority: Priority) -> Result<(), I18nText> {
        if id == 0 {
            return Err(err("features.err.bad_id"));
        }
        if variant > MAX_VARIANT {
            return Err(err("features.err.bad_variant"));
        }
        if priority.is_immutable() {
            return Err(I18nText::new("features.err.immutable_priority")
                .arg("priority", format!("{priority:?}")));
        }
        Ok(())
    }

    /// Writes the log (the OLD value at the exact tier) for each `(store, id, tier, new-value description)` BEFORE sending the command.
    /// The original = the configuration at the exact (id, store, tier) — not the effective configuration.
    fn record_keys(&self, keys: &[(Store, u32, Priority, String)]) -> Result<(), I18nText> {
        let mut js = lock(&self.journal);
        let Some(journal) = js.journal.as_mut() else {
            return Err(err("features.err.journal"));
        };
        // Read all needed stores BEFORE writing any log line: a read error refuses the whole pass.
        let mut cache: Vec<(Store, Vec<FeatureConfig>)> = Vec::new();
        for (store, ..) in keys {
            if !cache.iter().any(|(s, _)| s == store) {
                cache.push((*store, self.backend.query_all(*store).map_err(read_err)?));
            }
        }
        for (store, id, priority, new) in keys {
            let all = &cache.iter().find(|(s, _)| s == store).expect("vừa thêm").1;
            let original = all
                .iter()
                .find(|c| c.id == *id && c.priority == *priority)
                .copied();
            journal
                .record(UndoEntry {
                    unix: now_unix(),
                    store: *store,
                    original,
                    id: *id,
                    priority: *priority,
                })
                .map_err(|e| detail("features.err.journal_io", e))?;
            tracing::info!(
                target: "sg::features",
                "ghi id={} kho={:?} tầng={:?}: {} -> {}",
                id,
                store,
                priority,
                match &original {
                    Some(o) => format!("{:?}/{}", o.state, o.variant),
                    None => "không có cấu hình".to_owned(),
                },
                new
            );
        }
        self.changed.store(distinct_ids(journal), Ordering::SeqCst);
        Ok(())
    }

    /// Writes the log, then sends. `ops` has already been validated.
    fn write_batch(&self, ops: Vec<(Store, Update)>) -> Result<(), I18nText> {
        if ops.is_empty() {
            return Ok(());
        }
        let mut guard = self.session()?;
        let keys: Vec<_> = ops
            .iter()
            .map(|(s, u)| (*s, u.id, u.priority, format!("{:?}/{}", u.state, u.variant)))
            .collect();
        self.record_keys(&keys)?;
        self.send_sets(&mut guard, group_by_store(ops))
    }

    /// `/fixpriority`: moves `Service`-tier configurations (without Wexp) to `User`. Logs the originals of BOTH tiers
    /// (source `Service` and destination `User`) before sending. Returns the number of configurations to be moved (0 = nothing to do, no elevation requested).
    pub fn fix_priority(&self) -> Result<u32, I18nText> {
        // Preview (no lock): if nothing needs moving, skip asking for elevation. A read error is reported as an error, not as "0".
        if self.fix_priority_keys()?.is_empty() {
            return Ok(0);
        }
        let mut guard = self.session()?;
        // Recompute AFTER taking the write lock: no write pass can slip in between computing and sending.
        let keys = self.fix_priority_keys()?;
        if keys.is_empty() {
            return Ok(0);
        }
        let n = (keys.len() / 2) as u32;
        self.record_keys(&keys)?;
        tracing::info!(target: "sg::features", "sửa priority: {n} cấu hình");
        self.send_simple(&mut guard, Request::FixPriority)?;
        Ok(n)
    }

    /// Each `Service`-tier configuration without Wexp -> 2 keys: the source (Service) and the destination (User).
    fn fix_priority_keys(&self) -> Result<Vec<(Store, u32, Priority, String)>, I18nText> {
        let mut keys = Vec::new();
        for store in [Store::Runtime, Store::Boot] {
            for c in self.backend.query_all(store).map_err(read_err)? {
                if c.priority == Priority::Service && !c.wexp {
                    keys.push((store, c.id, Priority::Service, "xoá (chuyển sang User)".to_owned()));
                    keys.push((store, c.id, Priority::User, format!("{:?}/{}", c.state, c.variant)));
                }
            }
        }
        Ok(keys)
    }

    /// `RtlNotifyFeatureUsage`: does not change configuration, so nothing goes in the undo log.
    pub fn notify_usage(&self, report: UsageReport) -> Result<(), I18nText> {
        let mut guard = self.session()?;
        tracing::info!(target: "sg::features", "báo dùng id={} kind={}", report.id, report.reporting_kind);
        self.send_simple(&mut guard, Request::NotifyUsage(report))
    }

    /// Sends one command; a `call` error closes + discards the helper process.
    fn send_simple(
        &self,
        guard: &mut MutexGuard<'_, Option<Box<dyn Elevated>>>,
        req: Request,
    ) -> Result<(), I18nText> {
        let Some(client) = guard.as_mut() else {
            return Err(err("features.err.no_admin"));
        };
        match client.call(req) {
            Ok(Response::Ok) => Ok(()),
            Ok(Response::Err { status, message }) => Err(I18nText::new("features.err.helper")
                .arg("detail", message)
                .arg("status", format!("{status:#x}"))),
            Err(e) => {
                if let Some(mut c) = guard.take() {
                    c.close();
                }
                self.elevated.store(false, Ordering::SeqCst);
                Err(detail("features.err.channel", e))
            }
        }
    }

    pub fn set(
        &self,
        id: u32,
        store: Store,
        state: EnabledState,
        variant: u8,
        priority: Priority,
    ) -> Result<(), I18nText> {
        Self::check(id, variant, priority)?;
        let cfg = FeatureConfig {
            id,
            priority,
            state,
            variant,
            variant_payload_kind: PayloadKind::None,
            variant_payload: 0,
            wexp: false,
        };
        self.write_batch(vec![(store, Update::set_from(&cfg))])
    }

    /// Deletes the configuration of `id` at exactly tier `priority` (the tier selected in the detail pane). Does NOT delete every tier like
    /// `ViVeTool /reset`: doing so would also delete `Service`-tier configurations set by Windows Update. The original at the exact
    /// (store, id, tier) is written to the log before sending, like `set`.
    pub fn reset(&self, id: u32, store: Store, priority: Priority) -> Result<(), I18nText> {
        Self::check(id, 0, priority)?;
        self.write_batch(vec![(store, Update::reset(id, priority))])
    }

    /// Import from file contents (the app reads the file and sends the DATA). Invalid rows or immutable priority tiers are skipped with a warning.
    pub fn import_text(&self, text: &str) -> Result<ImportReport, I18nText> {
        let (rows, mut warnings) = transfer::parse_import(text);
        let mut skipped = warnings.len() as u32;
        let mut ops = Vec::new();
        for (store, cfg) in rows {
            if cfg.priority.is_immutable() {
                skipped += 1;
                warnings.push(format!(
                    "ID {}: bỏ qua, mức ưu tiên {:?} không được phép đổi.",
                    cfg.id, cfg.priority
                ));
                continue;
            }
            ops.push((store, Update::set_from(&cfg)));
        }
        let applied = ops.len() as u32;
        self.write_batch(ops)?;
        Ok(ImportReport { applied, skipped, warnings })
    }

    /// Undo all: back to the exact values before the first change, in reverse order, grouped by store. The log is only cleared
    /// after every command succeeds. Returns the number of configurations undone.
    pub fn undo_all(&self) -> Result<u32, I18nText> {
        // Preview (no lock): if the log is empty, skip asking for elevation.
        if self.plan_undo()?.is_empty() {
            return Ok(0);
        }
        // Hold the `client` lock for the whole pass (plan -> send -> clear): another write pass cannot slip in between,
        // so `clear()` cannot delete a line from a write pass that has not been undone.
        let mut guard = self.session()?;
        let plan = self.plan_undo()?;
        if plan.is_empty() {
            return Ok(0);
        }
        let n = plan.len() as u32;
        tracing::info!(target: "sg::features", "hoàn tác tất cả: {n} cấu hình");
        for (store, u) in &plan {
            let to = if u.operation.contains(Operation::RESET_STATE) {
                "xoá cấu hình".to_owned()
            } else {
                format!("{:?}/{}", u.state, u.variant)
            };
            tracing::info!(target: "sg::features", "hoàn tác id={} kho={:?} tầng={:?}: -> {}", u.id, store, u.priority, to);
        }
        self.send_sets(&mut guard, group_by_store(plan))?;
        let mut js = lock(&self.journal);
        if let Some(j) = js.journal.as_mut() {
            j.clear().map_err(|e| detail("features.err.journal_io", e))?;
        }
        self.changed.store(0, Ordering::SeqCst);
        Ok(n)
    }

    fn plan_undo(&self) -> Result<Vec<(Store, Update)>, I18nText> {
        let js = lock(&self.journal);
        match js.journal.as_ref() {
            Some(j) => Ok(j.plan_undo_all()),
            None => Err(err("features.err.journal")),
        }
    }
}

pub struct FeaturesModule {
    service: Arc<FeaturesService>,
}

impl FeaturesModule {
    pub fn new(service: Arc<FeaturesService>) -> FeaturesModule {
        FeaturesModule { service }
    }
}

impl Module for FeaturesModule {
    fn id(&self) -> ModuleId {
        ModuleId::Features
    }

    fn availability(&self) -> Availability {
        self.service.availability()
    }

    fn enable(&mut self, _cfg: &toml::Table) -> Result<(), I18nText> {
        match self.service.availability() {
            Availability::Unsupported { reason } => Err(reason),
            _ => Ok(()),
        }
    }

    fn apply(&mut self, _cfg: &toml::Table) -> Result<(), I18nText> {
        Ok(())
    }

    /// Only closes the helper process; changes already made are KEPT (the UI asks the user whether to undo).
    fn disable(&mut self) -> Result<(), I18nText> {
        self.service.release();
        Ok(())
    }

    /// Does not undo on its own: this has to be done on a dedicated page (the "Undo all" button).
    fn restore_os_default(&mut self) -> Result<(), I18nText> {
        Ok(())
    }

    fn on_app_exit(&mut self) -> Result<(), I18nText> {
        self.service.release();
        Ok(())
    }

    fn status(&self) -> ModuleStatus {
        let yes = self.service.is_elevated();
        let details = vec![
            (
                I18nText::new("features.status.changed"),
                I18nText::new("features.val.n_features").arg("n", self.service.changed_count()),
            ),
            (
                I18nText::new("features.status.admin"),
                I18nText::new(if yes { "features.val.yes" } else { "features.val.not_yet" }),
            ),
            (
                I18nText::new("features.status.windows"),
                I18nText::new("features.val.build").arg("n", self.service.backend.windows_build()),
            ),
        ];
        let summary = self.service.journal_reset_note().map(|p| {
            I18nText::new("features.warn.journal_reset").arg("path", p.display())
        });
        ModuleStatus { summary, details }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Log {
        events: Vec<String>,
        launches: u32,
        closes: u32,
        /// Results for the next `call`s (once exhausted, `Ok(Response::Ok)`).
        script: VecDeque<Result<Response, ClientError>>,
        decline: bool,
        /// Fake clients opened from launch 1 through launch `dead_upto` are dead (the helper process exited on its own).
        dead_upto: u32,
        /// Runs once, right inside the next `call` (simulates another write pass slipping in).
        hook: Option<Box<dyn FnOnce() -> std::thread::JoinHandle<()> + Send>>,
        threads: Vec<std::thread::JoinHandle<()>>,
    }

    struct FakeElevated {
        log: Arc<Mutex<Log>>,
        journal_path: PathBuf,
        /// Which launch this is (1, 2, ...).
        nth: u32,
    }

    impl Elevated for FakeElevated {
        fn call(&mut self, req: Request) -> Result<Response, ClientError> {
            // Record the number of log lines AT THE TIME of the call: proves the log hit disk before the write command.
            let on_disk = std::fs::read_to_string(&self.journal_path)
                .map(|t| t.matches("[[entry]]").count())
                .unwrap_or(0);
            let mut l = self.log.lock().unwrap();
            let tag = match &req {
                Request::Set { store, updates } => {
                    format!(
                        "set {:?} ids={:?} journal={on_disk}",
                        store,
                        updates.iter().map(|u| u.id).collect::<Vec<_>>()
                    )
                }
                other => format!("other {} journal={on_disk}", format!("{other:?}").split('(').next().unwrap_or("").split(' ').next().unwrap_or("")),
            };
            l.events.push(tag);
            if let Some(h) = l.hook.take() {
                let th = h();
                l.threads.push(th);
            }
            l.script.pop_front().unwrap_or(Ok(Response::Ok))
        }
        fn close(&mut self) {
            self.log.lock().unwrap().closes += 1;
        }
        fn is_alive(&self) -> bool {
            self.nth > self.log.lock().unwrap().dead_upto
        }
    }

    #[derive(Default)]
    struct FakeBackend {
        runtime: Vec<FeatureConfig>,
        boot: Vec<FeatureConfig>,
        /// `true`: `query_all` reports an ntdll read error.
        fail: bool,
    }

    impl Backend for FakeBackend {
        fn supported(&self) -> bool {
            true
        }
        fn windows_build(&self) -> u32 {
            26200
        }
        fn query(&self, id: u32, store: Store) -> Option<FeatureConfig> {
            // Effective: take the entry with the highest priority.
            self.query_all(store)
                .unwrap_or_default()
                .into_iter()
                .filter(|c| c.id == id)
                .max_by_key(|c| c.priority.as_u32())
        }
        fn query_all(&self, store: Store) -> Result<Vec<FeatureConfig>, i32> {
            if self.fail {
                return Err(-1);
            }
            Ok(match store {
                Store::Runtime => self.runtime.clone(),
                Store::Boot => self.boot.clone(),
            })
        }
        fn query_subscriptions(&self) -> Vec<Subscription> {
            Vec::new()
        }
    }

    fn cfg(id: u32, priority: Priority, state: EnabledState, variant: u8) -> FeatureConfig {
        FeatureConfig {
            id,
            priority,
            state,
            variant,
            variant_payload_kind: PayloadKind::None,
            wexp: false,
            variant_payload: 0,
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        path: PathBuf,
        log: Arc<Mutex<Log>>,
        svc: Arc<FeaturesService>,
    }

    fn fixture_with(backend: FakeBackend, prepare: impl FnOnce(&Path)) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backup").join("features-undo.toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        prepare(&path);
        let log = Arc::new(Mutex::new(Log::default()));
        let (l2, p2) = (log.clone(), path.clone());
        let launcher: Launcher = Box::new(move || {
            let mut l = l2.lock().unwrap();
            l.launches += 1;
            if l.decline {
                return Err(LaunchError::Declined);
            }
            Ok(Box::new(FakeElevated { log: l2.clone(), journal_path: p2.clone(), nth: l.launches }) as Box<dyn Elevated>)
        });
        let svc = FeaturesService::new(&path, Box::new(backend), launcher);
        Fixture { _dir: dir, path, log, svc }
    }

    fn fixture(backend: FakeBackend) -> Fixture {
        fixture_with(backend, |_| {})
    }

    #[test]
    fn set_records_undo_before_write() {
        let f = fixture(FakeBackend {
            runtime: vec![cfg(100, Priority::User, EnabledState::Disabled, 3)],
            ..Default::default()
        });
        f.svc.set(100, Store::Runtime, EnabledState::Enabled, 5, Priority::User).unwrap();
        let l = f.log.lock().unwrap();
        // By the time Set is called, the log on disk already has exactly 1 line.
        assert_eq!(l.events, vec!["set Runtime ids=[100] journal=1".to_string()]);
        drop(l);
        let entries = f.svc.undo_list();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].original, Some(cfg(100, Priority::User, EnabledState::Disabled, 3)));
        assert_eq!(f.svc.changed_count(), 1);
    }

    #[test]
    fn dead_helper_is_relaunched_before_journaling() {
        let f = fixture(FakeBackend {
            runtime: vec![cfg(100, Priority::User, EnabledState::Disabled, 3)],
            ..Default::default()
        });
        f.svc.set(100, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
        assert_eq!(f.log.lock().unwrap().launches, 1);
        // The helper process exits by itself after 10 idle minutes.
        f.log.lock().unwrap().dead_upto = 1;
        f.svc.set(101, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
        let l = f.log.lock().unwrap();
        assert_eq!(l.launches, 2, "phải mở lại tiến trình phụ");
        assert_eq!(l.closes, 1, "client chết phải được đóng");
        // The second command runs on the new client and sees exactly 2 log lines (no extra line).
        assert_eq!(
            l.events,
            vec![
                "set Runtime ids=[100] journal=1".to_string(),
                "set Runtime ids=[101] journal=2".to_string()
            ]
        );
        drop(l);
        assert_eq!(f.svc.undo_list().len(), 2);
        assert!(f.svc.is_elevated());
    }

    #[test]
    fn dead_helper_and_declined_relaunch_leaves_no_trace() {
        let f = fixture(FakeBackend::default());
        f.svc.set(100, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
        {
            let mut l = f.log.lock().unwrap();
            l.dead_upto = 1;
            l.decline = true;
        }
        let e = f.svc.set(101, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap_err();
        assert_eq!(e.key, "features.err.no_admin");
        assert_eq!(f.svc.undo_list().len(), 1, "không được ghi dòng nhật ký cho lượt thất bại");
        assert_eq!(f.svc.changed_count(), 1);
        assert!(!f.svc.is_elevated());
        assert_eq!(f.log.lock().unwrap().events.len(), 1, "không gửi lệnh nào");
    }

    #[test]
    fn is_elevated_drops_flag_when_helper_died() {
        let f = fixture(FakeBackend::default());
        f.svc.set(100, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
        assert!(f.svc.is_elevated());
        f.log.lock().unwrap().dead_upto = 1;
        assert!(!f.svc.is_elevated());
    }

    #[test]
    fn reset_uses_given_priority_and_records_that_tier() {
        let f = fixture(FakeBackend {
            runtime: vec![
                cfg(9, Priority::Service, EnabledState::Enabled, 2),
                cfg(9, Priority::User, EnabledState::Disabled, 0),
            ],
            ..Default::default()
        });
        f.svc.reset(9, Store::Runtime, Priority::Service).unwrap();
        let e = f.svc.undo_list();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].priority, Priority::Service);
        assert_eq!(e[0].original, Some(cfg(9, Priority::Service, EnabledState::Enabled, 2)));
        // The log hits disk before sending.
        assert_eq!(f.log.lock().unwrap().events, vec!["set Runtime ids=[9] journal=1".to_string()]);
        // Undo restores the exact Service-tier configuration.
        let plan = f.svc.plan_undo().unwrap();
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].1.priority, Priority::Service);
        assert!(!plan[0].1.operation.contains(Operation::RESET_STATE));
        assert_eq!(plan[0].1.state, EnabledState::Enabled);
    }

    #[test]
    fn reset_rejects_immutable_priority() {
        let f = fixture(FakeBackend::default());
        let e = f.svc.reset(9, Store::Runtime, Priority::ImageDefault).unwrap_err();
        assert_eq!(e.key, "features.err.immutable_priority");
        assert!(f.svc.undo_list().is_empty());
        assert_eq!(f.log.lock().unwrap().launches, 0);
    }

    #[test]
    fn original_is_taken_from_same_priority_layer() {
        // The effective configuration (higher priority) differs from the User-tier configuration: the original must be the User tier / None.
        let f = fixture(FakeBackend {
            runtime: vec![
                cfg(7, Priority::User, EnabledState::Enabled, 1),
                cfg(7, Priority::ImageOverride, EnabledState::Disabled, 9),
                cfg(8, Priority::ImageDefault, EnabledState::Enabled, 0),
            ],
            ..Default::default()
        });
        f.svc.set(7, Store::Runtime, EnabledState::Disabled, 0, Priority::User).unwrap();
        // id 8 exists only in the ImageDefault tier; writing at the User tier makes the original "none".
        f.svc.set(8, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
        let e = f.svc.undo_list();
        assert_eq!(e[0].original, Some(cfg(7, Priority::User, EnabledState::Enabled, 1)));
        assert_eq!(e[1].original, None);
    }

    #[test]
    fn restore_default_does_not_undo() {
        let f = fixture(FakeBackend::default());
        f.svc.set(5, Store::Boot, EnabledState::Enabled, 0, Priority::User).unwrap();
        let mut m = FeaturesModule::new(f.svc.clone());
        m.restore_os_default().unwrap();
        m.enable(&toml::Table::new()).unwrap();
        m.apply(&toml::Table::new()).unwrap();
        let l = f.log.lock().unwrap();
        assert_eq!(l.events.len(), 1, "restore_os_default không được gửi lệnh nào");
        drop(l);
        assert_eq!(f.svc.undo_list().len(), 1, "nhật ký giữ nguyên");
        assert_eq!(f.svc.changed_count(), 1);
    }

    #[test]
    fn disable_and_exit_close_helper_but_keep_changes() {
        let f = fixture(FakeBackend::default());
        f.svc.set(5, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
        assert!(f.svc.is_elevated());
        let mut m = FeaturesModule::new(f.svc.clone());
        m.disable().unwrap();
        assert!(!f.svc.is_elevated());
        assert_eq!(f.log.lock().unwrap().closes, 1);
        assert_eq!(f.svc.undo_list().len(), 1);
        assert_eq!(f.log.lock().unwrap().events.len(), 1);
        m.on_app_exit().unwrap();
        assert_eq!(f.log.lock().unwrap().closes, 1, "không có gì để đóng nữa");
    }

    #[test]
    fn undo_all_groups_by_store_in_reverse_and_clears_after_success() {
        let f = fixture(FakeBackend::default());
        f.svc.set(1, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
        f.svc.set(2, Store::Boot, EnabledState::Enabled, 0, Priority::User).unwrap();
        f.svc.set(3, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
        f.svc.set(1, Store::Runtime, EnabledState::Disabled, 0, Priority::User).unwrap();
        f.log.lock().unwrap().events.clear();

        let n = f.svc.undo_all().unwrap();
        assert_eq!(n, 3);
        let l = f.log.lock().unwrap();
        // Reverse of the original write order: (3,Runtime) (2,Boot) (1,Runtime) -> Runtime group [3,1], Boot group [2].
        assert_eq!(
            l.events,
            vec![
                "set Runtime ids=[3, 1] journal=4".to_string(),
                "set Boot ids=[2] journal=4".to_string()
            ]
        );
        drop(l);
        assert!(f.svc.undo_list().is_empty());
        assert_eq!(f.svc.changed_count(), 0);
        assert!(!f.path.exists());
    }

    #[test]
    fn undo_all_keeps_journal_when_a_call_fails() {
        let f = fixture(FakeBackend::default());
        f.svc.set(1, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
        f.svc.set(2, Store::Boot, EnabledState::Enabled, 0, Priority::User).unwrap();
        // The Runtime group is ok; the Boot group gets an error from the helper.
        f.log.lock().unwrap().script.extend([
            Ok(Response::Ok),
            Ok(Response::Err { status: -1, message: "hỏng".into() }),
        ]);
        let e = f.svc.undo_all().unwrap_err();
        assert_eq!(e.key, "features.err.helper");
        assert_eq!(f.svc.undo_list().len(), 2, "chưa xoá nhật ký");
        assert!(f.path.exists());
    }

    #[test]
    fn immutable_priority_rejected() {
        let f = fixture(FakeBackend::default());
        for p in [Priority::Security, Priority::ImageOverride, Priority::Service] {
            if !p.is_immutable() {
                continue;
            }
            let e = f.svc.set(9, Store::Runtime, EnabledState::Enabled, 0, p).unwrap_err();
            assert_eq!(e.key, "features.err.immutable_priority");
        }
        assert!(Priority::Security.is_immutable());
        // No elevation request, nothing sent, nothing logged.
        let l = f.log.lock().unwrap();
        assert_eq!(l.launches, 0);
        assert!(l.events.is_empty());
        drop(l);
        assert!(f.svc.undo_list().is_empty());
        // Import: the immutable row is skipped with a warning; the valid row is still applied.
        let text = r#"{"format":1,"runtime":[
            {"id":5,"priority":"security","state":"enabled"},
            {"id":6,"priority":"user","state":"enabled"}]}"#;
        let r = f.svc.import_text(text).unwrap();
        assert_eq!((r.applied, r.skipped), (1, 1));
        assert_eq!(f.svc.undo_list().len(), 1);
        assert_eq!(f.svc.undo_list()[0].id, 6);
    }

    #[test]
    fn import_with_nothing_valid_does_not_ask_for_admin() {
        let f = fixture(FakeBackend::default());
        let r = f.svc.import_text("không phải json").unwrap();
        assert_eq!((r.applied, r.skipped), (0, 1));
        assert_eq!(f.log.lock().unwrap().launches, 0);
    }

    #[test]
    fn broken_journal_is_renamed_and_fresh_one_opened() {
        let f = fixture_with(FakeBackend::default(), |p| {
            std::fs::write(p, "[[entry]]\nid = \"abc\"").unwrap();
        });
        // The corrupt file is intact under the name .bak-<unix>; the new log is empty; status reports it.
        let dir = f.path.parent().unwrap();
        let baks: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".bak-"))
            .collect();
        assert_eq!(baks.len(), 1);
        assert_eq!(std::fs::read_to_string(baks[0].path()).unwrap(), "[[entry]]\nid = \"abc\"");
        assert!(f.svc.undo_list().is_empty());
        assert!(f.svc.journal_reset_note().is_some());
        let m = FeaturesModule::new(f.svc.clone());
        assert_eq!(m.status().summary.unwrap().key, "features.warn.journal_reset");
        // The new log is usable.
        f.svc.set(1, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
        assert_eq!(f.svc.undo_list().len(), 1);
    }

    #[test]
    fn call_error_drops_client_and_next_write_asks_again() {
        let f = fixture(FakeBackend::default());
        f.log.lock().unwrap().script.push_back(Err(ClientError::Timeout));
        let e = f.svc.set(1, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap_err();
        assert_eq!(e.key, "features.err.channel");
        {
            let l = f.log.lock().unwrap();
            assert_eq!(l.closes, 1, "đã close() client lỗi");
            assert_eq!(l.launches, 1);
        }
        assert!(!f.svc.is_elevated());
        f.svc.set(1, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
        assert_eq!(f.log.lock().unwrap().launches, 2, "lần sau xin quyền lại");
        assert!(f.svc.is_elevated());
    }

    #[test]
    fn declined_uac_reports_false_and_writes_nothing() {
        let f = fixture(FakeBackend::default());
        f.log.lock().unwrap().decline = true;
        assert!(!f.svc.elevate().unwrap());
        let e = f.svc.set(1, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap_err();
        assert_eq!(e.key, "features.err.no_admin");
        assert!(f.svc.undo_list().is_empty(), "không ghi nhật ký khi chưa có quyền");
        f.log.lock().unwrap().decline = false;
        assert!(f.svc.elevate().unwrap());
        assert!(f.svc.elevate().unwrap());
        assert_eq!(f.log.lock().unwrap().launches, 3, "bấm lại 'Cấp quyền' được; đã có quyền thì không mở lại");
    }

    #[test]
    fn invalid_arguments_rejected() {
        let f = fixture(FakeBackend::default());
        assert_eq!(f.svc.set(0, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap_err().key, "features.err.bad_id");
        assert_eq!(f.svc.set(1, Store::Runtime, EnabledState::Enabled, 64, Priority::User).unwrap_err().key, "features.err.bad_variant");
        assert_eq!(f.log.lock().unwrap().launches, 0);
    }

    #[test]
    fn reset_records_original_and_sends_reset() {
        let f = fixture(FakeBackend {
            boot: vec![cfg(4, Priority::User, EnabledState::Enabled, 2)],
            ..Default::default()
        });
        f.svc.reset(4, Store::Boot, Priority::User).unwrap();
        assert_eq!(f.log.lock().unwrap().events, vec!["set Boot ids=[4] journal=1".to_string()]);
        assert_eq!(f.svc.undo_list()[0].original, Some(cfg(4, Priority::User, EnabledState::Enabled, 2)));
    }

    #[test]
    fn rows_search_and_status() {
        let f = fixture(FakeBackend {
            runtime: vec![cfg(100, Priority::User, EnabledState::Enabled, 0)],
            ..Default::default()
        });
        let id = Dictionary::embedded().search("a", 1)[0].0;
        let rows = f.svc.search("a", 3);
        assert_eq!(rows[0].id, id);
        assert!(rows[0].name.is_some());
        assert!(!rows[0].changed_by_app);
        let r = f.svc.get(100);
        assert_eq!(r.runtime.unwrap().state, EnabledState::Enabled);
        assert!(r.boot.is_none());
        f.svc.set(100, Store::Runtime, EnabledState::Disabled, 0, Priority::User).unwrap();
        assert!(f.svc.get(100).changed_by_app);
        let st = FeaturesModule::new(f.svc.clone()).status();
        assert_eq!(st.details.len(), 3);
        assert_eq!(st.details[0].1.args["n"], "1");
        assert_eq!(st.details[1].1.key, "features.val.yes");
        assert_eq!(st.details[2].1.args["n"], "26200");
        assert!(st.summary.is_none());
        let j = f.svc.export_json();
        assert!(j.unwrap().contains("\"format\""));
    }

    fn list_q() -> ListQuery {
        ListQuery {
            page: 0,
            page_size: 50,
            configured_only: false,
            hide_unnamed: false,
            text: String::new(),
            sort: crate::list::SortBy::Id,
        }
    }

    #[test]
    fn list_merges_dictionary_and_configured_ids_without_duplicates() {
        let dict = Dictionary::embedded();
        let named = dict.ids().min().unwrap();
        // 4_000_000_000 exists only in configuration (not in the dictionary); `named` is in both stores + the dictionary.
        let only_cfg = 4_000_000_000u32;
        assert!(dict.name_of(only_cfg).is_none());
        let f = fixture(FakeBackend {
            runtime: vec![
                cfg(named, Priority::User, EnabledState::Enabled, 0),
                cfg(only_cfg, Priority::User, EnabledState::Disabled, 0),
            ],
            boot: vec![cfg(named, Priority::User, EnabledState::Disabled, 0)],
            ..Default::default()
        });
        let all = f.svc.list(&list_q()).unwrap();
        assert_eq!(all.total as usize, dict.ids().count() + 1);
        assert_eq!(all.rows.len(), 50);

        let mut q = list_q();
        q.configured_only = true;
        let cfgd = f.svc.list(&q).unwrap();
        assert_eq!(cfgd.total, 2);
        assert_eq!(cfgd.rows[0].id, named);
        assert_eq!(cfgd.rows[0].name.as_deref(), dict.name_of(named));
        assert!(cfgd.rows[0].runtime.is_some() && cfgd.rows[0].boot.is_some());
        assert_eq!(cfgd.rows[1].id, only_cfg);
        assert!(cfgd.rows[1].name.is_none());
        assert!(cfgd.rows[1].boot.is_none());

        q.hide_unnamed = true;
        assert_eq!(f.svc.list(&q).unwrap().total, 1);
    }

    #[test]
    fn list_marks_changed_by_app_and_reports_real_page() {
        let f = fixture(FakeBackend {
            runtime: vec![cfg(100, Priority::User, EnabledState::Enabled, 0)],
            ..Default::default()
        });
        f.svc.set(100, Store::Runtime, EnabledState::Disabled, 0, Priority::User).unwrap();
        let mut q = list_q();
        q.configured_only = true;
        q.page = 7;
        let p = f.svc.list(&q).unwrap();
        assert_eq!((p.total, p.page, p.page_size), (1, 0, 50));
        assert!(p.rows[0].changed_by_app);
        q.text = "no-such-feature-xyz".into();
        let empty = f.svc.list(&q).unwrap();
        assert_eq!((empty.total, empty.page), (0, 0));
        assert!(empty.rows.is_empty());
    }

    #[test]
    fn list_propagates_read_error() {
        let f = fixture(FakeBackend { fail: true, ..Default::default() });
        let e = f.svc.list(&list_q()).unwrap_err();
        assert_eq!(e.key, "features.err.read");
    }

    #[test]
    fn web_search_url_uses_dictionary_name_when_known() {
        let f = fixture(FakeBackend::default());
        let (id, name) = Dictionary::embedded().search("cameraqi2505", 1)[0];
        let u = f.svc.web_search_url(id);
        assert!(u.starts_with("https://www.google.com/search?q=%22"));
        assert!(u.contains(&format!("%22{id}%22%20OR%20%22{name}%22")));
        assert!(f.svc.web_search_url(4_000_000_000).ends_with("%224000000000%22%20ViVeTool"));
    }

    #[test]
    fn fix_priority_records_source_and_destination_before_call() {
        let f = fixture(FakeBackend {
            runtime: vec![
                cfg(10, Priority::Service, EnabledState::Enabled, 2),
                cfg(10, Priority::User, EnabledState::Disabled, 7),
                cfg(11, Priority::User, EnabledState::Enabled, 0),
            ],
            boot: vec![cfg(20, Priority::Service, EnabledState::Disabled, 0)],
            fail: false,
        });
        assert_eq!(f.svc.fix_priority().unwrap(), 2);
        // When FixPriority is received, the log on disk already has all 4 lines (2 configurations x source + destination).
        assert_eq!(f.log.lock().unwrap().events, vec!["other FixPriority journal=4".to_string()]);
        let e = f.svc.undo_list();
        assert_eq!(e.len(), 4);
        assert_eq!((e[0].id, e[0].store, e[0].priority), (10, Store::Runtime, Priority::Service));
        assert_eq!(e[0].original, Some(cfg(10, Priority::Service, EnabledState::Enabled, 2)));
        assert_eq!((e[1].id, e[1].priority), (10, Priority::User));
        assert_eq!(e[1].original, Some(cfg(10, Priority::User, EnabledState::Disabled, 7)));
        assert_eq!((e[2].id, e[2].store, e[2].priority), (20, Store::Boot, Priority::Service));
        assert_eq!((e[3].id, e[3].priority, e[3].original), (20, Priority::User, None));
        // Undo: 20's User entry is deleted, 20's Service entry is restored, then the same for 10.
        let plan = f.svc.undo_list().len();
        assert_eq!(plan, 4);
    }

    #[test]
    fn fix_priority_with_nothing_to_fix_does_not_ask_for_admin() {
        let f = fixture(FakeBackend::default());
        assert_eq!(f.svc.fix_priority().unwrap(), 0);
        assert_eq!(f.log.lock().unwrap().launches, 0);
        assert!(f.svc.undo_list().is_empty());
    }

    #[test]
    fn fix_priority_call_error_drops_client() {
        let f = fixture(FakeBackend {
            runtime: vec![cfg(10, Priority::Service, EnabledState::Enabled, 0)],
            ..Default::default()
        });
        f.log.lock().unwrap().script.push_back(Err(ClientError::Closed));
        assert_eq!(f.svc.fix_priority().unwrap_err().key, "features.err.channel");
        assert_eq!(f.log.lock().unwrap().closes, 1);
        assert!(!f.svc.is_elevated());
    }

    #[test]
    fn notify_usage_sends_without_undo_entry() {
        let f = fixture(FakeBackend::default());
        let r = UsageReport { id: 5, reporting_kind: 1, reporting_options: 0 };
        f.svc.notify_usage(r).unwrap();
        let l = f.log.lock().unwrap();
        assert_eq!(l.events.len(), 1);
        assert!(l.events[0].starts_with("other NotifyUsage"));
        drop(l);
        assert!(f.svc.undo_list().is_empty());
        assert_eq!(f.svc.changed_count(), 0);
    }

    #[test]
    fn write_during_undo_all_survives_the_clear() {
        let f = fixture(FakeBackend::default());
        f.svc.set(1, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
        // While the undo command is being sent, another write pass (another thread) asks to write id 2.
        let svc = f.svc.clone();
        f.log.lock().unwrap().hook = Some(Box::new(move || {
            std::thread::spawn(move || {
                svc.set(2, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap();
            })
        }));
        assert_eq!(f.svc.undo_all().unwrap(), 1);
        let threads = std::mem::take(&mut f.log.lock().unwrap().threads);
        for th in threads {
            th.join().unwrap();
        }
        // The interleaving write pass waits for the lock and runs AFTER the log was cleared: its line stays intact for a later undo.
        let e = f.svc.undo_list();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].id, 2);
        assert_eq!(f.svc.changed_count(), 1);
    }

    #[test]
    fn read_error_refuses_write_export_and_fix_priority() {
        let f = fixture(FakeBackend { fail: true, ..Default::default() });
        assert_eq!(f.svc.set(1, Store::Runtime, EnabledState::Enabled, 0, Priority::User).unwrap_err().key, "features.err.read");
        assert_eq!(f.svc.reset(1, Store::Boot, Priority::User).unwrap_err().key, "features.err.read");
        assert_eq!(f.svc.export_json().unwrap_err().key, "features.err.read");
        assert_eq!(f.svc.fix_priority().unwrap_err().key, "features.err.read");
        assert!(f.svc.undo_list().is_empty(), "không ghi nhật ký khi đọc lỗi");
        assert!(f.log.lock().unwrap().events.is_empty(), "không gửi lệnh nào");
    }

    #[test]
    fn wexp_is_kept_by_import_export_and_undo() {
        let mut w = cfg(30, Priority::User, EnabledState::Enabled, 1);
        w.wexp = true;
        let f = fixture(FakeBackend { runtime: vec![w], ..Default::default() });
        // Export keeps the flag; an old file without "wexp" -> false.
        let json = f.svc.export_json().unwrap();
        assert!(json.contains("\"wexp\": true"));
        let old = r#"{"format":1,"runtime":[{"id":5,"priority":"user","state":"enabled"}]}"#;
        let (rows, _) = transfer::parse_import(old);
        assert!(!rows[0].1.wexp);
        // Import keeps the flag: Update carries state_options = 1.
        let text = r#"{"format":1,"runtime":[{"id":6,"priority":"user","state":"enabled","wexp":true}]}"#;
        let (rows, _) = transfer::parse_import(text);
        assert!(rows[0].1.wexp);
        assert_eq!(Update::set_from(&rows[0].1).state_options, 1);
        // Undo of an original with wexp -> written back with the flag.
        f.svc.set(30, Store::Runtime, EnabledState::Disabled, 0, Priority::User).unwrap();
        assert!(f.svc.undo_list()[0].original.unwrap().wexp);
        let plan = {
            let js = lock(&f.svc.journal);
            js.journal.as_ref().unwrap().plan_undo_all()
        };
        assert_eq!(plan[0].1.state_options, 1);
    }

    #[test]
    fn fix_priority_skips_wexp_entries() {
        let mut w = cfg(10, Priority::Service, EnabledState::Enabled, 0);
        w.wexp = true;
        let f = fixture(FakeBackend { runtime: vec![w], ..Default::default() });
        assert_eq!(f.svc.fix_priority().unwrap(), 0);
        assert_eq!(f.log.lock().unwrap().launches, 0);
    }

    #[test]
    fn suggestions_exist_in_dictionary() {
        #[derive(serde::Deserialize)]
        struct S {
            id: u32,
            name: String,
            source: String,
        }
        #[derive(serde::Deserialize)]
        struct F {
            s: Vec<S>,
        }
        let f: F = toml::from_str(SUGGESTIONS_TOML).expect("suggestions.toml phải đúng định dạng");
        // Threshold 3..=15 (not 10..=15): only three entries passed all 3 verification conditions (see data/suggestions.toml).
        assert!(
            (3..=15).contains(&f.s.len()),
            "số gợi ý phải trong 3..=15, được {}",
            f.s.len()
        );
        let dict = Dictionary::embedded();
        let mut seen = std::collections::HashSet::new();
        for s in &f.s {
            assert!(seen.insert(s.id), "id {} bị lặp", s.id);
            assert_eq!(dict.name_of(s.id), Some(s.name.as_str()), "id {} không khớp tên trong từ điển", s.id);
            assert!(s.source.starts_with("http"), "id {} thiếu nguồn", s.id);
        }
        assert_eq!(
            parse_suggestions(SUGGESTIONS_TOML),
            f.s.iter().map(|s| s.id).collect::<Vec<_>>()
        );
    }
}
