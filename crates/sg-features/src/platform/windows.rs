//! The real ntdll + registry layer on Windows — ported from ViVe (GPL-3.0, @thebookisclosed):
//! `NativeMethods.Ntdll.cs`, `FeatureManager.cs`, `ViVeTool/Program.cs`.
//!
//! ntdll functions are loaded at runtime with `GetProcAddress` so older Windows just reports "not supported".
//! Every WRITE function here affects the real system — only call it after the user has confirmed.

use std::ffi::c_void;
use std::sync::OnceLock;

use ::windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, WIN32_ERROR,
};
use ::windows::Win32::System::Com::CoCreateGuid;
use ::windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use ::windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, KEY_WRITE, REG_BINARY, REG_DWORD,
    REG_OPTION_NON_VOLATILE, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegDeleteValueW, RegEnumKeyExW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use ::windows::core::{PCSTR, PCWSTR, PWSTR, w};

use super::STATUS_NOT_SUPPORTED;
use crate::model::{FeatureConfig, Store, Subscription, Update, UsageReport};
use crate::native::{
    self, RegOp, RtlFeatureConfiguration, RtlFeatureConfigurationUpdate, RtlFeatureUsageReport,
    RtlFeatureUsageSubscriptionDetails,
};

type FnQueryAll =
    unsafe extern "system" fn(u32, *mut u64, *mut RtlFeatureConfiguration, *mut usize) -> i32;
type FnQueryOne =
    unsafe extern "system" fn(u32, u32, *mut u64, *mut RtlFeatureConfiguration) -> i32;
type FnStamp = unsafe extern "system" fn() -> u64;
type FnQuerySubs =
    unsafe extern "system" fn(*mut RtlFeatureUsageSubscriptionDetails, *mut usize) -> i32;
type FnSet =
    unsafe extern "system" fn(*mut u64, u32, *const RtlFeatureConfigurationUpdate, usize) -> i32;
type FnSubs = unsafe extern "system" fn(*const RtlFeatureUsageSubscriptionDetails, usize) -> i32;
type FnNotify = unsafe extern "system" fn(*mut RtlFeatureUsageReport) -> i32;
type FnSetBoot = unsafe extern "system" fn(i32, *mut i32, i32, *mut c_void) -> i32;
type FnGetBoot = unsafe extern "system" fn(i32, *mut i32, i32, *mut c_void) -> i32;
type FnCreateBootFile = unsafe extern "system" fn(*const u16) -> i32;

/// The loaded ntdll functions. If a required function is missing, the whole table is `None` (unsupported).
struct Ntdll {
    query_all: FnQueryAll,
    query_one: FnQueryOne,
    stamp: Option<FnStamp>,
    query_subs: Option<FnQuerySubs>,
    set: Option<FnSet>,
    subscribe: Option<FnSubs>,
    unsubscribe: Option<FnSubs>,
    notify: Option<FnNotify>,
    set_boot: Option<FnSetBoot>,
    get_boot: Option<FnGetBoot>,
    create_boot_file: Option<FnCreateBootFile>,
}

fn load() -> Option<Ntdll> {
    // SAFETY: ntdll is always preloaded in every process; the function pointers are cast to match the signatures of
    // `NativeMethods.Ntdll.cs`.
    unsafe {
        let module = GetModuleHandleW(w!("ntdll.dll")).ok()?;
        macro_rules! proc {
            ($name:literal, $ty:ty) => {
                GetProcAddress(module, PCSTR($name.as_ptr()))
                    .map(|p| std::mem::transmute::<_, $ty>(p))
            };
        }
        Some(Ntdll {
            query_all: proc!(b"RtlQueryAllFeatureConfigurations\0", FnQueryAll)?,
            query_one: proc!(b"RtlQueryFeatureConfiguration\0", FnQueryOne)?,
            stamp: proc!(b"RtlQueryFeatureConfigurationChangeStamp\0", FnStamp),
            query_subs: proc!(
                b"RtlQueryFeatureUsageNotificationSubscriptions\0",
                FnQuerySubs
            ),
            set: proc!(b"RtlSetFeatureConfigurations\0", FnSet),
            subscribe: proc!(b"RtlSubscribeForFeatureUsageNotification\0", FnSubs),
            unsubscribe: proc!(b"RtlUnsubscribeFromFeatureUsageNotifications\0", FnSubs),
            notify: proc!(b"RtlNotifyFeatureUsage\0", FnNotify),
            set_boot: proc!(b"RtlSetSystemBootStatus\0", FnSetBoot),
            get_boot: proc!(b"RtlGetSystemBootStatus\0", FnGetBoot),
            create_boot_file: proc!(b"RtlCreateBootStatusDataFile\0", FnCreateBootFile),
        })
    }
}

