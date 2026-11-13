//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! The UI element tree. Each node corresponds to a QWidget/QFrame/QLabel... in YASB,
//! carrying the same type name (tag) and `class` attribute, so styles.css applies identically.

use std::rc::Rc;

use crate::engine::css::{state, Elem, Style};

pub type NodeId = usize;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Row,
    Col,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Policy {
    Fixed,
    Preferred,
    Expanding,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(dead_code)]
pub enum Align {
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }
    pub fn inset(&self, e: &crate::engine::css::Edges) -> Rect {
        Rect {
            x: self.x + e.left,
            y: self.y + e.top,
            w: (self.w - e.h()).max(0.0),
            h: (self.h - e.v()).max(0.0),
        }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

/// A premultiplied-alpha BGRA bitmap, shared between frames (the id lets gfx keep the GPU bitmap).
pub struct Pixmap {
    pub id: u64,
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

impl Pixmap {
    pub fn new(w: u32, h: u32, data: Vec<u8>) -> Rc<Pixmap> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Rc::new(Pixmap { id: NEXT.fetch_add(1, Ordering::Relaxed), w, h, data })
    }

    /// Grayscale version (for muted applications in the volume menu).
    pub fn grayscale(&self) -> Rc<Pixmap> {
        let mut d = self.data.clone();
        for px in d.as_chunks_mut::<4>().0 {
            let g = (px[0] as u32 * 11 + px[1] as u32 * 59 + px[2] as u32 * 30) / 100;
            px[0] = g as u8;
            px[1] = g as u8;
            px[2] = g as u8;
        }
        Pixmap::new(self.w, self.h, d)
    }
}

#[derive(Clone)]
pub struct Image {
    pub pix: Rc<Pixmap>,
    pub w: f32,
    pub h: f32,
    pub circle: bool,
}

#[derive(Clone)]
pub enum Content {
    None,
    Text(String),
    Image(Image),
    /// Horizontal QSlider, value 0..=max
    Slider { value: i32, max: i32 },
    /// QFrame HLine (separator)
    HLine,
    /// stretch gap `addStretch(factor)`
    Stretch(u16),
    /// fixed spacing
    Spacing(f32),
    /// progress bar in the style of YASB's `progress_bar` (0..=100)
    Progress(Progress),
    /// Windows logo: 4 squares in the text color, side length = the value (logical px)
    WinLogo(f32),
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub value: f32,
    /// "circular" | "linear_horizontal" | "linear_vertical"
    pub kind: String,
    pub thickness: f32,
    pub radius: f32,
    pub colors: Vec<crate::engine::css::Color>,
    pub background: crate::engine::css::Color,
    pub animate: bool,
}

#[derive(Clone)]
pub struct Node {
    pub tag: &'static str,
    pub classes: String,
    pub id: Option<String>,
    pub dir: Dir,
    pub content: Content,
    /// alignment of text/image inside the content box (QLabel::setAlignment)
    pub halign: Align,
    pub valign: Align,
    /// alignment when placed into the parent layout (`addWidget(w, alignment=...)`); None = fill
    pub item_align: Option<(Align, Align)>,
    /// `layout.setAlignment(...)`: packs the children into a group and aligns the whole group
    pub pack: Option<(Align, Align)>,
    pub hpolicy: Policy,
    pub vpolicy: Policy,
    pub fixed: (Option<f32>, Option<f32>),
    pub word_wrap: bool,
    pub states: u32,
    pub hidden: bool,
    /// the node is a pseudo-child of its parent, e.g. a calendar cell (`::item`)
    pub sub_element: Option<&'static str>,
    /// owning widget (index in the App's widget list)
    pub owner: usize,
    pub on_click: Option<u32>,
    pub on_press: Option<u32>,
    pub on_wheel: Option<u32>,
    /// holding the mouse still over this node (about 500 ms) → `Ev::Hover { id }` for the owning widget
    pub on_hover: Option<u32>,
    pub on_slider: Option<u32>,
    pub tooltip: Option<String>,
    pub tooltip_delay: u32,
    pub hover_class: bool,
    /// margins of the inner layout (`layout.setContentsMargins`)
    pub layout_margins: crate::engine::css::Edges,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
    pub key: u64,
    // ---- computed results
    pub style: Style,
    pub rect: Rect,
    pub hint: (f32, f32),
}

impl Node {
    pub fn new(tag: &'static str, classes: &str) -> Node {
        let (hp, vp) = match tag {
            "QPushButton" => (Policy::Preferred, Policy::Fixed),
            "QSlider" => (Policy::Expanding, Policy::Fixed),
            _ => (Policy::Preferred, Policy::Preferred),
        };
        Node {
            tag,
            classes: classes.trim().to_string(),
            id: None,
            dir: Dir::Row,
            content: Content::None,
            halign: if tag == "QPushButton" { Align::Center } else { Align::Start },
            valign: Align::Center,
            item_align: None,
            pack: None,
            hpolicy: hp,
            vpolicy: vp,
            fixed: (None, None),
            word_wrap: false,
            states: if tag == "QSlider" { state::HORIZONTAL } else { 0 },
            hidden: false,
            sub_element: None,
            owner: usize::MAX,
            on_click: None,
            on_press: None,
            on_wheel: None,
            on_hover: None,
            on_slider: None,
            tooltip: None,
            tooltip_delay: 400,
            hover_class: false,
            layout_margins: crate::engine::css::Edges::default(),
            parent: None,
            children: Vec::new(),
            key: 0,
            style: Style::default(),
            rect: Rect::default(),
            hint: (0.0, 0.0),
        }
    }

}

/// A flat tree (arena). Node 0 is the root.
#[derive(Default, Clone)]
pub struct Tree {
    pub nodes: Vec<Node>,
    /// dynamic state merged in when computing style (hover/pressed), per node
    pub dyn_states: Vec<u32>,
}

impl Tree {
    pub fn root(&self) -> Option<NodeId> {
        if self.nodes.is_empty() {
            None
        } else {
            Some(0)
        }
    }

    pub fn elem(&self, id: NodeId) -> NodeRef<'_> {
        NodeRef { tree: self, id, sub: None }
    }

    /// The deepest node containing the point (hidden nodes skipped), in paint order (a later child covers an earlier one).
    pub fn hit(&self, x: f32, y: f32) -> Option<NodeId> {
        let root = self.root()?;
        if !self.nodes[root].rect.contains(x, y) || self.nodes[root].hidden {
            return None;
        }
        let mut cur = root;
        'outer: loop {
            for &c in self.nodes[cur].children.iter().rev() {
                let n = &self.nodes[c];
                if !n.hidden && n.rect.contains(x, y) {
                    cur = c;
                    continue 'outer;
                }
            }
            return Some(cur);
        }
    }

    /// The chain from the node up to the root.
    pub fn chain(&self, id: NodeId) -> Vec<NodeId> {
        let mut v = vec![id];
        let mut cur = id;
        while let Some(p) = self.nodes[cur].parent {
            v.push(p);
            cur = p;
        }
        v
    }

}

