// Ported from TachyonWallpaper (the idea originates from Lively Wallpaper, GPL-3.0).
//! Video player built on the Media Foundation Media Engine (built into Windows, hardware decoding).
//! The engine runs in "rendering mode": it draws into the host window itself via MF_MEDIA_ENGINE_PLAYBACK_HWND.

use std::sync::Once;

use windows::core::{implement, Interface, BSTR};
use windows::Win32::Foundation::{HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Multithread, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFDXGIDeviceManager, IMFMediaEngine, IMFMediaEngineClassFactory, IMFMediaEngineEx, IMFMediaEngineNotify,
    IMFMediaEngineNotify_Impl, MFCreateAttributes, MFCreateDXGIDeviceManager, MFStartup, MFARGB, MFVideoNormalizedRect,
    CLSID_MFMediaEngineClassFactory, MFMediaType_Audio, MFSTARTUP_FULL, MF_MT_MAJOR_TYPE, MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_DXGI_MANAGER, MF_MEDIA_ENGINE_EVENT_ERROR,
    MF_MEDIA_ENGINE_EVENT_FORMATCHANGE, MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA, MF_MEDIA_ENGINE_PLAYBACK_HWND, MF_MEDIA_ENGINE_REAL_TIME_MODE,
    MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_VERSION,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

use super::{host, rect_size, PauseReason, Player, EV_MF_ERROR, EV_MF_LOADED};
use sg_desktop::DesktopLayer as Desktop;
use crate::config::Scaling;

pub fn startup() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
            logln!("MFStartup lỗi: {e}");
        }
    });
}

thread_local! {
    /// One D3D11 device shared by every Media Engine. If each engine creates its own device
    /// (the default of PLAYBACK_HWND mode), every video switch leaks ~45 MB that is never returned
    /// (measured with examples/mf_leak_probe.rs). When shared, memory stays flat.
    static DXGI_MANAGER: std::cell::RefCell<Option<IMFDXGIDeviceManager>> = const { std::cell::RefCell::new(None) };
}

fn shared_device_manager() -> Option<IMFDXGIDeviceManager> {
    DXGI_MANAGER.with(|cell| {
        if let Some(m) = cell.borrow().as_ref() {
            return Some(m.clone());
        }
        let created = unsafe { create_device_manager() };
        match created {
            Ok(m) => {
                *cell.borrow_mut() = Some(m.clone());
                Some(m)
            }
            Err(e) => {
                logln!("Không tạo được D3D11 device dùng chung, để Media Foundation tự tạo: {e}");
                None
            }
        }
    })
}

unsafe fn create_device_manager() -> windows::core::Result<IMFDXGIDeviceManager> {
    let mut device = None;
    D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        HMODULE::default(),
        D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        None,
        D3D11_SDK_VERSION,
        Some(&mut device),
        None,
        None,
    )?;
    let device = device.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
    // The Media Engine uses the device from multiple threads.
    if let Ok(mt) = device.cast::<ID3D11Multithread>() {
        let _ = mt.SetMultithreadProtected(true);
    }
    let mut token = 0u32;
    let mut manager = None;
    MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
    let manager = manager.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
    manager.ResetDevice(&device, token)?;
    Ok(manager)
}

#[implement(IMFMediaEngineNotify)]
struct Notify {
    host: isize,
}

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, _param1: usize, param2: u32) -> windows::core::Result<()> {
        let ev = event as i32;
        if ev == MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA.0 || ev == MF_MEDIA_ENGINE_EVENT_FORMATCHANGE.0 {
            crate::engine::post_player_event(self.host, EV_MF_LOADED, 0);
        } else if ev == MF_MEDIA_ENGINE_EVENT_ERROR.0 {
            // param2 is the detailed HRESULT.
            crate::engine::post_player_event(self.host, EV_MF_ERROR, param2 as i32);
        }
        Ok(())
    }
}

pub struct MfPlayer {
    host: HWND,
    engine: IMFMediaEngine,
    engine_ex: Option<IMFMediaEngineEx>,
    // Keep the callback alive for as long as the engine.
    _notify: IMFMediaEngineNotify,
    scaling: Scaling,
    size: (i32, i32),
    paused: bool,
    loaded: bool,
    volume: u8,
}

impl MfPlayer {
    pub fn new(desktop: &Desktop, rect: RECT, path: &str, volume: u8, scaling: Scaling) -> Result<MfPlayer, String> {
        startup();
        let host = host::create(desktop, rect)?;
        match unsafe { Self::create_engine(host, path, volume) } {
            Ok((engine, notify)) => {
                let engine_ex = engine.cast::<IMFMediaEngineEx>().ok();
                Ok(MfPlayer {
                    host,
                    engine,
                    engine_ex,
                    _notify: notify,
                    scaling,
                    size: rect_size(&rect),
                    paused: false,
                    loaded: false,
                    volume,
                })
            }
            Err(e) => {
                host::destroy(host);
                Err(format!("Media Foundation không mở được video: {e}"))
            }
        }
    }