fn ntdll() -> Option<&'static Ntdll> {
    static CELL: OnceLock<Option<Ntdll>> = OnceLock::new();
    CELL.get_or_init(load).as_ref()
}

/// `RTL_FEATURE_CONFIGURATION_TYPE`: Boot = 0, Runtime = 1.
fn store_type(store: Store) -> u32 {
    match store {
        Store::Boot => 0,
        Store::Runtime => 1,
    }
}

/// The machine has `RtlQueryFeatureConfiguration` (Windows 10 version 2004 or later).
pub fn supported() -> bool {
    ntdll().is_some()
}

pub fn change_stamp() -> u64 {
    // SAFETY: the function takes no parameters.
    ntdll().and_then(|n| n.stamp).map_or(0, |f| unsafe { f() })
}

pub fn query(id: u32, store: Store) -> Option<FeatureConfig> {
    let n = ntdll()?;
    let mut stamp = 0u64;
    let mut cfg = RtlFeatureConfiguration::default();
    // SAFETY: the pointer points to a valid local variable.
    let status = unsafe { (n.query_one)(id, store_type(store), &mut stamp, &mut cfg) };
    if status != 0 {
        return None;
    }
    cfg.to_config()
}

/// `STATUS_BUFFER_OVERFLOW`: the buffer is too small, the returned `count` is the number needed.
const STATUS_BUFFER_OVERFLOW: i32 = 0x8000_0005_u32 as i32;

/// ViVe's two-call pattern: ask for the count (null buffer), then allocate a buffer and call again; if the set grows
/// between the two calls (`STATUS_BUFFER_OVERFLOW`), start over. `call(buf, &mut count)` is the ntdll call.
/// ntdll's count parameter is SIZE_T (8 bytes, measured on a real build 26200), so it must be `usize`.
fn query_two_calls<T: Default + Clone>(
    mut call: impl FnMut(*mut T, &mut usize) -> i32,
) -> Result<Vec<T>, i32> {
    let mut last = STATUS_BUFFER_OVERFLOW;
    for slack in [0usize, 64, 1024, 4096] {
        let mut count = 0usize;
        let status = call(std::ptr::null_mut(), &mut count);
        if status != 0 && status != STATUS_BUFFER_OVERFLOW {
            return Err(status);
        }
        if count == 0 {
            return Ok(Vec::new());
        }
        let cap = count + slack;
        let mut buf = vec![T::default(); cap];
        let mut got = cap;
        // `buf` has exactly `cap` elements; `got` is the capacity in / the count out.
        let status = call(buf.as_mut_ptr(), &mut got);
        if status == 0 {
            buf.truncate(got.min(cap));
            return Ok(buf);
        }
        if status != STATUS_BUFFER_OVERFLOW {
            return Err(status);
        }
        last = status;
    }
    Err(last)
}

/// Raw query (keeps the Wexp flag too) — used by `fix_priority`.
fn query_all_raw(store: Store) -> Result<Vec<RtlFeatureConfiguration>, i32> {
    let n = ntdll().ok_or(STATUS_NOT_SUPPORTED)?;
    let ty = store_type(store);
    // SAFETY: the buffer is null only when the incoming count = 0; otherwise `buf` holds `count` elements.
    query_two_calls(|buf, count| unsafe { (n.query_all)(ty, std::ptr::null_mut(), buf, count) })
}

