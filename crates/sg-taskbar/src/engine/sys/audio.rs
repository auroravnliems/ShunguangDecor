//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! WASAPI: default playback device volume, device list, per-application audio sessions.
//! Changes from the system (volume keys, device switches) are reported to the UI thread via `post_notify`.

use std::cell::RefCell;

use windows::core::{implement, Interface, GUID, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

use crate::engine::app::{N_AUDIO, N_AUDIO_DEVICES};
use crate::engine::win::post_notify;

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback;

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, _data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        post_notify(N_AUDIO, 0);
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceCallback;

impl IMMNotificationClient_Impl for DeviceCallback_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
        post_notify(N_AUDIO_DEVICES, 0);
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        post_notify(N_AUDIO_DEVICES, 0);
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        post_notify(N_AUDIO_DEVICES, 0);
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
        if flow == eRender && role == eMultimedia {
            post_notify(N_AUDIO_DEVICES, 0);
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}

// IPolicyConfig (undocumented) – used by Windows to set the default device; YASB does the same.
#[allow(non_snake_case)]
mod policy {
    use windows::core::{interface, IUnknown, IUnknown_Vtbl, HRESULT, PCWSTR};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::ERole;

    #[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
    pub unsafe trait IPolicyConfig: IUnknown {
        fn GetMixFormat(&self, a: PCWSTR, b: *mut *mut std::ffi::c_void) -> HRESULT;
        fn GetDeviceFormat(&self, a: PCWSTR, b: i32, c: *mut *mut std::ffi::c_void) -> HRESULT;
        fn ResetDeviceFormat(&self, a: PCWSTR) -> HRESULT;
        fn SetDeviceFormat(&self, a: PCWSTR, b: *mut std::ffi::c_void, c: *mut std::ffi::c_void) -> HRESULT;
        fn GetProcessingPeriod(&self, a: PCWSTR, b: i32, c: *mut i64, d: *mut i64) -> HRESULT;
        fn SetProcessingPeriod(&self, a: PCWSTR, b: *mut i64) -> HRESULT;
        fn GetShareMode(&self, a: PCWSTR, b: *mut std::ffi::c_void) -> HRESULT;
        fn SetShareMode(&self, a: PCWSTR, b: *mut std::ffi::c_void) -> HRESULT;
        fn GetPropertyValue(&self, a: PCWSTR, b: *const PROPERTYKEY, c: *mut std::ffi::c_void) -> HRESULT;
        fn SetPropertyValue(&self, a: PCWSTR, b: *const PROPERTYKEY, c: *mut std::ffi::c_void) -> HRESULT;
        fn SetDefaultEndpoint(&self, id: PCWSTR, role: ERole) -> HRESULT;
        fn SetEndpointVisibility(&self, a: PCWSTR, b: i32) -> HRESULT;
    }

    pub fn set_default_endpoint(pc: &IPolicyConfig, id: PCWSTR, role: ERole) -> HRESULT {
        unsafe { pc.SetDefaultEndpoint(id, role) }
    }
}
use policy::IPolicyConfig;

const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };

pub struct Session {
    pub pid: u32,
    pub name: String,
    pub volume: ISimpleAudioVolume,
}

struct State {
    enumerator: IMMDeviceEnumerator,
    endpoint: Option<IAudioEndpointVolume>,
    device_id: String,
    vol_cb: IAudioEndpointVolumeCallback,
    _dev_cb: IMMNotificationClient,
}

thread_local! {
    static AUDIO: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn pwstr_to_string(p: windows::core::PWSTR) -> String {
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn init() -> Option<State> {
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev_cb: IMMNotificationClient = DeviceCallback.into();
        let _ = enumerator.RegisterEndpointNotificationCallback(&dev_cb);
        let vol_cb: IAudioEndpointVolumeCallback = VolumeCallback.into();
        let mut st = State { enumerator, endpoint: None, device_id: String::new(), vol_cb, _dev_cb: dev_cb };
        attach_default(&mut st);
        Some(st)
    }
}

fn attach_default(st: &mut State) {
    unsafe {
        if let Some(ep) = st.endpoint.take() {
            let _ = ep.UnregisterControlChangeNotify(&st.vol_cb);
        }
        st.device_id.clear();
        let Ok(dev) = st.enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) else { return };
        if let Ok(id) = dev.GetId() {
            st.device_id = pwstr_to_string(id);
        }
        if let Ok(ep) = dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
            let _ = ep.RegisterControlChangeNotify(&st.vol_cb);
            st.endpoint = Some(ep);
        }
    }
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    AUDIO.with(|a| {
        let mut a = a.borrow_mut();
        if a.is_none() {
            *a = init();
        }
        a.as_mut().map(f)
    })
}

