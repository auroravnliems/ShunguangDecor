//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! Computes styles then lays out the way QBoxLayout does: each node has a size hint
//! (content + padding + border + margin), and the leftover space is divided "fairly" among the stretchable nodes.

use crate::engine::css::{Edges, Font, Style, Stylesheet};

use super::tree::*;

pub trait Measure {
    /// Text size (width, height) in logical px; `wrap_w` enables line wrapping.
    fn text_size(&self, text: &str, font: &Font, wrap_w: Option<f32>) -> (f32, f32);
}

/// A "bare" QWidget tag does not use padding/border for layout on its own (only QFrame and its subclasses do).
pub fn layout_uses_box(tag: &str) -> bool {
    !matches!(tag, "QWidget" | "Widget" | "PopupWidget")
}

/// Whether the node paints a background/border (Python subclasses of QWidget do not paint stylesheet backgrounds).
pub fn draws_box(n: &Node) -> bool {
    !n.is_layout_only() && n.tag != "Widget"
}

pub fn compute_styles(tree: &mut Tree, sheet: &Stylesheet, root_style: &Style) {
    let n = tree.nodes.len();
    let mut styles: Vec<Style> = Vec::with_capacity(n);
    for i in 0..n {
        let parent = tree.nodes[i].parent;
        let pstyle = match parent {
            Some(p) => &styles[p],
            None => root_style,
        };
        let node = &tree.nodes[i];
        let st = if node.is_layout_only() {
            Style::inherit_from(pstyle)
        } else if let (Some(el), Some(mut p)) = (node.sub_element, parent) {
            // `::item` belongs to the nearest real widget (skipping nested layouts)
            while tree.nodes[p].is_layout_only() {
                match tree.nodes[p].parent {
                    Some(pp) => p = pp,
                    None => break,
                }
            }
            let states = node.states | tree.dyn_states.get(i).copied().unwrap_or(0);
            let pref = NodeRef { tree, id: p, sub: Some(states) };
            let mut s = sheet.compute(pref, Some(el), &styles[p]);
            // a disabled QMenu item uses the "disabled" text color if the stylesheet does not set one explicitly
            if states & crate::engine::css::state::DISABLED != 0 && states & crate::engine::css::state::SELECTED == 0 && tree.nodes[i].tag == "QMenuItem" {
                s.color.a *= 0.4;
            }
            s
        } else {
            sheet.compute(tree.elem(i), None, pstyle)
        };
        styles.push(st);
    }
    for (node, st) in tree.nodes.iter_mut().zip(styles) {
        node.style = st;
    }
}

/// Insets between the outer box (margin box) and the node's content box.
pub fn content_insets(n: &Node) -> Edges {
    let s = &n.style;
    let mut e = s.margin;
    let lm = n.layout_margins;
    e.top += lm.top;
    e.right += lm.right;
    e.bottom += lm.bottom;
    e.left += lm.left;
    if layout_uses_box(n.tag) {
        e.top += s.border.top + s.padding.top;
        e.right += s.border.right + s.padding.right;
        e.bottom += s.border.bottom + s.padding.bottom;
        e.left += s.border.left + s.padding.left;
    }
    e
}

fn clamp_opt(v: f32, lo: Option<f32>, hi: Option<f32>) -> f32 {
    let mut v = v;
    if let Some(h) = hi {
        v = v.min(h);
    }
    if let Some(l) = lo {
        v = v.max(l);
    }
    v
}

/// Limits (min, max) in terms of the outer box size.
pub fn outer_limits(n: &Node) -> ((f32, f32), (f32, f32)) {
    let ins = content_insets(n);
    let s = &n.style;
    let (bx, by) = if layout_uses_box(n.tag) {
        (s.padding.h() + s.border.h(), s.padding.v() + s.border.v())
    } else {
        (0.0, 0.0)
    };
    let mx = s.margin.h();
    let my = s.margin.v();
    let _ = ins;
    let min_w = s.min_w.map(|v| v + bx + mx).unwrap_or(0.0);
    let max_w = s.max_w.map(|v| v + bx + mx).unwrap_or(f32::INFINITY);
    let min_h = s.min_h.map(|v| v + by + my).unwrap_or(0.0);
    let max_h = s.max_h.map(|v| v + by + my).unwrap_or(f32::INFINITY);
    let (mut min_w, mut max_w, mut min_h, mut max_h) = (min_w, max_w, min_h, max_h);
    if let Some(w) = n.fixed.0 {
        min_w = w + mx;
        max_w = w + mx;
    }
    if let Some(h) = n.fixed.1 {
        min_h = h + my;
        max_h = h + my;
    }
    ((min_w, max_w.max(min_w)), (min_h, max_h.max(min_h)))
}

/// Horizontal windows11-style QSlider: sizeHint 84×16.
pub fn slider_hint() -> (f32, f32) {
    (84.0, 16.0)
}