/// All configurations of a store. ntdll errors are returned (NOT swallowed into an empty list) so the caller can refuse to write/export.
pub fn query_all(store: Store) -> Result<Vec<FeatureConfig>, i32> {
    match query_all_raw(store) {
        Ok(v) => Ok(v.iter().filter_map(|c| c.to_config()).collect()),
        Err(status) => {
            tracing::warn!(status, ?store, "RtlQueryAllFeatureConfigurations thất bại");
            Err(status)
        }
    }
}

pub fn query_subscriptions() -> Vec<Subscription> {
    let Some(f) = ntdll().and_then(|n| n.query_subs) else {
        return Vec::new();
    };
    // SAFETY: same as `query_all_raw`.
    match query_two_calls::<RtlFeatureUsageSubscriptionDetails>(|buf, count| unsafe {
        f(buf, count)
    }) {
        Ok(v) => v.iter().map(Subscription::from).collect(),
        Err(status) => {
            tracing::warn!(
                status,
                "RtlQueryFeatureUsageNotificationSubscriptions thất bại"
            );
            Vec::new()
        }
    }
}

// ---------- GHI ----------

/// Converts a Win32 error code to an HRESULT (`HRESULT_FROM_WIN32`), like ViVe's `Exception.HResult`.
fn hresult(code: WIN32_ERROR) -> i32 {
    (0x8007_0000u32 | (code.0 & 0xFFFF)) as i32
}

fn check(code: WIN32_ERROR) -> Result<(), i32> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(hresult(code))
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Opens/creates an HKLM key with write access, runs `f`, closes the key.
fn with_created_key<T>(key: &str, f: impl FnOnce(HKEY) -> Result<T, i32>) -> Result<T, i32> {
    let k = wide(key);
    let mut hk = HKEY::default();
    // SAFETY: the string is NUL-terminated, `hk` is valid.
    let code = unsafe {
        RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(k.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hk,
            None,
        )
    };
    check(code)?;
    let r = f(hk);
    // SAFETY: `hk` was just opened.
    unsafe {
        let _ = RegCloseKey(hk);
    }
    r
}

/// Opens an existing HKLM key; `Ok(None)` if the key does not exist.
fn with_open_key<T>(
    key: &str,
    access: ::windows::Win32::System::Registry::REG_SAM_FLAGS,
    f: impl FnOnce(HKEY) -> Result<T, i32>,
) -> Result<Option<T>, i32> {
    let k = wide(key);
    let mut hk = HKEY::default();
    // SAFETY: as above.
    let code = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(k.as_ptr()),
            None,
            access,
            &mut hk,
        )
    };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(code)?;
    let r = f(hk);
    // SAFETY: `hk` was just opened.
    unsafe {
        let _ = RegCloseKey(hk);
    }
    r.map(Some)
}

fn set_value(hk: HKEY, name: &str, ty: REG_VALUE_TYPE, data: &[u8]) -> Result<(), i32> {
    let n = wide(name);
    // SAFETY: `data` lives for the whole call.
    check(unsafe { RegSetValueExW(hk, PCWSTR(n.as_ptr()), None, ty, Some(data)) })
}

fn read_value(hk: HKEY, name: &str) -> Result<Option<Vec<u8>>, i32> {
    let n = wide(name);
    let mut size = 0u32;
    // SAFETY: the first call only asks for the size.
    let code =
        unsafe { RegQueryValueExW(hk, PCWSTR(n.as_ptr()), None, None, None, Some(&mut size)) };
    if code == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check(code)?;
    let mut buf = vec![0u8; size as usize];
    // SAFETY: `buf` holds `size` bytes.
    let code = unsafe {
        RegQueryValueExW(
            hk,
            PCWSTR(n.as_ptr()),
            None,
            None,
            Some(buf.as_mut_ptr()),
            Some(&mut size),
        )
    };
    check(code)?;
    buf.truncate(size as usize);
    Ok(Some(buf))
}