#[derive(Clone, Copy)]
pub struct NodeRef<'a> {
    pub tree: &'a Tree,
    pub id: NodeId,
    /// state to use in place of the node's (used when computing `::item` with the cell's own state)
    pub sub: Option<u32>,
}

/// Qt-style inheritance tree so type selectors (QFrame, QWidget...) also match subclasses.
fn type_matches(tag: &str, sel: &str) -> bool {
    if tag == sel || sel == "QWidget" {
        return true;
    }
    if tag == "Widget" || tag == "PopupWidget" {
        return false;
    }
    let parents: &[&str] = match tag {
        "QLabel" => &["QFrame"],
        "QPushButton" => &["QAbstractButton"],
        "QSlider" => &["QAbstractSlider"],
        "QTableView" => &["QAbstractItemView", "QAbstractScrollArea", "QFrame"],
        "QScrollArea" => &["QAbstractScrollArea", "QFrame"],
        "QMenu" | "QCalendarWidget" | "QToolTip" => &[],
        _ => &[],
    };
    parents.contains(&sel)
}

impl Elem for NodeRef<'_> {
    fn is_type(&self, t: &str) -> bool {
        type_matches(self.tree.nodes[self.id].tag, t)
    }
    fn id_is(&self, id: &str) -> bool {
        self.tree.nodes[self.id].id.as_deref() == Some(id)
    }
    fn has_class(&self, c: &str) -> bool {
        self.tree.nodes[self.id].classes.split_whitespace().any(|x| x == c)
    }
    fn states(&self) -> u32 {
        if let Some(s) = self.sub {
            return s;
        }
        self.tree.nodes[self.id].states | self.tree.dyn_states.get(self.id).copied().unwrap_or(0)
    }
    fn parent(&self) -> Option<Self> {
        // a nested layout is not a widget, so selectors skip it (as in Qt)
        let mut p = self.tree.nodes[self.id].parent;
        while let Some(i) = p {
            if !self.tree.nodes[i].is_layout_only() {
                return Some(NodeRef { tree: self.tree, id: i, sub: None });
            }
            p = self.tree.nodes[i].parent;
        }
        None
    }
}

