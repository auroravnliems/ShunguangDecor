//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! A layered window holding a UI tree: the bar, a popup or a tooltip.
//! Handles hover/pressed, color transitions (`transition: background-color 0.1s`) and the fade effect.

use std::collections::HashMap;
use std::time::Instant;

use windows::Win32::Foundation::HWND;

use crate::engine::css::{state, Color, Style, Stylesheet};
use crate::engine::gfx::{Gfx, Surface};
use crate::engine::ui::layout;
use crate::engine::ui::tree::*;
use crate::engine::win::MouseBtn;

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

pub fn ease_in_cubic(t: f32) -> f32 {
    t * t * t
}

pub struct Fade {
    pub from: f32,
    pub to: f32,
    pub start: Instant,
    pub ms: f32,
    pub ease_out: bool,
    /// slide effect (tooltip): y offset at the start → at the end
    pub slide: Option<(f32, f32)>,
}

struct Trans {
    from: Color,
    to: Color,
    start: Instant,
    ms: f32,
    /// 3 = image zoom (slight bounce, easeOutBack style), everything else uses easeOutCubic
    slot: u8,
}

/// easeOutBack: overshoots the target slightly and then settles back — fits icon zoom.
pub fn ease_out_back(t: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

pub struct Layer {
    pub hwnd: HWND,
    pub scale: f32,
    pub pos: (i32, i32),
    pub size: (i32, i32),
    surf: Option<Surface>,
    pub tree: Tree,
    /// mouse coordinates (logical px inside the window)
    pub mouse: Option<(f32, f32)>,
    pub hover: Vec<NodeId>,
    pub pressed: Option<(u64, MouseBtn)>,
    pub drag_slider: Option<u64>,
    pub alpha: f32,
    pub fade: Option<Fade>,
    pub slide: f32,
    trans: HashMap<(u64, u8), Trans>,
    pub dirty: bool,
    pub visible: bool,
    pub animating: bool,
}

impl Layer {
    pub fn new(hwnd: HWND, scale: f32) -> Layer {
        Layer {
            hwnd,
            scale,
            pos: (0, 0),
            size: (1, 1),
            surf: None,
            tree: Tree::default(),
            mouse: None,
            hover: Vec::new(),
            pressed: None,
            drag_slider: None,
            alpha: 1.0,
            fade: None,
            slide: 0.0,
            trans: HashMap::new(),
            dirty: true,
            visible: false,
            animating: false,
        }
    }

    pub fn node_by_key(&self, key: u64) -> Option<NodeId> {
        self.tree.nodes.iter().position(|n| n.key == key)
    }

    /// Chain of nodes under the mouse (from deepest up to the root).
    pub fn chain_at(&self, x: f32, y: f32) -> Vec<NodeId> {
        match self.tree.hit(x, y) {
            Some(n) => self.tree.chain(n),
            None => Vec::new(),
        }
    }

    /// Computes style + layout. `fixed` = fixed size (bar); None = sized by content (popup/tooltip).
    pub fn layout(&mut self, gfx: &Gfx, sheet: &Stylesheet, root_style: &Style, fixed: Option<(f32, f32)>) -> (f32, f32) {
        let n = self.tree.nodes.len();
        if n == 0 {
            return (0.0, 0.0);
        }
        self.tree.dyn_states = vec![0; n];
        let size = self.pass(gfx, sheet, root_style, fixed);
        // hover/pressed from the mouse position over the layout just computed
        let chain = match self.mouse {
            Some((x, y)) => self.chain_at(x, y),
            None => Vec::new(),
        };
        let pressed_chain: Vec<NodeId> = match self.pressed.and_then(|(k, _)| self.node_by_key(k)) {
            Some(p) => self.tree.chain(p),
            None => Vec::new(),
        };
        let mut changed = false;
        for &c in &chain {
            self.tree.dyn_states[c] |= state::HOVER;
            // QMenu: the item under the mouse is in the :selected state
            if self.tree.nodes[c].tag == "QMenuItem" && self.tree.nodes[c].states & state::DISABLED == 0 {
                self.tree.dyn_states[c] |= state::SELECTED;
            }
            changed = true;
            if pressed_chain.contains(&c) {
                self.tree.dyn_states[c] |= state::PRESSED;
            }
            // power menu: the "hover" class is added when the mouse enters the button frame and all child labels
            if self.tree.nodes[c].hover_class {
                self.add_hover_class(c);
            }
        }
        if let Some(ds) = self.drag_slider.and_then(|k| self.node_by_key(k)) {
            self.tree.dyn_states[ds] |= state::PRESSED | state::HOVER;
            changed = true;
        }
        let size = if changed { self.pass(gfx, sheet, root_style, fixed) } else { size };
        self.hover = chain;
        size
    }

    fn add_hover_class(&mut self, id: NodeId) {
        let mut stack = vec![id];
        while let Some(i) = stack.pop() {
            let n = &mut self.tree.nodes[i];
            if (i == id || n.tag == "QLabel")
                && !n.classes.split_whitespace().any(|c| c == "hover") {
                    n.classes.push_str(" hover");
                }
            stack.extend(n.children.iter().copied());
        }
    }

    fn pass(&mut self, gfx: &Gfx, sheet: &Stylesheet, root_style: &Style, fixed: Option<(f32, f32)>) -> (f32, f32) {
        layout::compute_styles(&mut self.tree, sheet, root_style);
        layout::measure(&mut self.tree, gfx);
        let (hw, hh) = layout::root_hint(&self.tree);
        let (w, h) = fixed.unwrap_or((hw.ceil(), hh.ceil()));
        layout::arrange(&mut self.tree, 0, Rect::new(0.0, 0.0, w, h));
        (w, h)
    }

    /// Updates the color transitions; returns true if any is still running.
    fn update_transitions(&mut self, now: Instant) -> bool {
        let mut active = false;
        let mut seen = std::collections::HashSet::new();
        for n in &self.tree.nodes {
            // progress bar values always slide smoothly (350ms) if animation is on
            let progress = match &n.content {
                Content::Progress(p) if p.animate => Some(p.value),
                _ => None,
            };
            if n.style.transitions.is_empty() && progress.is_none() {
                continue;
            }
            // slots 3 and 4 hold floats (packed into the r channel of Color) so they share the interpolation mechanism
            for (slot, prop) in [(0u8, "background-color"), (1u8, "color"), (2u8, "border-color"), (3u8, "-tachyon-image-scale"), (4u8, "progress")] {
                let ms = if slot == 4 {
                    if progress.is_none() {
                        continue;
                    }
                    350.0
                } else {
                    let found = n.style.transition_ms(prop).or_else(|| match slot {
                        0 => n.style.transition_ms("background"),
                        2 => n.style.transition_ms("border"),
                        _ => None,
                    });
                    match found {
                        Some(ms) => ms,
                        None => continue,
                    }
                };
                let target = match slot {
                    0 => n.style.background.unwrap_or(Color::TRANSPARENT),
                    1 => n.style.color,
                    2 => n.style.border_color[0],
                    3 => Color { r: n.style.image_scale, g: 0.0, b: 0.0, a: 1.0 },
                    _ => Color { r: progress.unwrap_or(0.0), g: 0.0, b: 0.0, a: 1.0 },
                };
                let key = (n.key, slot);
                seen.insert(key);
                match self.trans.get_mut(&key) {
                    None => {
                        self.trans.insert(key, Trans { from: target, to: target, start: now, ms, slot });
                    }
                    Some(t) => {
                        if t.to != target {
                            let cur = Self::value(t, now);
                            t.from = if cur.a <= 0.001 && slot < 3 { target.with_alpha(0.0) } else { cur };
                            t.to = target;
                            t.start = now;
                            t.ms = ms;
                        }
                        if now.duration_since(t.start).as_secs_f32() * 1000.0 < t.ms {
                            active = true;
                        }
                    }
                }
            }
        }
        self.trans.retain(|k, _| seen.contains(k));
        active
    }

    fn value(t: &Trans, now: Instant) -> Color {
        if t.ms <= 0.0 {
            return t.to;
        }
        let p = (now.duration_since(t.start).as_secs_f32() * 1000.0 / t.ms).clamp(0.0, 1.0);
        if t.slot >= 3 {
            // the float lives in the r channel: interpolate by hand so easeOutBack can exceed 1
            let e = if t.slot == 3 { ease_out_back(p) } else { ease_out_cubic(p) };
            return Color { r: t.from.r + (t.to.r - t.from.r) * e, g: 0.0, b: 0.0, a: 1.0 };
        }
        let to = if t.to.a <= 0.001 { t.from.with_alpha(0.0) } else { t.to };
        t.from.lerp(to, ease_out_cubic(p))
    }

    /// Updates the opacity (fade); returns true if still running.
    pub fn update_fade(&mut self, now: Instant) -> bool {
        let Some(f) = &self.fade else { return false };
        let p = if f.ms <= 0.0 { 1.0 } else { (now.duration_since(f.start).as_secs_f32() * 1000.0 / f.ms).clamp(0.0, 1.0) };
        let e = if f.ease_out { ease_out_cubic(p) } else { ease_in_cubic(p) };
        self.alpha = f.from + (f.to - f.from) * e;
        if let Some((a, b)) = f.slide {
            self.slide = a + (b - a) * e;
        }
        if p >= 1.0 {
            self.fade = None;
            return false;
        }
        true
    }

    /// Draws and pushes to the screen.
    pub fn present(&mut self, gfx: &Gfx) {
        let now = Instant::now();
        let anim = self.update_transitions(now);
        if self.surf.as_ref().map(|s| (s.w, s.h)) != Some(self.size) {
            self.surf = Some(Surface::new(self.size.0, self.size.1));
        }
        let surf = self.surf.as_ref().unwrap();
        let trans = &self.trans;
        let colors = |_: usize, n: &Node| -> crate::engine::gfx::Dyn {
            let get = |slot: u8| trans.get(&(n.key, slot)).map(|t| Self::value(t, now));
            crate::engine::gfx::Dyn {
                bg: get(0).or(n.style.background),
                fg: get(1).unwrap_or(n.style.color),
                border: get(2),
                scale: get(3).map(|c| c.r).unwrap_or(n.style.image_scale),
                value: get(4).map(|c| c.r),
            }
        };
        let tree = &self.tree;
        let pos = (self.pos.0, self.pos.1 + self.slide.round() as i32);
        gfx.present(self.hwnd, surf, pos, self.scale, (self.alpha.clamp(0.0, 1.0) * 255.0) as u8, |g| {
            g.paint_tree(tree, &colors);
        });
        self.dirty = false;
        self.animating = anim;
    }

    /// Changes only alpha/position while fading when the content has not changed.
    pub fn present_alpha_only(&mut self, gfx: &Gfx) {
        if self.slide != 0.0 || self.fade.as_ref().is_some_and(|f| f.slide.is_some()) {
            // sliding tooltip: the position must be reset → redraw
            self.present(gfx);
        } else {
            Gfx::set_window_alpha(self.hwnd, (self.alpha.clamp(0.0, 1.0) * 255.0) as u8);
        }
    }

    /// Rectangle of the node (physical px, screen coordinates).
    pub fn screen_rect(&self, id: NodeId) -> (i32, i32, i32, i32) {
        let r = self.tree.nodes[id].rect;
        let s = self.scale;
        (
            self.pos.0 + (r.x * s).round() as i32,
            self.pos.1 + (r.y * s).round() as i32,
            (r.w * s).round() as i32,
            (r.h * s).round() as i32,
        )
    }

    pub fn contains_screen(&self, x: i32, y: i32) -> bool {
        self.visible && x >= self.pos.0 && y >= self.pos.1 && x < self.pos.0 + self.size.0 && y < self.pos.1 + self.size.1
    }
}

/// A fade has just finished and its target is transparent (alpha ≤ 0) → the window must be hidden completely (`SW_HIDE`).
/// Based on the fade's target, not on comparing the current alpha float.
pub fn fade_finished_hidden(target_alpha: f32, still_running: bool) -> bool {
    !still_running && target_alpha <= 0.0
}

/// The tooltip window is still visible (WS_VISIBLE) while the state says it is hidden → the invariant is broken, hide it again.
pub fn tooltip_window_stale(shown: bool, window_visible: bool) -> bool {
    !shown && window_visible
}

#[cfg(test)]
mod hide_tests {
    use super::*;

    #[test]
    fn finished_fade_out_hides_but_fade_in_and_running_do_not() {
        assert!(fade_finished_hidden(0.0, false));
        assert!(!fade_finished_hidden(0.0, true));
        assert!(!fade_finished_hidden(1.0, false));
    }

    #[test]
    fn visible_window_with_hidden_state_is_stale() {
        assert!(tooltip_window_stale(false, true));
        assert!(!tooltip_window_stale(true, true));
        assert!(!tooltip_window_stale(false, false));
    }

    #[test]
    fn update_fade_reaches_exact_target_when_done() {
        // a fade-out starting midway through a fade-in: it must end at exactly 0 so the window hides
        let mut l = Layer::new(windows::Win32::Foundation::HWND::default(), 1.0);
        let start = Instant::now();
        l.alpha = 0.37;
        l.fade = Some(Fade { from: 0.37, to: 0.0, start, ms: 200.0, ease_out: false, slide: None });
        let running = l.update_fade(start + std::time::Duration::from_millis(250));
        assert!(!running);
        assert!(l.fade.is_none());
        assert_eq!(l.alpha, 0.0);
    }
}
