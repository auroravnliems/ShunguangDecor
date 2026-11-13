//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! Draws with Direct2D (DC render target) + DirectWrite and pushes to the layered window with UpdateLayeredWindow
//! – exactly how Qt draws transparent windows, so DWM blur/rounded corners keep working as in YASB.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::core::{w, Interface, HSTRING, PCWSTR};
use windows::core::BOOL;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};
use windows_numerics::{Matrix3x2, Vector2};

use crate::engine::css::{Color, Font, Gradient, Style};
use crate::engine::ui::layout::{draws_box, Measure};
use crate::engine::ui::tree::*;
use crate::engine::ui::tree::Progress;

fn col(c: Color, opacity: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a * opacity }
}

/// The in-motion values of a node (transition/animation) while drawing.
pub struct Dyn {
    pub bg: Option<Color>,
    pub fg: Color,
    pub border: Option<Color>,
    pub scale: f32,
    pub value: Option<f32>,
}

fn rf(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h }
}

/// Drawing surface of a window: a 32-bit DIB + a memory DC.
pub struct Surface {
    hdc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    pub w: i32,
    pub h: i32,
}

impl Surface {
    pub fn new(w: i32, h: i32) -> Surface {
        unsafe {
            let screen = GetDC(None);
            let hdc = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            let bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w.max(1),
                    biHeight: -(h.max(1)),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(hdc), &bi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();
            let old = SelectObject(hdc, bmp.into());
            Surface { hdc, bmp, old, w: w.max(1), h: h.max(1) }
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.hdc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.hdc);
        }
    }
}

#[derive(Hash, PartialEq, Eq, Clone)]
struct FontKey {
    families: String,
    size_x100: u32,
    weight: u16,
    italic: bool,
}

impl FontKey {
    fn of(f: &Font) -> FontKey {
        FontKey { families: f.families.join("|"), size_x100: (f.size * 100.0) as u32, weight: f.weight, italic: f.italic }
    }
}

type MeasureKey = (String, FontKey, u32);
type FormatEntry = (IDWriteTextFormat, Option<IDWriteFontFallback>);

pub struct Gfx {
    pub factory: ID2D1Factory1,
    rt: ID2D1DCRenderTarget,
    dwrite: IDWriteFactory2,
    collection: IDWriteFontCollection,
    sys_fallback: Option<IDWriteFontFallback>,
    brush: ID2D1SolidColorBrush,
    formats: RefCell<HashMap<FontKey, FormatEntry>>,
    measures: RefCell<HashMap<MeasureKey, (f32, f32)>>,
    bitmaps: RefCell<HashMap<u64, (ID2D1Bitmap, u64)>>,
    frame: std::cell::Cell<u64>,
    scale: std::cell::Cell<f32>,
    pub accent: Color,
    pub dark: bool,
}