/// A QLabel with a stylesheet box (padding/border/margin) is considered by Qt to have a frame: when left/right aligned,
/// the size hint gains the width of the letter 'x' and the text is pushed in by half of that (QLabelPrivate::sizeForWidth).
pub fn qlabel_indent(n: &Node, m: &dyn Measure) -> f32 {
    if n.tag != "QLabel" || n.halign == Align::Center || !matches!(n.content, Content::Text(_)) {
        return 0.0;
    }
    let s = &n.style;
    let boxed = s.padding.h() + s.padding.v() + s.border.h() + s.border.v() + s.margin.h() + s.margin.v() > 0.0;
    if !boxed {
        return 0.0;
    }
    m.text_size("x", &s.font, None).0.round()
}

/// Computes size hints bottom-up (a child node always has a larger index than its parent).
pub fn measure(tree: &mut Tree, m: &dyn Measure) {
    for i in (0..tree.nodes.len()).rev() {
        let n = &tree.nodes[i];
        let (cw, ch) = match &n.content {
            Content::Text(t) => {
                let (w, h) = m.text_size(t, &n.style.font, None);
                let w = if t.is_empty() { 0.0 } else { w };
                (w + qlabel_indent(n, m), h)
            }
            Content::Image(img) => (img.w, img.h),
            Content::Slider { .. } => slider_hint(),
            Content::Progress(p) => (p.thickness * 2.0, p.thickness * 2.0),
            Content::WinLogo(s) => (*s, *s),
            Content::HLine => (0.0, 1.0),
            Content::Stretch(_) => (0.0, 0.0),
            Content::Spacing(px) => {
                let horiz = n.parent.map(|p| tree.nodes[p].dir == Dir::Row).unwrap_or(true);
                if horiz {
                    (*px, 0.0)
                } else {
                    (0.0, *px)
                }
            }
            Content::None => {
                let mut w: f32 = 0.0;
                let mut h: f32 = 0.0;
                for &c in &n.children {
                    let cn = &tree.nodes[c];
                    if cn.hidden {
                        continue;
                    }
                    let (hw, hh) = cn.hint;
                    if n.dir == Dir::Row {
                        w += hw;
                        h = h.max(hh);
                    } else {
                        h += hh;
                        w = w.max(hw);
                    }
                }
                (w, h)
            }
        };
        // a label that has child content (rare) still adds the children in
        let (cw, ch) = if !matches!(n.content, Content::None) && !n.children.is_empty() {
            let mut w = cw;
            let mut h = ch;
            for &c in &n.children {
                let (hw, hh) = tree.nodes[c].hint;
                w += hw;
                h = h.max(hh);
            }
            (w, h)
        } else {
            (cw, ch)
        };
        let ins = content_insets(n);
        let ((minw, maxw), (minh, maxh)) = outer_limits(n);
        let w = clamp_opt(cw + ins.h(), Some(minw), Some(maxw));
        let h = clamp_opt(ch + ins.v(), Some(minh), Some(maxh));
        tree.nodes[i].hint = (w, h);
    }
}

/// Length for each item along the main axis, like Qt's qGeomCalc (the leftover is divided evenly; whoever needs more keeps its hint).
fn distribute(items: &[(f32, f32, f32, u32)], avail: f32) -> Vec<f32> {
    // (hint, min, max, stretch: 0 = no stretch, 1 = preferred, 2 = expanding, 1000+ = stretch)
    let mut sizes: Vec<f32> = items.iter().map(|it| it.0).collect();
    let total: f32 = sizes.iter().sum();
    let extra = avail - total;
    if extra <= 0.0 || items.is_empty() {
        return sizes;
    }
    let top = items.iter().map(|it| it.3).max().unwrap_or(0);
    if top == 0 {
        return sizes;
    }
    let mut growers: Vec<usize> = (0..items.len()).filter(|&i| items[i].3 == top).collect();
    let fixed_sum: f32 = (0..items.len()).filter(|i| !growers.contains(i)).map(|i| sizes[i]).sum();
    let mut pool = avail - fixed_sum;
    // divide fairly: whoever has a hint larger than its share keeps the hint and leaves the group
    loop {
        if growers.is_empty() {
            break;
        }
        let share = pool / growers.len() as f32;
        let mut changed = false;
        growers.retain(|&i| {
            let (hint, _min, max, _) = items[i];
            if hint > share {
                sizes[i] = hint;
                pool -= hint;
                changed = true;
                false
            } else if max < share {
                sizes[i] = max;
                pool -= max;
                changed = true;
                false
            } else {
                true
            }
        });
        if !changed {
            for &i in &growers {
                sizes[i] = share;
            }
            break;
        }
    }
    sizes
}

fn grow_class(n: &Node, horizontal: bool) -> u32 {
    if let Content::Stretch(f) = n.content {
        return 1000 + f as u32;
    }
    let p = if horizontal { n.hpolicy } else { n.vpolicy };
    match p {
        Policy::Fixed => 0,
        Policy::Preferred => 1,
        Policy::Expanding => 2,
    }
}