    unsafe fn create_engine(host: HWND, path: &str, volume: u8) -> windows::core::Result<(IMFMediaEngine, IMFMediaEngineNotify)> {
        let factory: IMFMediaEngineClassFactory =
            CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 3)?;
        let attrs = attrs.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
        let notify: IMFMediaEngineNotify = Notify { host: host.0 as isize }.into();
        attrs.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
        attrs.SetUINT64(&MF_MEDIA_ENGINE_PLAYBACK_HWND, host.0 as u64)?;
        attrs.SetUINT32(&MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32)?;
        if let Some(m) = shared_device_manager() {
            attrs.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &m)?;
        }
        // Real-time mode: fewer buffered frames → saves ~25 MB while playing 1080p; a looping wallpaper does not need deep buffering.
        let engine = factory.CreateInstance(MF_MEDIA_ENGINE_REAL_TIME_MODE.0 as u32, &attrs)?;
        engine.SetLoop(true)?;
        engine.SetMuted(volume == 0)?;
        engine.SetVolume(volume as f64 / 100.0)?;
        engine.SetAutoPlay(true)?;
        let url = to_url(path);
        engine.SetSource(&BSTR::from(url.as_str()))?;
        engine.Play()?;
        Ok((engine, notify))
    }

    /// Enables/disables all audio streams. Fully disabled (not just muted) so the engine does not decode audio.
    fn select_audio(&self, enabled: bool) {
        let Some(ex) = &self.engine_ex else { return };
        unsafe {
            let Ok(n) = ex.GetNumberOfStreams() else { return };
            let mut changed = false;
            for i in 0..n {
                let Ok(pv) = ex.GetStreamAttribute(i, &MF_MT_MAJOR_TYPE) else { continue };
                let raw = &pv.Anonymous.Anonymous;
                // VT_CLSID = 72
                if raw.vt.0 == 72 && !raw.Anonymous.puuid.is_null() && *raw.Anonymous.puuid == MFMediaType_Audio {
                    if ex.SetStreamSelection(i, enabled).is_ok() {
                        changed = true;
                    }
                }
            }
            if changed {
                let _ = ex.ApplyStreamSelections();
                logln!("MF: {} luồng âm thanh", if enabled { "bật" } else { "tắt hẳn" });
            }
        }
    }

    fn update_stream(&self) {
        let Some(ex) = &self.engine_ex else { return };
        let mut vw = 0u32;
        let mut vh = 0u32;
        unsafe {
            if self.engine.GetNativeVideoSize(Some(&mut vw), Some(&mut vh)).is_err() || vw == 0 || vh == 0 {
                return;
            }
            let (src, dst) = super::fit_rects((vw, vh), self.size, self.scaling);
            let nrect = MFVideoNormalizedRect { left: src[0], top: src[1], right: src[2], bottom: src[3] };
            let black = MFARGB { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbAlpha: 255 };
            if let Err(e) = ex.UpdateVideoStream(Some(&nrect), Some(&dst), Some(&black)) {
                logln!("UpdateVideoStream lỗi: {e}");
            }
        }
    }
}

/// The Media Engine takes a URL; Windows paths are converted to file:///.
fn to_url(path: &str) -> String {
    if path.contains("://") {
        return path.to_string();
    }
    let p = path.replace('\\', "/");
    let mut out = String::from("file:///");
    for ch in p.chars() {
        match ch {
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            '%' => out.push_str("%25"),
            '?' => out.push_str("%3F"),
            c => out.push(c),
        }
    }
    out
}

impl Player for MfPlayer {
    fn backend(&self) -> &'static str {
        "media-foundation"
    }

    fn host(&self) -> Option<HWND> {
        Some(self.host)
    }

    fn set_paused(&mut self, paused: bool, _reason: PauseReason) {
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        unsafe {
            let _ = if paused { self.engine.Pause() } else { self.engine.Play() };
        }
    }

    fn set_volume(&mut self, volume: u8) {
        if self.loaded && (self.volume == 0) != (volume == 0) {
            self.select_audio(volume > 0);
        }
        self.volume = volume;
        unsafe {
            let _ = self.engine.SetMuted(volume == 0);
            let _ = self.engine.SetVolume(volume as f64 / 100.0);
        }
    }

    fn resize(&mut self, rect: RECT) {
        host::move_to(self.host, rect);
        self.size = rect_size(&rect);
        self.update_stream();
    }

    fn on_event(&mut self, code: u32, param: isize) -> Result<(), String> {
        match code {
            EV_MF_LOADED => {
                if !self.loaded {
                    self.loaded = true;
                    let (mut w, mut h) = (0u32, 0u32);
                    unsafe {
                        let _ = self.engine.GetNativeVideoSize(Some(&mut w), Some(&mut h));
                    }
                    logln!("MF đã mở video {w}x{h}");
                    if self.volume == 0 {
                        self.select_audio(false);
                    }
                }
                self.update_stream();
                Ok(())
            }
            EV_MF_ERROR => Err(format!("Media Foundation báo lỗi khi phát (HRESULT 0x{:08X})", param as i32 as u32)),
            _ => Ok(()),
        }
    }

    fn close(&mut self) {
        unsafe {
            let _ = self.engine.Shutdown();
        }
        host::destroy(self.host);
    }
}

#[cfg(test)]
mod tests {
    use super::to_url;

    #[test]
    fn converts_paths_to_file_urls() {
        assert_eq!(to_url(r"C:\Videos\my clip#1.mp4"), "file:///C:/Videos/my%20clip%231.mp4");
        assert_eq!(to_url("https://x/y.mp4"), "https://x/y.mp4");
    }
}