/// Re-attaches the default device (after a device change).
pub fn refresh_device() {
    with(attach_default);
}

/// (volume 0..=100, muted)
pub fn get() -> Option<(i32, bool)> {
    with(|s| unsafe {
        let ep = s.endpoint.as_ref()?;
        let v = ep.GetMasterVolumeLevelScalar().ok()?;
        let m = ep.GetMute().ok()?.as_bool();
        Some(((v * 100.0).round() as i32, m))
    })
    .flatten()
}

pub fn set_volume(v: i32) {
    with(|s| unsafe {
        if let Some(ep) = &s.endpoint {
            let _ = ep.SetMasterVolumeLevelScalar((v.clamp(0, 100) as f32) / 100.0, std::ptr::null());
        }
    });
}

pub fn set_mute(m: bool) {
    with(|s| unsafe {
        if let Some(ep) = &s.endpoint {
            let _ = ep.SetMute(m, std::ptr::null());
        }
    });
}

/// Active playback devices: (id, name), plus the default id.
pub fn devices() -> (Vec<(String, String)>, String) {
    with(|s| unsafe {
        let mut out = Vec::new();
        if let Ok(col) = s.enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) {
            let n = col.GetCount().unwrap_or(0);
            for i in 0..n {
                let Ok(d) = col.Item(i) else { continue };
                let id = d.GetId().map(pwstr_to_string).unwrap_or_default();
                let name = d
                    .OpenPropertyStore(STGM_READ)
                    .and_then(|ps| ps.GetValue(&PKEY_DEVICE_FRIENDLY_NAME))
                    .map(|pv| pv.to_string())
                    .unwrap_or_else(|_| id.clone());
                out.push((id, name));
            }
        }
        (out, s.device_id.clone())
    })
    .unwrap_or_default()
}

pub fn set_default_device(id: &str) {
    unsafe {
        let Ok(pc) = CoCreateInstance::<_, IPolicyConfig>(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL) else {
            crate::engine::log::error("Không tạo được IPolicyConfig để đổi thiết bị âm thanh");
            return;
        };
        let w: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
        for role in [eConsole, eMultimedia, eCommunications] {
            let _ = policy::set_default_endpoint(&pc, PCWSTR(w.as_ptr()), role);
        }
    }
    refresh_device();
}

/// Audio sessions of applications on the default device (system sessions skipped, merged by pid).
pub fn sessions() -> Vec<Session> {
    with(|s| unsafe {
        let mut out: Vec<Session> = Vec::new();
        let Ok(dev) = s.enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) else { return out };
        let Ok(mgr) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { return out };
        let Ok(en) = mgr.GetSessionEnumerator() else { return out };
        let n = en.GetCount().unwrap_or(0);
        crate::engine::log::debug(&format!("audio: {n} phiên"));
        for i in 0..n {
            let Ok(ctl) = en.GetSession(i) else { continue };
            let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else { continue };
            let pid = ctl2.GetProcessId().unwrap_or(0);
            if pid == 0 || ctl2.IsSystemSoundsSession() == windows::Win32::Foundation::S_OK {
                continue;
            }
            if ctl.GetState().map(|st| st == AudioSessionStateExpired).unwrap_or(true) {
                continue;
            }
            if out.iter().any(|x| x.pid == pid) {
                continue;
            }
            let Ok(vol) = ctl.cast::<ISimpleAudioVolume>() else { continue };
            let name = crate::engine::sys::process::app_name(pid);
            out.push(Session { pid, name, volume: vol });
        }
        out
    })
    .unwrap_or_default()
}

impl Session {
    pub fn get(&self) -> (i32, bool) {
        unsafe {
            let v = self.volume.GetMasterVolume().unwrap_or(1.0);
            let m = self.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false);
            ((v * 100.0).round() as i32, m)
        }
    }
    pub fn set(&self, v: i32) {
        unsafe {
            let _ = self.volume.SetMasterVolume(v.clamp(0, 100) as f32 / 100.0, std::ptr::null());
        }
    }
    pub fn set_mute(&self, m: bool) {
        unsafe {
            let _ = self.volume.SetMute(m, std::ptr::null());
        }
    }
}