fn read_dword(hk: HKEY, name: &str) -> Result<Option<u32>, i32> {
    Ok(read_value(hk, name)?.and_then(|b| {
        b.get(..4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }))
}

fn apply_reg_op(op: &RegOp) -> Result<(), i32> {
    match op {
        RegOp::EnsureKey { key } => with_created_key(key, |_| Ok(())),
        RegOp::SetDword { key, name, value } => with_created_key(key, |hk| {
            set_value(hk, name, REG_DWORD, &value.to_le_bytes())
        }),
        RegOp::DeleteTree { key } => {
            let k = wide(key);
            // SAFETY: the string is NUL-terminated.
            let code = unsafe { RegDeleteTreeW(HKEY_LOCAL_MACHINE, PCWSTR(k.as_ptr())) };
            if code == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                check(code)
            }
        }
        RegOp::DeleteValueIfKeyExists { key, name } => {
            with_open_key(key, KEY_SET_VALUE, |hk| {
                let n = wide(name);
                // SAFETY: `hk` is open.
                let code = unsafe { RegDeleteValueW(hk, PCWSTR(n.as_ptr())) };
                if code == ERROR_FILE_NOT_FOUND {
                    Ok(())
                } else {
                    check(code)
                }
            })
            .map(|_| ())
        }
    }
}

/// `BSD_FEATURE_CONFIGURATION_STATE.BootPending`.
const BSD_BOOT_PENDING: i32 = 1;
/// `RtlBsdItemFeatureConfigurationState`.
const BSD_ITEM_FEATURE_CONFIG_STATE: i32 = 17;
const STATUS_OBJECT_NAME_NOT_FOUND: i32 = 0xC000_0034_u32 as i32;