/// Assigns a rectangle (margin box) to the node, then lays out its children.
pub fn arrange(tree: &mut Tree, id: NodeId, rect: Rect) {
    tree.nodes[id].rect = rect;
    let n = &tree.nodes[id];
    if n.children.is_empty() {
        return;
    }
    let inner = rect.inset(&content_insets(n));
    let dir = n.dir;
    let pack = n.pack;
    let kids: Vec<NodeId> = n.children.iter().copied().filter(|&c| !tree.nodes[c].hidden).collect();
    let horizontal = dir == Dir::Row;
    let main_avail = if horizontal { inner.w } else { inner.h };
    let cross_avail = if horizontal { inner.h } else { inner.w };

    let items: Vec<(f32, f32, f32, u32)> = kids
        .iter()
        .map(|&c| {
            let cn = &tree.nodes[c];
            let ((minw, maxw), (minh, maxh)) = outer_limits(cn);
            let (hint, min, max) =
                if horizontal { (cn.hint.0, minw, maxw) } else { (cn.hint.1, minh, maxh) };
            let g = if pack.is_some() { 0 } else { grow_class(cn, horizontal) };
            (hint, min, max, g)
        })
        .collect();
    let sizes = distribute(&items, main_avail);
    let used: f32 = sizes.iter().sum();
    let mut pos = if horizontal { inner.x } else { inner.y };
    if let Some((ha, va)) = pack {
        let a = if horizontal { ha } else { va };
        let free = (main_avail - used).max(0.0);
        pos += match a {
            Align::Start => 0.0,
            Align::Center => free / 2.0,
            Align::End => free,
        };
    }
    for (k, &c) in kids.iter().enumerate() {
        let main = sizes[k];
        let cn = &tree.nodes[c];
        let ((_, maxw), (_, maxh)) = outer_limits(cn);
        let cross_max = if horizontal { maxh } else { maxw };
        let cross_hint = if horizontal { cn.hint.1 } else { cn.hint.0 };
        let cross_policy = if horizontal { cn.vpolicy } else { cn.hpolicy };
        let align = cn.item_align.or(pack);
        let (cross, cross_a) = match align {
            Some((ha, va)) => (cross_hint.min(cross_avail).max(0.0), if horizontal { va } else { ha }),
            None if cross_policy == Policy::Fixed => (cross_hint.min(cross_avail), Align::Center),
            None => (cross_avail.min(cross_max), Align::Center),
        };
        let cross_start = if horizontal { inner.y } else { inner.x };
        let off = match cross_a {
            Align::Start => 0.0,
            Align::Center => ((cross_avail - cross) / 2.0).max(0.0),
            Align::End => (cross_avail - cross).max(0.0),
        };
        // on the main axis, an item with its own alignment whose cell is wider than its hint is placed according to that alignment
        let (mut mpos, mut msize) = (pos, main);
        if let Some((ha, va)) = cn.item_align {
            let main_hint = if horizontal { cn.hint.0 } else { cn.hint.1 };
            if main > main_hint {
                let a = if horizontal { ha } else { va };
                let free = main - main_hint;
                mpos += match a {
                    Align::Start => 0.0,
                    Align::Center => free / 2.0,
                    Align::End => free,
                };
                msize = main_hint;
            }
        }
        let r = if horizontal {
            Rect::new(mpos, cross_start + off, msize, cross)
        } else {
            Rect::new(cross_start + off, mpos, cross, msize)
        };
        pos += main;
        arrange(tree, c, r);
    }
}

/// Window size just big enough to contain the tree (adjustSize).
pub fn root_hint(tree: &Tree) -> (f32, f32) {
    tree.nodes.first().map(|n| n.hint).unwrap_or((0.0, 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fair_share_centers() {
        // 3 columns like the bar: left 300, middle 50, right 150, width 1920 → equal at 640
        let s = distribute(&[(300.0, 0.0, f32::INFINITY, 2), (50.0, 0.0, f32::INFINITY, 2), (150.0, 0.0, f32::INFINITY, 2)], 1920.0);
        assert!(s.iter().all(|v| (*v - 640.0).abs() < 0.01));
        let s = distribute(&[(900.0, 0.0, f32::INFINITY, 2), (50.0, 0.0, f32::INFINITY, 2), (150.0, 0.0, f32::INFINITY, 2)], 1920.0);
        assert_eq!(s[0], 900.0);
        assert!((s[1] - 510.0).abs() < 0.01);
    }

    #[test]
    fn stretch_wins() {
        let s = distribute(&[(20.0, 0.0, f32::INFINITY, 1), (30.0, 0.0, f32::INFINITY, 1), (0.0, 0.0, f32::INFINITY, 1001)], 200.0);
        assert_eq!(s, vec![20.0, 30.0, 150.0]);
    }
}
