//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! Windows user information for the power menu: display name, account type, email, avatar.

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, EXTENDED_NAME_FORMAT, NameDisplay, NameUserPrincipal};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::System::WindowsProgramming::GetUserNameW;

fn name_ex(fmt: EXTENDED_NAME_FORMAT) -> Option<String> {
    unsafe {
        let mut len = 256u32;
        let mut buf = vec![0u16; len as usize];
        if GetUserNameExW(fmt, Some(PWSTR(buf.as_mut_ptr())), &mut len) {
            let s = String::from_utf16_lossy(&buf[..len as usize]);
            if !s.trim().is_empty() {
                return Some(s);
            }
        }
        None
    }
}

pub fn username() -> String {
    name_ex(NameDisplay).unwrap_or_else(|| unsafe {
        let mut len = 256u32;
        let mut buf = vec![0u16; len as usize];
        if GetUserNameW(Some(PWSTR(buf.as_mut_ptr())), &mut len).is_ok() {
            String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)])
        } else {
            String::new()
        }
    })
}

fn sid_string(sid: PSID) -> Option<String> {
    unsafe {
        let mut p = PWSTR::null();
        ConvertSidToStringSidW(sid, &mut p).ok()?;
        let s = p.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(p.0 as *mut _)));
        s
    }
}

fn token_info(class: TOKEN_INFORMATION_CLASS) -> Option<Vec<u8>> {
    unsafe {
        let mut tok = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(tok, class, None, 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let r = GetTokenInformation(tok, class, Some(buf.as_mut_ptr() as *mut _), len, &mut len);
        let _ = CloseHandle(tok);
        r.ok()?;
        Some(buf)
    }
}

pub fn user_sid() -> Option<String> {
    let buf = token_info(TokenUser)?;
    unsafe {
        let tu = &*(buf.as_ptr() as *const TOKEN_USER);
        sid_string(tu.User.Sid)
    }
}

/// "Administrator" if the account belongs to the Administrators group (even when UAC filters the token), otherwise "Standard User".
pub fn account_type() -> String {
    let admin = token_info(TokenGroups)
        .map(|buf| unsafe {
            let g = &*(buf.as_ptr() as *const TOKEN_GROUPS);
            let groups = std::slice::from_raw_parts(g.Groups.as_ptr(), g.GroupCount as usize);
            groups.iter().any(|x| sid_string(x.Sid).as_deref() == Some("S-1-5-32-544"))
        })
        .unwrap_or(false);
    if admin { "Administrator".into() } else { "Standard User".into() }
}

pub fn email() -> Option<String> {
    if let Some(e) = name_ex(NameUserPrincipal) {
        return Some(e);
    }
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\IdentityCRL\\UserExtendedProperties"), Some(0), KEY_READ, &mut key).is_err() {
            return None;
        }
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        let r = RegEnumKeyExW(key, 0, Some(PWSTR(buf.as_mut_ptr())), &mut len, None, None, None, None);
        let _ = RegCloseKey(key);
        if r.is_ok() {
            return Some(String::from_utf16_lossy(&buf[..len as usize]));
        }
    }
    None
}

/// The largest account picture in the registry; if there is none, Windows' default picture.
pub fn avatar_path() -> Option<String> {
    if let Some(sid) = user_sid() {
        unsafe {
            let path: Vec<u16> = format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\AccountPicture\\Users\\{sid}").encode_utf16().chain(Some(0)).collect();
            let mut key = HKEY::default();
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, PCWSTR(path.as_ptr()), Some(0), KEY_READ, &mut key).is_ok() {
                let mut best: Option<(u32, String)> = None;
                for i in 0.. {
                    let mut name = [0u16; 128];
                    let mut nlen = name.len() as u32;
                    let mut data = vec![0u8; 2048];
                    let mut dlen = data.len() as u32;
                    let mut ty = 0u32;
                    let r = RegEnumValueW(key, i, Some(PWSTR(name.as_mut_ptr())), &mut nlen, None, Some(&mut ty), Some(data.as_mut_ptr()), Some(&mut dlen));
                    if r.is_err() {
                        break;
                    }
                    let n = String::from_utf16_lossy(&name[..nlen as usize]);
                    if ty != REG_SZ.0 {
                        continue;
                    }
                    let Some(res) = n.strip_prefix("Image").and_then(|x| x.parse::<u32>().ok()) else { continue };
                    let wide = std::slice::from_raw_parts(data.as_ptr() as *const u16, dlen as usize / 2);
                    let v = String::from_utf16_lossy(wide).trim_end_matches('\0').to_string();
                    if std::path::Path::new(&v).is_file() && best.as_ref().map(|b| res > b.0).unwrap_or(true) {
                        best = Some((res, v));
                    }
                }
                let _ = RegCloseKey(key);
                if let Some((_, p)) = best {
                    return Some(p);
                }
            }
        }
    }
    let pd = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
    let p = format!("{pd}\\Microsoft\\User Account Pictures\\user-192.png");
    std::path::Path::new(&p).is_file().then_some(p)
}