/// ViVeTool's `UpdateLKGStatus`: tells startup there is a new boot configuration. Errors only produce a warning.
fn update_lkg_status(new_state: i32) {
    let Some(n) = ntdll() else { return };
    let (Some(get), Some(set)) = (n.get_boot, n.set_boot) else {
        return;
    };
    let mut current = 0i32;
    // SAFETY: pointer to a 4-byte local variable.
    let mut status = unsafe {
        get(
            BSD_ITEM_FEATURE_CONFIG_STATE,
            &mut current,
            4,
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        if status == STATUS_OBJECT_NAME_NOT_FOUND {
            let Some(create) = n.create_boot_file else {
                return;
            };
            // SAFETY: null = default path, as in ViVe.
            status = unsafe { create(std::ptr::null()) };
            if status != 0 {
                tracing::warn!(status, "RtlCreateBootStatusDataFile thất bại");
                return;
            }
        } else {
            tracing::warn!(status, "RtlGetSystemBootStatus thất bại");
        }
    }
    if current != new_state {
        let mut v = new_state;
        // SAFETY: as above.
        let status = unsafe {
            set(
                BSD_ITEM_FEATURE_CONFIG_STATE,
                &mut v,
                4,
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            tracing::warn!(status, "RtlSetSystemBootStatus thất bại");
        }
    }
}

/// WRITE: set/delete configurations. Runtime → `RtlSetFeatureConfigurations`; Boot → write the registry like
/// `SetFeatureConfigurationsInRegistry`, then set the LKG status = BootPending (like ViVeTool's `FinalizeSet`).
pub fn set(updates: &[Update], store: Store) -> Result<(), i32> {
    for u in updates {
        native::validate_update(u)?;
    }
    if updates.is_empty() {
        return Ok(());
    }
    match store {
        Store::Runtime => {
            let n = ntdll().ok_or(STATUS_NOT_SUPPORTED)?;
            let f = n.set.ok_or(STATUS_NOT_SUPPORTED)?;
            let native: Vec<_> = updates
                .iter()
                .map(RtlFeatureConfigurationUpdate::from_update)
                .collect();
            let mut prev = 0u64;
            // SAFETY: `native` lives for the whole call, `prev` = 0 means the change stamp is not checked.
            let status = unsafe {
                f(
                    &mut prev,
                    store_type(Store::Runtime),
                    native.as_ptr(),
                    native.len(),
                )
            };
            if status == 0 { Ok(()) } else { Err(status) }
        }
        Store::Boot => {
            for u in updates {
                for op in native::registry_ops(u) {
                    apply_reg_op(&op)?;
                }
            }
            update_lkg_status(BSD_BOOT_PENDING);
            Ok(())
        }
    }
}

pub fn add_subscriptions(subs: &[Subscription]) -> Result<(), i32> {
    let f = ntdll()
        .and_then(|n| n.subscribe)
        .ok_or(STATUS_NOT_SUPPORTED)?;
    let native: Vec<RtlFeatureUsageSubscriptionDetails> = subs.iter().map(Into::into).collect();
    // SAFETY: `native` lives for the whole call.
    let status = unsafe { f(native.as_ptr(), native.len()) };
    if status != 0 {
        return Err(status);
    }
    // ViVe also writes to the registry so the subscription persists after a restart.
    for s in subs {
        let guid = new_guid_string()?;
        let key = format!(r"{}\{guid}", native::subscription_key(s.id));
        with_created_key(&key, |hk| {
            set_value(
                hk,
                "ReportingKind",
                REG_DWORD,
                &u32::from(s.reporting_kind).to_le_bytes(),
            )?;
            set_value(
                hk,
                "ReportingOptions",
                REG_DWORD,
                &u32::from(s.reporting_options).to_le_bytes(),
            )?;
            set_value(
                hk,
                "ReportingTarget",
                REG_BINARY,
                &s.reporting_target.to_le_bytes(),
            )
        })?;
    }
    Ok(())
}

fn new_guid_string() -> Result<String, i32> {
    // SAFETY: CoCreateGuid does not need COM to be initialized.
    let g = unsafe { CoCreateGuid() }.map_err(|e| e.code().0)?;
    Ok(format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        g.data1,
        g.data2,
        g.data3,
        g.data4[0],
        g.data4[1],
        g.data4[2],
        g.data4[3],
        g.data4[4],
        g.data4[5],
        g.data4[6],
        g.data4[7]
    ))
}

pub fn remove_subscriptions(subs: &[Subscription]) -> Result<(), i32> {
    let f = ntdll()
        .and_then(|n| n.unsubscribe)
        .ok_or(STATUS_NOT_SUPPORTED)?;
    let native: Vec<RtlFeatureUsageSubscriptionDetails> = subs.iter().map(Into::into).collect();
    // SAFETY: `native` lives for the whole call.
    let status = unsafe { f(native.as_ptr(), native.len()) };
    if status != 0 {
        return Err(status);
    }
    for s in subs {
        remove_subscription_from_registry(s)?;
    }
    Ok(())
}

/// `RemoveFeatureUsageSubscriptionsFromRegistry` for a single subscription.
fn remove_subscription_from_registry(s: &Subscription) -> Result<(), i32> {
    let parent = native::subscription_key(s.id);
    let children = with_open_key(&parent, KEY_READ, |hk| {
        let mut names = Vec::new();
        for index in 0u32.. {
            let mut buf = [0u16; 256];
            let mut len = buf.len() as u32;
            // SAFETY: `buf` holds `len` characters.
            let code = unsafe {
                RegEnumKeyExW(
                    hk,
                    index,
                    Some(PWSTR(buf.as_mut_ptr())),
                    &mut len,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if code == ERROR_NO_MORE_ITEMS {
                break;
            }
            check(code)?;
            names.push(String::from_utf16_lossy(&buf[..len as usize]));
        }
        Ok(names)
    })?;
    let Some(children) = children else {
        return Ok(());
    };
    let mut remaining = children.len();
    for name in children {
        let child = format!(r"{parent}\{name}");
        let matches = with_open_key(&child, KEY_READ, |hk| {
            let kind = read_dword(hk, "ReportingKind")?;
            let target = read_value(hk, "ReportingTarget")?;
            let target = target
                .and_then(|b| <[u8; 8]>::try_from(b.get(..8)?).ok())
                .map(u64::from_le_bytes);
            Ok(kind == Some(u32::from(s.reporting_kind)) && target == Some(s.reporting_target))
        })?
        .unwrap_or(false);
        if matches {
            apply_reg_op(&RegOp::DeleteTree { key: child })?;
            remaining -= 1;
        }
    }
    if remaining == 0 {
        apply_reg_op(&RegOp::DeleteTree { key: parent })?;
    }
    Ok(())
}

pub fn notify_usage(report: &UsageReport) -> Result<(), i32> {
    let f = ntdll().and_then(|n| n.notify).ok_or(STATUS_NOT_SUPPORTED)?;
    let mut native = RtlFeatureUsageReport::from(report);
    // SAFETY: pointer to a local variable.
    let status = unsafe { f(&mut native) };
    if status == 0 { Ok(()) } else { Err(status) }
}

// There is no `fix_lkg`: ViVeTool's `/fixlkg` actually does nothing (wrong key path, opened read-only), so it was dropped entirely.

/// `/fixpriority`: moves `Service`-priority configurations (without Wexp) to the `User` priority, Runtime first, then Boot.
/// Returns the number of configurations moved. Stops and returns the error as soon as one store fails (like ViVeTool's `HandleFixPriority`).
pub fn fix_priority() -> Result<usize, i32> {
    let mut fixed = 0;
    for store in [Store::Runtime, Store::Boot] {
        let cfgs = query_all_raw(store)?;
        let updates = native::priority_fix_updates(&cfgs);
        set(&updates, store)?;
        fixed += updates.len() / 2;
    }
    Ok(fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// READ-ONLY: run with `cargo test -p sg-features -- --ignored query_real_machine`.
    #[test]
    #[ignore = "đọc ntdll thật trên máy này"]
    fn query_real_machine() {
        assert!(supported(), "máy này phải có RtlQueryFeatureConfiguration");
        let all = query_all(Store::Runtime).unwrap();
        println!("runtime configs: {}", all.len());
        assert!(!all.is_empty(), "kho Runtime không được rỗng");
        let first = all[0];
        let one = query(first.id, Store::Runtime).expect("truy vấn lẻ phải trả cấu hình");
        assert_eq!(one.id, first.id);
        println!("boot configs: {}", query_all(Store::Boot).unwrap().len());
        println!("change stamp: {}", change_stamp());
        let subs = query_subscriptions();
        println!("subscriptions: {}", subs.len());
        assert!(!subs.is_empty());
        assert_eq!(
            subs.len(),
            query_subscriptions().len(),
            "hai lần gọi phải cùng số lượng"
        );

        // Check the width of count (SIZE_T): capacity in = 1, a 2-element buffer filled with sentinel values. The function must return
        // STATUS_BUFFER_OVERFLOW, with count out = the exact number, and write nothing into the buffer.
        let nt = ntdll().unwrap();
        let sentinel = RtlFeatureConfiguration {
            feature_id: 0xDEAD_BEEF,
            compact_state: 0xCAFE_F00D,
            variant_payload: 7,
        };
        let mut buf = [sentinel; 2];
        let mut count = 1usize;
        // SAFETY: `buf` has 2 elements, count is a usize like the real ntdll.
        let status =
            unsafe { (nt.query_all)(1, std::ptr::null_mut(), buf.as_mut_ptr(), &mut count) };
        assert_eq!(status, STATUS_BUFFER_OVERFLOW);
        assert_eq!(buf, [sentinel; 2]);
        assert!(
            count > 100,
            "count ra phải là tổng số cấu hình, được {count}"
        );
        // The two-call pattern returns that full count.
        let raw = query_all_raw(Store::Runtime).unwrap();
        assert_eq!(raw.len(), count);
        assert!(raw.iter().any(|c| c.feature_id != 0));
    }
}
