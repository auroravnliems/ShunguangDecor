//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! System metrics for the cpu/memory/wifi widgets: reads Win32 directly (GetSystemTimes, GlobalMemoryStatusEx,
//! GetIfTable2, WLAN API) in place of YASB's psutil. Every function is fast (< 1ms) and is called on the UI thread.

use std::cell::RefCell;
use std::time::Instant;

use windows::Win32::Foundation::{FILETIME, HANDLE};
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::NetworkManagement::WiFi::*;
use windows::Win32::Networking::WinSock::AF_INET;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

// ------------------------------------------------------------------ CPU

thread_local! {
    static CPU_LAST: RefCell<Option<(u64, u64)>> = const { RefCell::new(None) };
}

/// System-wide CPU % since the previous call (returns 0 on the first call).
pub fn cpu_percent() -> f32 {
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_err() {
        return 0.0;
    }
    // kernel time already includes idle
    let total = ft(kernel) + ft(user);
    let idle = ft(idle);
    CPU_LAST.with(|l| {
        let mut l = l.borrow_mut();
        let r = match *l {
            Some((t0, i0)) if total > t0 => {
                let dt = (total - t0) as f64;
                let di = idle.saturating_sub(i0) as f64;
                ((1.0 - di / dt) * 100.0).clamp(0.0, 100.0) as f32
            }
            _ => 0.0,
        };
        *l = Some((total, idle));
        r
    })
}

pub fn cpu_cores() -> (u32, u32) {
    let logical = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
    // physical core count: count RelationProcessorCore
    let physical = unsafe {
        use windows::Win32::System::SystemInformation::*;
        let mut len = 0u32;
        let _ = GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut len);
        let mut buf = vec![0u8; len as usize];
        if len > 0 && GetLogicalProcessorInformationEx(RelationProcessorCore, Some(buf.as_mut_ptr() as *mut _), &mut len).is_ok() {
            let mut n = 0;
            let mut off = 0usize;
            while off + 8 <= len as usize {
                let size = u32::from_ne_bytes(buf[off + 4..off + 8].try_into().unwrap()) as usize;
                if size == 0 {
                    break;
                }
                n += 1;
                off += size;
            }
            n
        } else {
            logical
        }
    };
    (physical.max(1), logical)
}

// ------------------------------------------------------------------ RAM

#[derive(Clone, Copy, Debug, Default)]
pub struct Mem {
    pub total: u64,
    pub avail: u64,
    pub used: u64,
    pub percent: f32,
    pub swap_total: u64,
    pub swap_used: u64,
    pub swap_percent: f32,
}

pub fn memory() -> Mem {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if unsafe { GlobalMemoryStatusEx(&mut m) }.is_err() || m.ullTotalPhys == 0 {
        return Mem::default();
    }
    let total = m.ullTotalPhys;
    let avail = m.ullAvailPhys;
    let used = total.saturating_sub(avail);
    // like psutil: swap = commit limit minus RAM
    let swap_total = m.ullTotalPageFile.saturating_sub(m.ullTotalPhys);
    let commit_used = m.ullTotalPageFile.saturating_sub(m.ullAvailPageFile);
    let swap_used = commit_used.saturating_sub(used).min(swap_total);
    Mem {
        total,
        avail,
        used,
        percent: used as f32 * 100.0 / total as f32,
        swap_total,
        swap_used,
        swap_percent: if swap_total > 0 { swap_used as f32 * 100.0 / swap_total as f32 } else { 0.0 },
    }
}

/// `humanize.naturalsize(v, binary=True, gnu=True, fmt)` like YASB: "7.2G", "512M", "900B".
pub fn naturalsize(bytes: u64, decimals: usize) -> String {
    const UNITS: [&str; 8] = ["K", "M", "G", "T", "P", "E", "Z", "Y"];
    let b = bytes as f64;
    if b < 1024.0 {
        return format!("{bytes}B");
    }
    let mut v = b / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.decimals$}{}", UNITS[i])
}

// ------------------------------------------------------------------ network

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum NetKind {
    #[default]
    None,
    Ethernet,
    Wifi,
    Other,
}

#[derive(Clone, Debug, Default)]
pub struct Net {
    pub kind: NetKind,
    /// adapter name (Alias), or the SSID when on Wi-Fi
    pub name: String,
    pub ssid: String,
    /// 0..=100 (Wi-Fi), 100 for wired
    pub quality: u32,
    pub ip: String,
    pub link_mbps: u64,
    /// bytes/second
    pub down: f64,
    pub up: f64,
}

struct NetState {
    last: Option<(u32, u64, u64, Instant)>,
    wlan: Option<HANDLE>,
}

thread_local! {
    static NET: RefCell<NetState> = const { RefCell::new(NetState { last: None, wlan: None }) };
}

