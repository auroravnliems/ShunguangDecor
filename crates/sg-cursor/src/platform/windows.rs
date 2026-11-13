//! Windows: `HKCU\Control Panel\Cursors` (this key only), the active cursor size (`SPI_SETCURSORSIZE`) and `SPI_SETCURSORS`.

use crate::Role;
use crate::backup::RegistrySnapshot;
use ::windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use ::windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_DWORD, REG_EXPAND_SZ, REG_OPTION_NON_VOLATILE, REG_SZ,
    REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegQueryValueExW, RegSetValueExW,
};
use ::windows::Win32::UI::HiDpi::GetDpiForSystem;
use ::windows::Win32::UI::WindowsAndMessaging::{
    SPI_SETCURSORS, SPIF_SENDCHANGE, SYSTEM_PARAMETERS_INFO_ACTION, SystemParametersInfoW,
};
use ::windows::core::{HSTRING, PCWSTR, w};
use std::collections::BTreeMap;
use std::io;

const KEY: PCWSTR = w!(r"Control Panel\Cursors");
const SCHEME_SOURCE: &str = "Scheme Source";
const BASE_SIZE: &str = "CursorBaseSize";

fn check(e: WIN32_ERROR) -> io::Result<()> {
    if e == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(e.0 as i32)) }
}

/// Closes the key when it goes out of scope.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by RegCreateKeyExW and is closed exactly once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn open(access: ::windows::Win32::System::Registry::REG_SAM_FLAGS) -> io::Result<Key> {
    let mut h = HKEY::default();
    // SAFETY: the arguments are valid and `h` is a local variable.
    unsafe {
        check(RegCreateKeyExW(HKEY_CURRENT_USER, KEY, None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, access, None, &mut h, None))?;
    }
    Ok(Key(h))
}

/// An empty name means the `(Default)` value.
fn name_arg(name: &str) -> HSTRING {
    HSTRING::from(name)
}

fn query(key: &Key, name: &str) -> io::Result<Option<(REG_VALUE_TYPE, Vec<u8>)>> {
    let n = name_arg(name);
    let pname = if name.is_empty() { PCWSTR::null() } else { PCWSTR(n.as_ptr()) };
    let mut ty = REG_VALUE_TYPE(0);
    let mut size = 0u32;
    // SAFETY: called twice following the standard pattern: query the size, then read into a buffer of exactly that size.
    unsafe {
        let e = RegQueryValueExW(key.0, pname, None, Some(&mut ty), None, Some(&mut size));
        if e == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(e)?;
        let mut buf = vec![0u8; size as usize];
        let e = RegQueryValueExW(key.0, pname, None, Some(&mut ty), Some(buf.as_mut_ptr()), Some(&mut size));
        if e == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(e)?;
        buf.truncate(size as usize);
        Ok(Some((ty, buf)))
    }
}

fn read_string(key: &Key, name: &str) -> io::Result<Option<String>> {
    Ok(query(key, name)?.and_then(|(ty, b)| {
        (ty == REG_SZ || ty == REG_EXPAND_SZ).then(|| {
            let wide: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&c| c != 0).collect();
            String::from_utf16_lossy(&wide)
        })
    }))
}

fn read_dword(key: &Key, name: &str) -> io::Result<Option<u32>> {
    Ok(query(key, name)?.and_then(|(ty, b)| (ty == REG_DWORD).then(|| b.get(..4).and_then(|x| x.try_into().ok()).map(u32::from_le_bytes)).flatten()))
}

fn delete(key: &Key, name: &str) -> io::Result<()> {
    let n = name_arg(name);
    let pname = if name.is_empty() { PCWSTR::null() } else { PCWSTR(n.as_ptr()) };
    // SAFETY: the key is valid and the name is a null-terminated UTF-16 string.
    let e = unsafe { RegDeleteValueW(key.0, pname) };
    if e == ERROR_FILE_NOT_FOUND { Ok(()) } else { check(e) }
}

fn set_string(key: &Key, name: &str, value: &str) -> io::Result<()> {
    let n = name_arg(name);
    let pname = if name.is_empty() { PCWSTR::null() } else { PCWSTR(n.as_ptr()) };
    let bytes: Vec<u8> = value.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect();
    // Keep the type Windows itself writes: REG_EXPAND_SZ if there is a `%variable%`, otherwise REG_SZ (absolute path).
    let ty = if value.contains('%') { REG_EXPAND_SZ } else { REG_SZ };
    // SAFETY: the UTF-16 buffer has a terminating 0 as REG_SZ/REG_EXPAND_SZ require.
    check(unsafe { RegSetValueExW(key.0, pname, None, ty, Some(&bytes)) })
}

fn set_dword(key: &Key, name: &str, value: u32) -> io::Result<()> {
    let n = name_arg(name);
    // SAFETY: 4 little-endian bytes for REG_DWORD.
    check(unsafe { RegSetValueExW(key.0, PCWSTR(n.as_ptr()), None, REG_DWORD, Some(&value.to_le_bytes())) })
}