impl Gfx {
    pub fn new() -> windows::core::Result<Gfx> {
        unsafe {
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = factory.CreateDCRenderTarget(&props)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE);
            rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let dwrite: IDWriteFactory2 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            // the "typographic" collection, so names like "Segoe UI Variable" can match
            let collection: IDWriteFontCollection = match dwrite.cast::<IDWriteFactory6>() {
                Ok(f6) => f6.GetSystemFontCollection(false, DWRITE_FONT_FAMILY_MODEL_TYPOGRAPHIC)?.cast()?,
                Err(_) => {
                    let mut c = None;
                    dwrite.GetSystemFontCollection(&mut c, false)?;
                    c.unwrap()
                }
            };
            let sys_fallback = dwrite.GetSystemFontFallback().ok();
            let brush = rt.CreateSolidColorBrush(&col(Color::WHITE, 1.0), None)?;
            Ok(Gfx {
                factory,
                rt,
                dwrite,
                collection,
                sys_fallback,
                brush,
                formats: RefCell::new(HashMap::new()),
                measures: RefCell::new(HashMap::new()),
                bitmaps: RefCell::new(HashMap::new()),
                frame: std::cell::Cell::new(0),
                scale: std::cell::Cell::new(1.0),
                accent: Color::rgba8(0x4c, 0xc2, 0xff, 255),
                dark: true,
            })
        }
    }

    fn family_exists(&self, name: &str) -> bool {
        unsafe {
            let mut idx = 0u32;
            let mut exists = BOOL(0);
            self.collection.FindFamilyName(&HSTRING::from(name), &mut idx, &mut exists).is_ok() && exists.as_bool()
        }
    }

    /// Text format for a font stack; the first family present on the machine is primary, the rest are fallbacks.
    fn format(&self, f: &Font) -> (IDWriteTextFormat, Option<IDWriteFontFallback>) {
        let key = FontKey::of(f);
        if let Some(v) = self.formats.borrow().get(&key) {
            return v.clone();
        }
        let existing: Vec<&String> = f.families.iter().filter(|n| self.family_exists(n)).collect();
        let primary = existing.first().map(|s| s.as_str()).unwrap_or("Segoe UI");
        let weight = DWRITE_FONT_WEIGHT(f.weight as i32);
        let style = if f.italic { DWRITE_FONT_STYLE_ITALIC } else { DWRITE_FONT_STYLE_NORMAL };
        let format = unsafe {
            self.dwrite
                .CreateTextFormat(
                    &HSTRING::from(primary),
                    &self.collection,
                    weight,
                    style,
                    DWRITE_FONT_STRETCH_NORMAL,
                    f.size,
                    w!("en-us"),
                )
                .or_else(|_| {
                    self.dwrite.CreateTextFormat(w!("Segoe UI"), None, weight, style, DWRITE_FONT_STRETCH_NORMAL, f.size, w!("en-us"))
                })
                .expect("CreateTextFormat")
        };
        unsafe {
            let _ = format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        }
        let fallback = if existing.len() > 1 {
            unsafe {
                self.dwrite.CreateFontFallbackBuilder().ok().and_then(|b| {
                    let range = [DWRITE_UNICODE_RANGE { first: 0, last: 0x10FFFF }];
                    for fam in existing.iter().skip(1) {
                        let wide: Vec<u16> = fam.encode_utf16().chain(Some(0)).collect();
                        let names = [wide.as_ptr()];
                        let _ = b.AddMapping(&range, &names, &self.collection, PCWSTR::null(), PCWSTR::null(), 1.0);
                    }
                    if let Some(sys) = &self.sys_fallback {
                        let _ = b.AddMappings(sys);
                    }
                    b.CreateFontFallback().ok()
                })
            }
        } else {
            None
        };
        let v = (format, fallback);
        self.formats.borrow_mut().insert(key, v.clone());
        v
    }

    fn layout(&self, text: &str, f: &Font, wrap_w: Option<f32>) -> Option<IDWriteTextLayout> {
        let (format, fallback) = self.format(f);
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let l = self.dwrite.CreateTextLayout(&wide, &format, wrap_w.unwrap_or(100_000.0), 100_000.0).ok()?;
            if wrap_w.is_some() {
                let _ = l.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            }
            if let (Some(fb), Ok(l2)) = (fallback, l.cast::<IDWriteTextLayout2>()) {
                let _ = l2.SetFontFallback(&fb);
            }
            Some(l)
        }
    }

    // ------------------------------------------------------------------ drawing

    /// Draws the whole tree onto the surface, then pushes it to the window.
    pub fn present(&self, hwnd: HWND, surf: &Surface, pos: (i32, i32), scale: f32, alpha: u8, draw: impl FnOnce(&Gfx)) {
        self.frame.set(self.frame.get() + 1);
        self.scale.set(scale);
        unsafe {
            let rc = RECT { left: 0, top: 0, right: surf.w, bottom: surf.h };
            if self.rt.BindDC(surf.hdc, &rc).is_err() {
                return;
            }
            self.rt.BeginDraw();
            self.rt.SetDpi(96.0 * scale, 96.0 * scale);
            self.rt.SetTransform(&Matrix3x2::identity());
            self.rt.Clear(Some(&col(Color::TRANSPARENT, 1.0)));
            draw(self);
            if let Err(e) = self.rt.EndDraw(None, None) {
                crate::engine::log::error(&format!("EndDraw lỗi: {e}"));
            }
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
            let pt = POINT { x: pos.0, y: pos.1 };
            let size = SIZE { cx: surf.w, cy: surf.h };
            let src = POINT { x: 0, y: 0 };
            let _ = UpdateLayeredWindow(hwnd, None, Some(&pt), Some(&size), Some(surf.hdc), Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA);
        }
        // clean up bitmaps unused for ~600 frames
        let f = self.frame.get();
        if f.is_multiple_of(300) {
            self.bitmaps.borrow_mut().retain(|_, (_, last)| f - *last < 600);
        }
    }

    /// Only changes the opacity of the whole window (fade effect), without redrawing.
    pub fn set_window_alpha(hwnd: HWND, alpha: u8) {
        unsafe {
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = UpdateLayeredWindow(hwnd, None, None, None, None, None, COLORREF(0), Some(&blend), ULW_ALPHA);
        }
    }

    fn set_color(&self, c: D2D1_COLOR_F) {
        unsafe { self.brush.SetColor(&c) }
    }

    fn rounded_path(&self, r: Rect, rad: [f32; 4]) -> Option<ID2D1PathGeometry> {
        unsafe {
            let g = self.factory.CreatePathGeometry().ok()?;
            let s = g.Open().ok()?;
            let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
            let [tl, tr, br, bl] = rad;
            let arc = |x: f32, y: f32, rr: f32| D2D1_ARC_SEGMENT {
                point: Vector2 { X: x, Y: y },
                size: D2D_SIZE_F { width: rr, height: rr },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            };
            s.BeginFigure(Vector2 { X: x0 + tl, Y: y0 }, D2D1_FIGURE_BEGIN_FILLED);
            s.AddLine(Vector2 { X: x1 - tr, Y: y0 });
            if tr > 0.0 {
                s.AddArc(&arc(x1, y0 + tr, tr));
            }
            s.AddLine(Vector2 { X: x1, Y: y1 - br });
            if br > 0.0 {
                s.AddArc(&arc(x1 - br, y1, br));
            }
            s.AddLine(Vector2 { X: x0 + bl, Y: y1 });
            if bl > 0.0 {
                s.AddArc(&arc(x0, y1 - bl, bl));
            }
            s.AddLine(Vector2 { X: x0, Y: y0 + tl });
            if tl > 0.0 {
                s.AddArc(&arc(x0 + tl, y0, tl));
            }
            s.EndFigure(D2D1_FIGURE_END_CLOSED);
            s.Close().ok()?;
            g.cast().ok()
        }
    }

    fn clamp_radius(r: Rect, rad: [f32; 4]) -> [f32; 4] {
        let m = (r.w.min(r.h) / 2.0).max(0.0);
        rad.map(|x| x.clamp(0.0, m))
    }

    fn gradient_brush(&self, g: &Gradient, r: Rect, opacity: f32) -> Option<ID2D1Brush> {
        unsafe {
            let stops: Vec<D2D1_GRADIENT_STOP> = g.stops.iter().map(|(p, c)| D2D1_GRADIENT_STOP { position: *p, color: col(*c, opacity) }).collect();
            let coll = self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok()?;
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: Vector2 { X: r.x + g.x1 * r.w, Y: r.y + g.y1 * r.h },
                endPoint: Vector2 { X: r.x + g.x2 * r.w, Y: r.y + g.y2 * r.h },
            };
            self.rt.CreateLinearGradientBrush(&props, None, &coll).ok().map(|b| b.into())
        }
    }

    /// Fills a rounded rectangle with any brush.
    fn fill_shape(&self, r: Rect, rad: [f32; 4], brush: &ID2D1Brush) {
        let uniform = rad.iter().all(|&x| (x - rad[0]).abs() < 0.01);
        unsafe {
            if rad[0] <= 0.0 && uniform {
                self.rt.FillRectangle(&rf(r), brush);
            } else if uniform {
                let rr = D2D1_ROUNDED_RECT { rect: rf(r), radiusX: rad[0], radiusY: rad[0] };
                self.rt.FillRoundedRectangle(&rr, brush);
            } else if let Some(g) = self.rounded_path(r, rad) {
                self.rt.FillGeometry(&g, brush, None);
            }
        }
    }

    fn stroke_shape(&self, r: Rect, rad: [f32; 4], brush: &ID2D1Brush, w: f32) {
        let uniform = rad.iter().all(|&x| (x - rad[0]).abs() < 0.01);
        unsafe {
            if uniform {
                let rr = D2D1_ROUNDED_RECT { rect: rf(r), radiusX: rad[0], radiusY: rad[0] };
                self.rt.DrawRoundedRectangle(&rr, brush, w, None);
            } else if let Some(g) = self.rounded_path(r, rad) {
                self.rt.DrawGeometry(&g, brush, w, None);
            }
        }
    }

    /// `box-shadow` halo: concentric rings that fade out (a Gaussian approximation), drawn before the background.
    fn draw_shadows(&self, r: Rect, st: &Style, rad: [f32; 4], opacity: f32) {
        for s in &st.shadows {
            if !s.color.is_visible() {
                continue;
            }
            let steps = (s.blur.max(1.0)).ceil() as i32;
            for i in (1..=steps).rev() {
                let k = i as f32;
                let t = 1.0 - (k - 1.0) / steps as f32;
                let a = s.color.a * t * t * (2.2 / steps as f32).min(0.6);
                let e = s.spread + k;
                let sr = Rect::new(r.x + s.dx - e, r.y + s.dy - e, r.w + 2.0 * e, r.h + 2.0 * e);
                let srad = rad.map(|x| x + e);
                self.set_color(col(s.color.with_alpha(a), opacity));
                self.stroke_shape(sr, srad, &self.brush.cast().unwrap(), 2.0);
            }
            if s.spread > 0.0 || s.dx != 0.0 || s.dy != 0.0 {
                let sr = Rect::new(r.x + s.dx - s.spread, r.y + s.dy - s.spread, r.w + 2.0 * s.spread, r.h + 2.0 * s.spread);
                self.set_color(col(s.color.with_alpha(s.color.a * 0.6), opacity));
                self.fill_shape(sr, rad.map(|x| x + s.spread), &self.brush.cast().unwrap());
            }
        }
    }

    /// Image from the stylesheet (absolute path) at its real pixel size.
    fn css_bitmap(&self, path: &str, w: f32, h: f32, fit: crate::engine::sys::image::Fit) -> Option<(ID2D1Bitmap, f32, f32)> {
        let s = self.scale.get();
        let (pw, ph) = ((w * s).round().max(1.0) as u32, (h * s).round().max(1.0) as u32);
        let p = crate::engine::sys::image::load_file_fit(path, pw, ph, fit)?;
        let (iw, ih) = (p.w as f32 / s, p.h as f32 / s);
        Some((self.bitmap(&p)?, iw, ih))
    }

    /// Background + border of a box (border box): shadow, color/gradient, background image, border.
    pub fn draw_box(&self, r: Rect, st: &Style, d: &Dyn, opacity: f32) {
        if r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let rad = Self::clamp_radius(r, st.radius);
        self.draw_shadows(r, st, rad, opacity);
        unsafe {
            if let Some(g) = &st.gradient {
                if let Some(b) = self.gradient_brush(g, r, opacity) {
                    self.fill_shape(r, rad, &b);
                }
            } else if let Some(c) = d.bg.filter(|c| c.is_visible()) {
                self.set_color(col(c, opacity));
                self.fill_shape(r, rad, &self.brush.cast().unwrap());
            }
            if let Some(path) = &st.background_image {
                use crate::engine::sys::image::Fit;
                let fit = match st.bg_size.as_str() {
                    "cover" => Fit::Cover((st.bg_pos.0 * 1000.0) as u16, (st.bg_pos.1 * 1000.0) as u16),
                    "contain" => Fit::Contain,
                    _ => Fit::Stretch,
                };
                if let Some((bmp, iw, ih)) = self.css_bitmap(path, r.w, r.h, fit)
                    && let Ok(brush) = self.rt.CreateBitmapBrush(&bmp, None, None) {
                        let x = r.x + (r.w - iw) * st.bg_pos.0;
                        let y = r.y + (r.h - ih) * st.bg_pos.1;
                        let s = self.scale.get();
                        brush.SetTransform(&(Matrix3x2::scale(1.0 / s, 1.0 / s) * Matrix3x2::translation(x, y)));
                        brush.SetOpacity(opacity);
                        let clip = Rect::new(x.max(r.x), y.max(r.y), iw.min(r.w), ih.min(r.h));
                        self.fill_shape(if fit == Fit::Contain { clip } else { r }, rad, &brush.into());
                    }
            }
            let b = st.border;
            if b.top + b.right + b.bottom + b.left <= 0.0 {
                return;
            }
            let same_w = b.top == b.right && b.top == b.bottom && b.top == b.left;
            let same_c = st.border_color.iter().all(|c| *c == st.border_color[0]);
            if same_w && (same_c || st.border_gradient.is_some()) {
                let half = b.top / 2.0;
                let ir = Rect::new(r.x + half, r.y + half, r.w - b.top, r.h - b.top);
                let irad = rad.map(|x| (x - half).max(0.0));
                if let Some(g) = &st.border_gradient {
                    if let Some(br) = self.gradient_brush(g, r, opacity) {
                        self.stroke_shape(ir, irad, &br, b.top);
                    }
                    return;
                }
                let c = d.border.unwrap_or(st.border_color[0]);
                if !c.is_visible() {
                    return;
                }
                self.set_color(col(c, opacity));
                self.stroke_shape(ir, irad, &self.brush.cast().unwrap(), b.top);
            } else {
                // the sides differ: draw each strip (ignoring rounded corners, as Qt does when the border is uneven)
                let sides = [
                    (Rect::new(r.x, r.y, r.w, b.top), st.border_color[0]),
                    (Rect::new(r.right() - b.right, r.y, b.right, r.h), st.border_color[1]),
                    (Rect::new(r.x, r.bottom() - b.bottom, r.w, b.bottom), st.border_color[2]),
                    (Rect::new(r.x, r.y, b.left, r.h), st.border_color[3]),
                ];
                for (sr, c) in sides {
                    if sr.w > 0.0 && sr.h > 0.0 && c.is_visible() {
                        if let Some(g) = &st.border_gradient
                            && let Some(br) = self.gradient_brush(g, r, opacity) {
                                self.rt.FillRectangle(&rf(sr), &br);
                                continue;
                            }
                        self.set_color(col(c, opacity));
                        self.rt.FillRectangle(&rf(sr), &self.brush);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_text(&self, text: &str, font: &Font, color: Color, area: Rect, clip: Rect, h: Align, v: Align, wrap: bool) {
        if text.is_empty() || !color.is_visible() {
            return;
        }
        let Some(layout) = self.layout(text, font, if wrap { Some(area.w.max(1.0)) } else { None }) else { return };
        unsafe {
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_err() {
                return;
            }
            let tw = m.widthIncludingTrailingWhitespace;
            let th = m.height;
            let x = match h {
                Align::Start => area.x,
                Align::Center => area.x + (area.w - tw) / 2.0,
                Align::End => area.x + area.w - tw,
            };
            let y = match v {
                Align::Start => area.y,
                Align::Center => area.y + (area.h - th) / 2.0,
                Align::End => area.y + area.h - th,
            };
            if wrap {
                let _ = layout.SetTextAlignment(match h {
                    Align::Start => DWRITE_TEXT_ALIGNMENT_LEADING,
                    Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                    Align::End => DWRITE_TEXT_ALIGNMENT_TRAILING,
                });
            }
            self.rt.PushAxisAlignedClip(&rf(clip), D2D1_ANTIALIAS_MODE_ALIASED);
            self.set_color(col(color, 1.0));
            let ox = if wrap { area.x } else { x };
            self.rt.DrawTextLayout(Vector2 { X: ox.round(), Y: y.round() }, &layout, &self.brush, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
            self.rt.PopAxisAlignedClip();
        }
    }

    fn bitmap(&self, p: &Pixmap) -> Option<ID2D1Bitmap> {
        let f = self.frame.get();
        if let Some((b, last)) = self.bitmaps.borrow_mut().get_mut(&p.id) {
            *last = f;
            return Some(b.clone());
        }
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let b = unsafe {
            self.rt
                .CreateBitmap(D2D_SIZE_U { width: p.w, height: p.h }, Some(p.data.as_ptr() as *const _), p.w * 4, &props)
                .ok()?
        };
        self.bitmaps.borrow_mut().insert(p.id, (b.clone(), f));
        Some(b)
    }

    pub fn draw_image(&self, img: &Image, dest: Rect, opacity: f32) {
        let Some(b) = self.bitmap(&img.pix) else { return };
        unsafe {
            if img.circle {
                let Ok(brush) = self.rt.CreateBitmapBrush(&b, None, None) else { return };
                let sx = dest.w / img.pix.w as f32;
                let sy = dest.h / img.pix.h as f32;
                brush.SetTransform(&(Matrix3x2::scale(sx, sy) * Matrix3x2::translation(dest.x, dest.y)));
                brush.SetOpacity(opacity);
                let e = D2D1_ELLIPSE {
                    point: Vector2 { X: dest.x + dest.w / 2.0, Y: dest.y + dest.h / 2.0 },
                    radiusX: dest.w / 2.0,
                    radiusY: dest.h / 2.0,
                };
                self.rt.FillEllipse(&e, &brush);
            } else {
                self.rt.DrawBitmap(&b, Some(&rf(dest)), opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            }
        }
    }

    pub fn fill_round(&self, r: Rect, radius: f32, c: Color) {
        unsafe {
            self.set_color(col(c, 1.0));
            let rr = D2D1_ROUNDED_RECT { rect: rf(r), radiusX: radius, radiusY: radius };
            self.rt.FillRoundedRectangle(&rr, &self.brush);
        }
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, radius: f32, c: Color) {
        unsafe {
            self.set_color(col(c, 1.0));
            let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius };
            self.rt.FillEllipse(&e, &self.brush);
        }
    }

    // ------------------------------------------------------------------ drawing the tree

    pub fn paint_tree(&self, tree: &Tree, dynv: &dyn Fn(usize, &Node) -> Dyn) {
        if tree.nodes.is_empty() {
            return;
        }
        self.paint_node(tree, 0, dynv);
    }

    fn paint_node(&self, tree: &Tree, id: NodeId, dynv: &dyn Fn(usize, &Node) -> Dyn) {
        let n = &tree.nodes[id];
        if n.hidden {
            return;
        }
        let st = &n.style;
        let d = dynv(id, n);
        let border_box = n.rect.inset(&st.margin);
        if draws_box(n) {
            self.draw_box(border_box, st, &d, st.opacity);
        }
        let content = if crate::engine::ui::layout::layout_uses_box(n.tag) {
            let mut e = st.border;
            e.top += st.padding.top;
            e.right += st.padding.right;
            e.bottom += st.padding.bottom;
            e.left += st.padding.left;
            border_box.inset(&e)
        } else {
            border_box
        };
        // the `image:` property – drawn to fit the content box, centered, can be scaled by -tachyon-image-scale
        if let Some(path) = &st.image
            && !n.is_layout_only() && content.w > 0.0 && content.h > 0.0 {
                let k = d.scale;
                if let Some((bmp, iw, ih)) = self.css_bitmap(path, content.w * k, content.h * k, crate::engine::sys::image::Fit::Contain) {
                    let x = content.x + (content.w - iw) / 2.0;
                    let y = content.y + (content.h - ih) / 2.0;
                    unsafe {
                        self.rt.DrawBitmap(&bmp, Some(&rf(Rect::new(x, y, iw, ih))), st.opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                    }
                }
            }
        match &n.content {
            Content::Text(t) => {
                let c = d.fg.with_alpha(d.fg.a * st.opacity);
                let ind = (crate::engine::ui::layout::qlabel_indent(n, self) / 2.0).floor();
                let mut area = content;
                match n.halign {
                    Align::Start => {
                        area.x += ind;
                        area.w -= ind;
                    }
                    Align::End => area.w -= ind,
                    Align::Center => {}
                }
                self.draw_text(t, &st.font, c, area, border_box, n.halign, n.valign, n.word_wrap);
            }
            Content::Image(img) => {
                let (w, h) = (img.w * d.scale, img.h * d.scale);
                let x = match n.halign {
                    Align::Start => content.x,
                    Align::Center => content.x + (content.w - w) / 2.0,
                    Align::End => content.right() - w,
                };
                let y = content.y + (content.h - h) / 2.0;
                let dest = if d.scale == 1.0 { Rect::new(x.round(), y.round(), w, h) } else { Rect::new(x, y, w, h) };
                self.draw_image(img, dest, st.opacity);
            }
            Content::Slider { value, max } => self.paint_slider(tree, id, content, *value, *max),
            Content::Progress(p) => self.paint_progress(p, d.value.unwrap_or(p.value), content, st.opacity),
            Content::WinLogo(s) => self.paint_win_logo(*s * d.scale, content, d.fg.with_alpha(d.fg.a * st.opacity)),
            _ => {}
        }
        for &c in &n.children {
            self.paint_node(tree, c, dynv);
        }
    }

    /// Windows 11 logo: 4 squares (1/12-side gap) centered in the content box.
    fn paint_win_logo(&self, size: f32, r: Rect, color: Color) {
        let cells = win_logo_cells(size);
        let (ox, oy) = (r.x + (r.w - size) / 2.0, r.y + (r.h - size) / 2.0);
        self.set_color(col(color, 1.0));
        let brush: ID2D1Brush = self.brush.cast().unwrap();
        for c in cells {
            self.fill_shape(Rect::new(ox + c.x, oy + c.y, c.w, c.h), [0.0; 4], &brush);
        }
    }

    /// Progress bar: a circle (arc from 12 o'clock, clockwise) or a horizontal/vertical bar.
    fn paint_progress(&self, p: &Progress, value: f32, r: Rect, opacity: f32) {
        let v = (value / 100.0).clamp(0.0, 1.0);
        let main = p.colors.first().copied().unwrap_or(Color::rgba8(0, 200, 0, 255));
        let grad = |rr: Rect, horiz: bool| -> Option<ID2D1Brush> {
            if p.colors.len() < 2 {
                return None;
            }
            let n = (p.colors.len() - 1) as f32;
            let g = Gradient {
                x1: 0.0,
                y1: if horiz { 0.0 } else { 1.0 },
                x2: if horiz { 1.0 } else { 0.0 },
                y2: 0.0,
                stops: p.colors.iter().enumerate().map(|(i, c)| (i as f32 / n, *c)).collect(),
            };
            self.gradient_brush(&g, rr, opacity)
        };
        unsafe {
            match p.kind.as_str() {
                "linear_horizontal" | "linear_vertical" => {
                    let horiz = p.kind == "linear_horizontal";
                    let t = p.thickness;
                    let track = if horiz { Rect::new(r.x, r.y + (r.h - t) / 2.0, r.w, t) } else { Rect::new(r.x + (r.w - t) / 2.0, r.y, t, r.h) };
                    let rad = [p.radius.min(t / 2.0); 4];
                    self.set_color(col(p.background, opacity));
                    self.fill_shape(track, rad, &self.brush.cast().unwrap());
                    let fill = if horiz { Rect::new(track.x, track.y, track.w * v, t) } else { Rect::new(track.x, track.bottom() - track.h * v, t, track.h * v) };
                    if v > 0.0 {
                        match grad(track, horiz) {
                            Some(b) => self.fill_shape(fill, rad, &b),
                            None => {
                                self.set_color(col(main, opacity));
                                self.fill_shape(fill, rad, &self.brush.cast().unwrap());
                            }
                        }
                    }
                }
                _ => {
                    let size = r.w.min(r.h);
                    let t = p.thickness.min(size / 2.0);
                    let rad = (size - t) / 2.0;
                    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                    self.set_color(col(p.background, opacity));
                    let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: rad, radiusY: rad };
                    self.rt.DrawEllipse(&e, &self.brush, t, None);
                    if v <= 0.0 {
                        return;
                    }
                    let brush: ID2D1Brush = match grad(Rect::new(cx - rad, cy - rad, rad * 2.0, rad * 2.0), true) {
                        Some(b) => b,
                        None => {
                            self.set_color(col(main, opacity));
                            self.brush.cast().unwrap()
                        }
                    };
                    if v >= 0.999 {
                        self.rt.DrawEllipse(&e, &brush, t, None);
                        return;
                    }
                    let a0 = -std::f32::consts::FRAC_PI_2;
                    let a1 = a0 + v * std::f32::consts::TAU;
                    let Ok(g) = self.factory.CreatePathGeometry() else { return };
                    let Ok(s) = g.Open() else { return };
                    s.BeginFigure(Vector2 { X: cx + rad * a0.cos(), Y: cy + rad * a0.sin() }, D2D1_FIGURE_BEGIN_HOLLOW);
                    s.AddArc(&D2D1_ARC_SEGMENT {
                        point: Vector2 { X: cx + rad * a1.cos(), Y: cy + rad * a1.sin() },
                        size: D2D_SIZE_F { width: rad, height: rad },
                        rotationAngle: 0.0,
                        sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                        arcSize: if v > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                    });
                    s.EndFigure(D2D1_FIGURE_END_OPEN);
                    let _ = s.Close();
                    let base: ID2D1Factory = self.factory.cast().unwrap();
                    let style = base
                        .CreateStrokeStyle(
                            &D2D1_STROKE_STYLE_PROPERTIES {
                                startCap: D2D1_CAP_STYLE_ROUND,
                                endCap: D2D1_CAP_STYLE_ROUND,
                                dashCap: D2D1_CAP_STYLE_ROUND,
                                lineJoin: D2D1_LINE_JOIN_ROUND,
                                miterLimit: 1.0,
                                dashStyle: D2D1_DASH_STYLE_SOLID,
                                dashOffset: 0.0,
                            },
                            None,
                        )
                        .ok();
                    self.rt.DrawGeometry(&g, &brush, t, style.as_ref());
                }
            }
        }
    }

    /// QSlider: if the stylesheet has ::groove/::handle, follow the stylesheet; otherwise draw it Windows 11 style.
    fn paint_slider(&self, tree: &Tree, id: NodeId, r: Rect, value: i32, max: i32) {
        let n = &tree.nodes[id];
        let frac = if max > 0 { (value as f32 / max as f32).clamp(0.0, 1.0) } else { 0.0 };
        let states = n.states | tree.dyn_states.get(id).copied().unwrap_or(0);
        let disabled = states & crate::engine::css::state::DISABLED != 0;
        let hover = states & crate::engine::css::state::HOVER != 0;
        let pressed = states & crate::engine::css::state::PRESSED != 0;
        // measured from Qt 6.10's windows11 QSlider: 4px groove inset by 2px, handle radius 7 (max 8)
        let cy = r.y + r.h / 2.0;
        let outer_r = (r.h.min(16.0) / 2.0 - 1.0).clamp(4.0, 8.0);
        let (g0, g1) = (r.x + 2.0, r.right() - 2.0);
        let (h0, h1) = (r.x + outer_r, r.right() - outer_r);
        let hx = h0 + (h1 - h0).max(0.0) * frac;
        let accent = if disabled { Color::rgba8(0x9a, 0x9a, 0x9a, 255).with_alpha(0.4) } else { self.accent };
        let rail = if self.dark { Color::rgba8(255, 255, 255, 147) } else { Color::rgba8(0, 0, 0, 114) };
        self.fill_round(Rect::new(g0, cy - 2.0, (g1 - g0).max(0.0), 4.0), 2.0, rail);
        self.fill_round(Rect::new(g0, cy - 2.0, (hx - g0).max(0.0), 4.0), 2.0, accent);
        let outer = if self.dark { Color::rgba8(0x45, 0x45, 0x45, 255) } else { Color::WHITE };
        self.fill_circle(hx, cy, outer_r, outer);
        let k = if pressed { 0.36 } else if hover { 0.57 } else { 0.43 };
        self.fill_circle(hx, cy, outer_r * k, accent);
    }
}

impl Measure for Gfx {
    fn text_size(&self, text: &str, font: &Font, wrap_w: Option<f32>) -> (f32, f32) {
        let key = (text.to_string(), FontKey::of(font), wrap_w.map(|w| w as u32).unwrap_or(0));
        if let Some(v) = self.measures.borrow().get(&key) {
            return *v;
        }
        let t = if text.is_empty() { " " } else { text };
        let v = match self.layout(t, font, wrap_w) {
            Some(l) => unsafe {
                let mut m = DWRITE_TEXT_METRICS::default();
                let _ = l.GetMetrics(&mut m);
                (if text.is_empty() { 0.0 } else { m.widthIncludingTrailingWhitespace.ceil() }, m.height.ceil())
            },
            None => (0.0, font.size * 1.33),
        };
        let mut c = self.measures.borrow_mut();
        if c.len() > 2000 {
            c.clear();
        }
        c.insert(key, v);
        v
    }
}

/// The 4 squares of the Windows logo in a `size` box (coordinates relative to the top-left corner): top-left, top-right,
/// bottom-left, bottom-right; the gap between squares = 1/12 of the side.
fn win_logo_cells(size: f32) -> [Rect; 4] {
    let gap = size / 12.0;
    let cell = (size - gap) / 2.0;
    let far = cell + gap;
    [Rect::new(0.0, 0.0, cell, cell), Rect::new(far, 0.0, cell, cell), Rect::new(0.0, far, cell, cell), Rect::new(far, far, cell, cell)]
}

#[cfg(test)]
mod tests {
    use super::win_logo_cells;

    #[test]
    fn win_logo_is_four_equal_squares_inside_box() {
        let c = win_logo_cells(24.0);
        assert!(c.iter().all(|r| (r.w - r.h).abs() < 1e-4 && (r.w - c[0].w).abs() < 1e-4));
        assert!((c[1].x + c[1].w - 24.0).abs() < 1e-4 && (c[3].y + c[3].h - 24.0).abs() < 1e-4);
        assert!(c[0].x + c[0].w < c[1].x, "phải có khe giữa các ô");
    }
}