impl Node {
    /// a layout-only node (nested layout, stretch, spacing) – receives no CSS, is not painted.
    pub fn is_layout_only(&self) -> bool {
        matches!(self.tag, "layout" | "stretch" | "spacing")
    }
}

// ------------------------------------------------------------------ builder

/// Builds the tree declaratively: `ui.frame(...)`, `ui.label(...)`.
pub struct Ui<'a> {
    pub tree: &'a mut Tree,
    stack: Vec<NodeId>,
    pub owner: usize,
}

impl<'a> Ui<'a> {
    pub fn new(tree: &'a mut Tree, owner: usize) -> Ui<'a> {
        tree.nodes.clear();
        Ui { tree, stack: Vec::new(), owner }
    }

    pub fn add(&mut self, mut n: Node) -> NodeId {
        let id = self.tree.nodes.len();
        if n.owner == usize::MAX {
            n.owner = self.owner;
        }
        n.parent = self.stack.last().copied();
        let idx = match n.parent {
            Some(p) => self.tree.nodes[p].children.len() as u64,
            None => 0,
        };
        let pkey = n.parent.map(|p| self.tree.nodes[p].key).unwrap_or(0xcbf2_9ce4_8422_2325 ^ self.owner as u64);
        n.key = (pkey ^ (idx + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)).wrapping_mul(0x100_0000_01b3);
        self.tree.nodes.push(n);
        if let Some(p) = self.stack.last() {
            self.tree.nodes[*p].children.push(id);
        }
        id
    }

    /// Adds a container node and builds its children inside the closure.
    pub fn with(&mut self, n: Node, f: impl FnOnce(&mut Ui)) -> NodeId {
        let id = self.add(n);
        self.stack.push(id);
        f(self);
        self.stack.pop();
        id
    }

    pub fn node(&mut self, id: NodeId) -> &mut Node {
        &mut self.tree.nodes[id]
    }

    // ---- helpers

    pub fn frame(&mut self, tag: &'static str, class: &str, dir: Dir, f: impl FnOnce(&mut Ui)) -> NodeId {
        let mut n = Node::new(tag, class);
        n.dir = dir;
        self.with(n, f)
    }

    pub fn row(&mut self, class: &str, f: impl FnOnce(&mut Ui)) -> NodeId {
        self.frame("QFrame", class, Dir::Row, f)
    }

    pub fn col(&mut self, class: &str, f: impl FnOnce(&mut Ui)) -> NodeId {
        self.frame("QFrame", class, Dir::Col, f)
    }

    /// Nested layout (QHBoxLayout/QVBoxLayout without a widget of their own) – receives no style.
    pub fn layout(&mut self, dir: Dir, f: impl FnOnce(&mut Ui)) -> NodeId {
        let mut n = Node::new("layout", "");
        n.dir = dir;
        self.with(n, f)
    }

    pub fn label(&mut self, class: &str, text: impl Into<String>) -> NodeId {
        let mut n = Node::new("QLabel", class);
        n.content = Content::Text(text.into());
        self.add(n)
    }

    pub fn label_centered(&mut self, class: &str, text: impl Into<String>) -> NodeId {
        let id = self.label(class, text);
        self.tree.nodes[id].halign = Align::Center;
        id
    }

    pub fn button(&mut self, class: &str, text: impl Into<String>, on_click: u32) -> NodeId {
        let mut n = Node::new("QPushButton", class);
        n.content = Content::Text(text.into());
        n.on_click = Some(on_click);
        self.add(n)
    }

    pub fn image(&mut self, class: &str, img: Image) -> NodeId {
        let mut n = Node::new("QLabel", class);
        n.halign = Align::Center;
        n.content = Content::Image(img);
        self.add(n)
    }

    pub fn stretch(&mut self) -> NodeId {
        let mut n = Node::new("stretch", "");
        n.content = Content::Stretch(1);
        n.hpolicy = Policy::Expanding;
        n.vpolicy = Policy::Expanding;
        self.add(n)
    }

    pub fn spacing(&mut self, px: f32) -> NodeId {
        let mut n = Node::new("spacing", "");
        n.content = Content::Spacing(px);
        n.hpolicy = Policy::Fixed;
        n.vpolicy = Policy::Fixed;
        self.add(n)
    }

    pub fn separator(&mut self) -> NodeId {
        let mut n = Node::new("QFrame", "separator");
        n.content = Content::HLine;
        n.vpolicy = Policy::Fixed;
        self.add(n)
    }

    pub fn slider(&mut self, class: &str, value: i32, max: i32, id: u32) -> NodeId {
        let mut n = Node::new("QSlider", class);
        n.content = Content::Slider { value, max };
        n.on_slider = Some(id);
        n.on_wheel = Some(id);
        self.add(n)
    }
}