pub fn read_registry() -> io::Result<RegistrySnapshot> {
    let key = open(KEY_READ)?;
    let mut roles = BTreeMap::new();
    for role in Role::ALL {
        if let Some(v) = read_string(&key, role.reg_name())? {
            roles.insert(role, v);
        }
    }
    Ok(RegistrySnapshot {
        scheme_name: read_string(&key, "")?,
        scheme_source: read_dword(&key, SCHEME_SOURCE)?,
        base_size: read_dword(&key, BASE_SIZE)?,
        roles,
    })
}

/// Writes the snapshot exactly: a role that is absent → delete the value; `None` → delete that value.
pub fn write_registry(s: &RegistrySnapshot) -> io::Result<()> {
    let key = open(KEY_SET_VALUE | KEY_READ)?;
    for role in Role::ALL {
        match s.roles.get(&role) {
            Some(v) => set_string(&key, role.reg_name(), v)?,
            None => delete(&key, role.reg_name())?,
        }
    }
    match &s.scheme_name {
        Some(n) => set_string(&key, "", n)?,
        None => delete(&key, "")?,
    }
    match s.scheme_source {
        Some(v) => set_dword(&key, SCHEME_SOURCE, v)?,
        None => delete(&key, SCHEME_SOURCE)?,
    }
    match s.base_size {
        Some(v) => set_dword(&key, BASE_SIZE, v)?,
        None => delete(&key, BASE_SIZE)?,
    }
    Ok(())
}

/// `SPI_SETCURSORSIZE`: undocumented; it is the command that Settings → Accessibility → "Mouse pointer" calls. The
/// parameter is the cursor size in px at 100% DPI (same as `CursorBaseSize`), passed directly in `pvParam`.
/// Writing only `CursorBaseSize` to the registry does NOT change the size in the running session (it is read only at
/// sign-in): Windows draws every cursor file at the system's current size, and the image size inside the file only
/// determines sharpness. Tested for real on 13/11 on Win11 26200: changing the registry + SPI_SETCURSORS leaves the
/// cursor size unchanged; calling this command with 128 makes the cursor large immediately.
/// (GetIconInfo/SM_CXCURSOR in a user process always report 32, so they cannot be used to verify this.)
const SPI_SETCURSORSIZE: SYSTEM_PARAMETERS_INFO_ACTION = SYSTEM_PARAMETERS_INFO_ACTION(0x2029);

/// Windows' default cursor size when the registry has no `CursorBaseSize`.
pub const DEFAULT_BASE_SIZE: u32 = 32;

/// Sets the cursor size for the running session, then reloads the cursor scheme from the registry.
/// `base_size` is the `CursorBaseSize` just written (`None` = default 32).
pub fn reload_cursors(base_size: Option<u32>) -> io::Result<()> {
    let size = base_size.unwrap_or(DEFAULT_BASE_SIZE).clamp(DEFAULT_BASE_SIZE, crate::MAX_IMAGE_SIDE);
    // No SPIF_UPDATEINIFILE: the registry is written by write_registry (preserving "no value" exactly on restore).
    // SAFETY: this command reads the size from the pvParam value itself, not from any memory.
    unsafe { SystemParametersInfoW(SPI_SETCURSORSIZE, 0, Some(size as usize as *mut _), SPIF_SENDCHANGE) }
        .map_err(|e| io::Error::other(format!("SPI_SETCURSORSIZE({size}): {e}")))?;
    // Do NOT pass SPIF_UPDATEINIFILE: SPI_SETCURSORS has nothing to write, and that flag makes it return FALSE even
    // after the reload finished, with GetLastError then being a stale leftover code (often 6 "The handle is invalid")
    // → the app used to report a bogus "file read/write error", sometimes yes and sometimes no depending on the
    // leftover code. With SPIF_SENDCHANGE alone it returns TRUE (measured on Win11 26200).
    // SAFETY: SPI_SETCURSORS does not use the pointer parameter.
    unsafe { SystemParametersInfoW(SPI_SETCURSORS, 0, None, SPIF_SENDCHANGE) }
        .map_err(|e| io::Error::other(format!("SPI_SETCURSORS: {e}")))
}

/// System DPI (primary display at sign-in); 96 = 100%.
pub fn system_dpi() -> u32 {
    // SAFETY: no parameters.
    match unsafe { GetDpiForSystem() } {
        0 => 96,
        d => d,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run manually: `cargo test -p sg-cursor -- --ignored`. Read → write back unchanged → read again must match.
    #[test]
    #[ignore]
    fn windows_registry_roundtrip() {
        let before = read_registry().unwrap();
        write_registry(&before).unwrap();
        assert_eq!(read_registry().unwrap(), before);
    }
}