fn wstr(s: &[u16]) -> String {
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..n])
}

fn ipv4_of(index: u32) -> String {
    unsafe {
        let mut t: *mut MIB_UNICASTIPADDRESS_TABLE = std::ptr::null_mut();
        if GetUnicastIpAddressTable(AF_INET, &mut t).is_err() || t.is_null() {
            return String::new();
        }
        let rows = std::slice::from_raw_parts((*t).Table.as_ptr(), (*t).NumEntries as usize);
        let mut ip = String::new();
        for r in rows {
            if r.InterfaceIndex == index {
                let a = r.Address.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes();
                ip = format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3]);
                break;
            }
        }
        FreeMibTable(t as *const _);
        ip
    }
}

fn wifi_info(st: &mut NetState) -> Option<(String, u32)> {
    unsafe {
        if st.wlan.is_none() {
            let mut ver = 0u32;
            let mut h = HANDLE::default();
            if WlanOpenHandle(2, None, &mut ver, &mut h) != 0 {
                return None;
            }
            st.wlan = Some(h);
        }
        let h = st.wlan?;
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if WlanEnumInterfaces(h, None, &mut list) != 0 || list.is_null() {
            return None;
        }
        let infos = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), (*list).dwNumberOfItems as usize);
        let mut out = None;
        for info in infos {
            if info.isState != wlan_interface_state_connected {
                continue;
            }
            let mut size = 0u32;
            let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
            if WlanQueryInterface(h, &info.InterfaceGuid, wlan_intf_opcode_current_connection, None, &mut size, &mut data, None) == 0 && !data.is_null() {
                let a = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                let s = &a.wlanAssociationAttributes.dot11Ssid;
                let ssid = String::from_utf8_lossy(&s.ucSSID[..(s.uSSIDLength as usize).min(32)]).into_owned();
                out = Some((ssid, a.wlanAssociationAttributes.wlanSignalQuality.min(100)));
                WlanFreeMemory(data);
                break;
            }
        }
        WlanFreeMemory(list as *const _);
        out
    }
}

/// The connection currently used to reach the Internet (the adapter with a route to 1.1.1.1) and its upload/download speed.
pub fn network() -> Net {
    NET.with(|st| {
        let mut st = st.borrow_mut();
        let mut idx = 0u32;
        let dest = u32::from_ne_bytes([1, 1, 1, 1]);
        if unsafe { GetBestInterface(dest, &mut idx) } != 0 {
            st.last = None;
            return Net::default();
        }
        let mut net = Net::default();
        let mut octets = None;
        unsafe {
            let mut t: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
            if GetIfTable2(&mut t).is_ok() && !t.is_null() {
                let rows = std::slice::from_raw_parts((*t).Table.as_ptr(), (*t).NumEntries as usize);
                if let Some(r) = rows.iter().find(|r| r.InterfaceIndex == idx) {
                    net.kind = match r.Type {
                        6 => NetKind::Ethernet,
                        71 => NetKind::Wifi,
                        _ => NetKind::Other,
                    };
                    net.name = wstr(&r.Alias);
                    net.link_mbps = r.ReceiveLinkSpeed / 1_000_000;
                    octets = Some((r.InOctets, r.OutOctets));
                }
                FreeMibTable(t as *const _);
            }
        }
        if net.kind == NetKind::None {
            st.last = None;
            return net;
        }
        net.quality = 100;
        if net.kind == NetKind::Wifi
            && let Some((ssid, q)) = wifi_info(&mut st) {
                net.ssid = ssid.clone();
                net.name = ssid;
                net.quality = q;
            }
        net.ip = ipv4_of(idx);
        if let Some((rx, tx)) = octets {
            let now = Instant::now();
            if let Some((li, lrx, ltx, lt)) = st.last {
                let dt = now.duration_since(lt).as_secs_f64();
                if li == idx && dt > 0.05 {
                    net.down = rx.saturating_sub(lrx) as f64 / dt;
                    net.up = tx.saturating_sub(ltx) as f64 / dt;
                }
            }
            st.last = Some((idx, rx, tx, now));
        }
        net
    })
}

/// Speed formatted like "1.2 MB/s", "350 KB/s".
pub fn speed(bps: f64) -> String {
    if bps >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", bps / 1024.0 / 1024.0)
    } else if bps >= 1024.0 {
        format!("{:.0} KB/s", bps / 1024.0)
    } else {
        format!("{:.0} B/s", bps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(naturalsize(512, 1), "512B");
        assert_eq!(naturalsize(1536, 1), "1.5K");
        assert_eq!(naturalsize(16 * 1024 * 1024 * 1024, 1), "16.0G");
        assert_eq!(naturalsize(7_730_941_133, 0), "7G");
        assert!(memory().total > 0);
    }
}
