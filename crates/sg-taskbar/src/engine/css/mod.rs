//! Ported from TachyonTaskbar (MIT), which is based on YASB (MIT).
//! Stylesheet engine: reads YASB's `styles.css` preprocessed exactly the way YASB does it
//! (@import, `:root` variables, comment stripping), then applies Qt Style Sheets-style selectors
//! (`.class` = the class property contains that name, Qt-style types such as `QLabel`, `:hover`, `::item`...).

pub mod values;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use regex::Regex;
pub use values::{parse_color, Color, Edges, Gradient, Shadow};
use values::*;

// ---------------------------------------------------------------- preprocessing

pub fn preprocess(css: &str, base: &Path) -> String {
    let mut seen = HashSet::new();
    let css = remove_comments(css);
    let css = process_imports(&css, base, &mut seen);
    let css = remove_comments(&css);
    let css = replace_variables(&css);
    resolve_urls(&css, base)
}

fn remove_comments(css: &str) -> String {
    let block = Regex::new(r"(?s)/\*.*?\*/").unwrap();
    let css = block.replace_all(css, "");
    // YASB also strips `// ...` to the end of the line; keep `://` inside urls intact.
    let line = Regex::new(r"(^|[^:])//[^\n]*").unwrap();
    line.replace_all(&css, "$1").into_owned()
}

fn process_imports(css: &str, base: &Path, seen: &mut HashSet<PathBuf>) -> String {
    let re = Regex::new(r#"(?i)@import\s+(?:url\(\s*["']?([^)"']+?)["']?\s*\)|["']([^"']+)["'])\s*;"#).unwrap();
    let mut out = String::with_capacity(css.len());
    let mut last = 0;
    for cap in re.captures_iter(css) {
        let m = cap.get(0).unwrap();
        out.push_str(&css[last..m.start()]);
        last = m.end();
        let rel = cap.get(1).or(cap.get(2)).map(|x| x.as_str()).unwrap_or("");
        let full = base.join(rel);
        let key = full.canonicalize().unwrap_or(full.clone());
        if !seen.insert(key) {
            crate::engine::log::warn(&format!("CSS: @import vòng lặp: {}", full.display()));
            continue;
        }
        match std::fs::read_to_string(&full) {
            Ok(t) => {
                let sub_base = full.parent().map(Path::to_path_buf).unwrap_or_else(|| base.to_path_buf());
                out.push_str(&process_imports(&remove_comments(&t), &sub_base, seen));
            }
            Err(e) => crate::engine::log::warn(&format!("CSS: không đọc được @import {}: {e}", full.display())),
        }
    }
    out.push_str(&css[last..]);
    out
}

fn replace_variables(css: &str) -> String {
    let root_re = Regex::new(r"(?s):root\s*\{([^}]*)\}").unwrap();
    let var_def = Regex::new(r"--([\w-]+)\s*:\s*([^;]+);").unwrap();
    let mut vars: HashMap<String, String> = HashMap::new();
    for cap in root_re.captures_iter(css) {
        for v in var_def.captures_iter(&cap[1]) {
            vars.insert(format!("--{}", v[1].trim()), v[2].trim().to_string());
        }
    }
    let css = root_re.replace_all(css, "").into_owned();
    let use_re = Regex::new(r"var\(\s*(--[\w-]+)\s*(?:,\s*([^)]*))?\)").unwrap();
    // resolve nested variables, at most 10 passes like YASB
    for _ in 0..10 {
        let mut changed = false;
        let snapshot = vars.clone();
        for v in vars.values_mut() {
            let nv = use_re
                .replace_all(v, |c: &regex::Captures| {
                    snapshot.get(&c[1]).cloned().unwrap_or_else(|| c[0].to_string())
                })
                .into_owned();
            if nv != *v {
                *v = nv;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    use_re
        .replace_all(&css, |c: &regex::Captures| {
            vars.get(&c[1])
                .cloned()
                .or_else(|| c.get(2).map(|f| f.as_str().trim().to_string()))
                .unwrap_or_else(|| c[0].to_string())
        })
        .into_owned()
}

fn resolve_urls(css: &str, base: &Path) -> String {
    let re = Regex::new(r#"url\((["']?)([^)]+?)(["']?)\)"#).unwrap();
    re.replace_all(css, |c: &regex::Captures| {
        let p = &c[2];
        if p.starts_with("data:") || p.contains("://") || Path::new(p).is_absolute() {
            return c[0].to_string();
        }
        let abs = base.join(p);
        format!("url({}{}{})", &c[1], abs.display().to_string().replace('\\', "/"), &c[3])
    })
    .into_owned()
}

fn url_of(v: &str) -> String {
    let v = v.trim();
    let inner = v.strip_prefix("url(").unwrap_or(v);
    unquote(inner.strip_suffix(')').unwrap_or(inner))
}

// ---------------------------------------------------------------- selector

pub mod state {
    pub const HOVER: u32 = 1;
    pub const PRESSED: u32 = 2;
    pub const CHECKED: u32 = 4;
    pub const DISABLED: u32 = 8;
    pub const FOCUS: u32 = 16;
    pub const SELECTED: u32 = 32;
    pub const HORIZONTAL: u32 = 64;
    pub const CHECKABLE: u32 = 128;
}

#[derive(Clone, Debug)]
enum Pseudo {
    State { flag: u32, want: bool },
    Never,
}

#[derive(Clone, Debug, Default)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    pseudos: Vec<Pseudo>,
    element: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Comb {
    Descendant,
    Child,
}

#[derive(Clone, Debug)]
struct Selector {
    /// left to right; the combinator precedes each part (ignored for the first part)
    parts: Vec<(Comb, Compound)>,
    specificity: u32,
}

/// An element for selector matching: the UI tree implements this trait.
pub trait Elem: Copy {
    fn is_type(&self, tag: &str) -> bool;
    fn id_is(&self, id: &str) -> bool;
    fn has_class(&self, c: &str) -> bool;
    fn states(&self) -> u32;
    fn parent(&self) -> Option<Self>;
}

fn pseudo_from(name: &str) -> Pseudo {
    let (neg, n) = match name.strip_prefix('!') {
        Some(r) => (true, r),
        None => (false, name),
    };
    let (flag, want) = match n {
        "hover" => (state::HOVER, true),
        "pressed" => (state::PRESSED, true),
        "checked" | "on" => (state::CHECKED, true),
        "unchecked" | "off" => (state::CHECKED, false),
        "disabled" => (state::DISABLED, true),
        "enabled" => (state::DISABLED, false),
        "focus" => (state::FOCUS, true),
        "selected" => (state::SELECTED, true),
        "horizontal" => (state::HORIZONTAL, true),
        "vertical" => (state::HORIZONTAL, false),
        "checkable" => (state::CHECKABLE, true),
        "active" | "window" => return Pseudo::State { flag: 0, want: !neg },
        _ => return Pseudo::Never,
    };
    Pseudo::State { flag, want: want != neg }
}

fn parse_compound(s: &str) -> Option<Compound> {
    let mut c = Compound::default();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let ident = |i: &mut usize| -> String {
        let st = *i;
        while *i < b.len() && (b[*i].is_alphanumeric() || b[*i] == '-' || b[*i] == '_' || b[*i] == '!') {
            *i += 1;
        }
        b[st..*i].iter().collect()
    };
    if i < b.len() && b[i] == '*' {
        i += 1;
    } else if i < b.len() && (b[i].is_alphabetic() || b[i] == '_') {
        c.tag = Some(ident(&mut i));
    }
    while i < b.len() {
        match b[i] {
            '.' => {
                i += 1;
                let n = ident(&mut i);
                if n.is_empty() {
                    return None;
                }
                c.classes.push(n);
            }
            '#' => {
                i += 1;
                c.id = Some(ident(&mut i));
            }
            ':' => {
                if i + 1 < b.len() && b[i + 1] == ':' {
                    i += 2;
                    c.element = Some(ident(&mut i).to_ascii_lowercase());
                } else {
                    i += 1;
                    let n = ident(&mut i).to_ascii_lowercase();
                    c.pseudos.push(pseudo_from(&n));
                }
            }
            '[' => {
                let st = i + 1;
                while i < b.len() && b[i] != ']' {
                    i += 1;
                }
                let inner: String = b[st..i.min(b.len())].iter().collect();
                i += 1;
                // only [class~="x"] / [class="x"] are supported
                if let Some((k, v)) = inner.split_once('=') {
                    let k = k.trim().trim_end_matches(['~', '|', '^', '$', '*']);
                    if k == "class" {
                        for cl in unquote(v).split_whitespace() {
                            c.classes.push(cl.to_string());
                        }
                        continue;
                    }
                }
                c.pseudos.push(Pseudo::Never);
            }
            _ => return None,
        }
    }
    Some(c)
}

fn parse_selector(s: &str) -> Option<Selector> {
    let s = s.replace('>', " > ");
    let mut parts = Vec::new();
    let mut comb = Comb::Descendant;
    for tok in s.split_whitespace() {
        if tok == ">" {
            comb = Comb::Child;
            continue;
        }
        parts.push((comb, parse_compound(tok)?));
        comb = Comb::Descendant;
    }
    if parts.is_empty() {
        return None;
    }
    let mut spec = 0u32;
    for (_, c) in &parts {
        if c.id.is_some() {
            spec += 10000;
        }
        spec += 100 * (c.classes.len() + c.pseudos.len()) as u32;
        if c.tag.is_some() {
            spec += 1;
        }
        if c.element.is_some() {
            spec += 1;
        }
    }
    Some(Selector { parts, specificity: spec })
}

fn compound_matches<E: Elem>(c: &Compound, e: E) -> bool {
    if let Some(t) = &c.tag
        && !e.is_type(t) {
            return false;
        }
    if let Some(id) = &c.id
        && !e.id_is(id) {
            return false;
        }
    if !c.classes.iter().all(|cl| e.has_class(cl)) {
        return false;
    }
    let st = e.states();
    c.pseudos.iter().all(|p| match p {
        Pseudo::Never => false,
        Pseudo::State { flag: 0, want } => *want,
        Pseudo::State { flag, want } => ((st & flag) != 0) == *want,
    })
}

fn selector_matches<E: Elem>(sel: &Selector, e: E, element: Option<&str>) -> bool {
    let last = sel.parts.len() - 1;
    if sel.parts[last].1.element.as_deref() != element {
        return false;
    }
    if !compound_matches(&sel.parts[last].1, e) {
        return false;
    }
    match_rest(sel, last, e)
}

/// Matches parts `idx-1` and earlier against the ancestors of `e` (with backtracking for the descendant combinator).
fn match_rest<E: Elem>(sel: &Selector, idx: usize, e: E) -> bool {
    if idx == 0 {
        return true;
    }
    let comb = sel.parts[idx].0;
    let target = &sel.parts[idx - 1].1;
    let mut cur = e.parent();
    while let Some(p) = cur {
        if compound_matches(target, p) && match_rest(sel, idx - 1, p) {
            return true;
        }
        if comb == Comb::Child {
            return false;
        }
        cur = p.parent();
    }
    false
}

// ---------------------------------------------------------------- stylesheet

struct Rule {
    sel: Selector,
    order: u32,
    decls: Rc<Vec<(String, String)>>,
}

#[derive(Default)]
pub struct Stylesheet {
    rules: Vec<Rule>,
}

impl Stylesheet {
    pub fn parse(css: &str) -> Stylesheet {
        let mut rules = Vec::new();
        let b = css.as_bytes();
        let mut i = 0;
        let mut order = 0u32;
        while i < b.len() {
            let Some(open) = css[i..].find('{').map(|x| x + i) else { break };
            let head = css[i..open].trim();
            // find the matching closing brace
            let mut depth = 0;
            let mut j = open;
            while j < b.len() {
                match b[j] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            let body = &css[open + 1..j.min(b.len())];
            i = j + 1;
            // skip @media, @font-face... (Qt does not support them); a block-less @ statement preceding the head
            let head = head.rsplit(';').next().unwrap_or(head).trim();
            if head.starts_with('@') || head.is_empty() {
                continue;
            }
            let decls: Vec<(String, String)> = body
                .split(';')
                .filter_map(|d| {
                    let (k, v) = d.split_once(':')?;
                    let k = k.trim().to_ascii_lowercase();
                    let v = v.trim().trim_end_matches("!important").trim().to_string();
                    if k.is_empty() || v.is_empty() {
                        return None;
                    }
                    Some((k, v))
                })
                .collect();
            let decls = Rc::new(decls);
            for s in split_commas(head) {
                match parse_selector(&s) {
                    Some(sel) => {
                        rules.push(Rule { sel, order, decls: decls.clone() });
                        order += 1;
                    }
                    None => crate::engine::log::debug(&format!("CSS: bỏ qua selector không hiểu: {s}")),
                }
            }
        }
        rules.sort_by_key(|r| (r.sel.specificity, r.order));
        Stylesheet { rules }
    }

    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Computes the style for an element (or for its `::element` sub-element).
    pub fn compute<E: Elem>(&self, e: E, element: Option<&str>, parent: &Style) -> Style {
        let mut st = Style::inherit_from(parent);
        for r in &self.rules {
            if selector_matches(&r.sel, e, element) {
                for (k, v) in r.decls.iter() {
                    st.apply(k, v, parent);
                }
            }
        }
        st
    }
}

// ---------------------------------------------------------------- style

#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub families: Rc<[String]>,
    pub size: f32,
    pub weight: u16,
    pub italic: bool,
}

impl Default for Font {
    fn default() -> Self {
        Font { families: Rc::from(vec!["Segoe UI".to_string()]), size: 12.0, weight: 400, italic: false }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Transition {
    pub prop: String,
    pub ms: f32,
}

#[derive(Clone, Debug)]
pub struct Style {
    pub color: Color,
    pub font: Font,
    pub background: Option<Color>,
    pub gradient: Option<Gradient>,
    pub background_image: Option<String>,
    /// `background-size`: auto | cover | contain | stretch (100% 100%)
    pub bg_size: String,
    /// `background-position` as a ratio (0 = left/top, 0.5 = center)
    pub bg_pos: (f32, f32),
    /// Qt's `image:` property: the image fits the content box, keeping its aspect ratio
    pub image: Option<String>,
    pub shadows: Vec<Shadow>,
    /// extension: gradient-filled border (`border-color: qlineargradient(...)`)
    pub border_gradient: Option<Gradient>,
    /// custom extension: scales the image/icon up (used with `transition: -tachyon-image-scale`)
    pub image_scale: f32,
    pub margin: Edges,
    pub padding: Edges,
    pub border: Edges,
    pub border_color: [Color; 4],
    /// top-left, top-right, bottom-right, bottom-left
    pub radius: [f32; 4],
    pub min_w: Option<f32>,
    pub max_w: Option<f32>,
    pub min_h: Option<f32>,
    pub max_h: Option<f32>,
    pub opacity: f32,
    pub transitions: Vec<Transition>,
    pub text_align: Option<String>,
    pub spacing: Option<f32>,
    pub icon_size: Option<f32>,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            color: Color::BLACK,
            font: Font::default(),
            background: None,
            gradient: None,
            background_image: None,
            bg_size: "auto".into(),
            bg_pos: (0.0, 0.0),
            image: None,
            shadows: Vec::new(),
            border_gradient: None,
            image_scale: 1.0,
            margin: Edges::default(),
            padding: Edges::default(),
            border: Edges::default(),
            border_color: [Color::TRANSPARENT; 4],
            radius: [0.0; 4],
            min_w: None,
            max_w: None,
            min_h: None,
            max_h: None,
            opacity: 1.0,
            transitions: Vec::new(),
            text_align: None,
            spacing: None,
            icon_size: None,
        }
    }
}

fn parse_weight(v: &str) -> Option<u16> {
    match v.trim().to_ascii_lowercase().as_str() {
        "normal" => Some(400),
        "bold" => Some(700),
        "light" | "lighter" => Some(300),
        "bolder" => Some(800),
        "medium" => Some(500),
        "demibold" | "semibold" => Some(600),
        "black" => Some(900),
        "thin" => Some(100),
        n => n.parse::<u16>().ok().map(|w| w.clamp(1, 1000)),
    }
}

impl Style {
    /// Qt does not inherit by default, but the text color/font are usually set by `*` for every element;
    /// when no rule applies, inherit from the parent like CSS so we don't end up with black text on a dark background.
    pub fn inherit_from(parent: &Style) -> Style {
        Style { color: parent.color, font: parent.font.clone(), ..Style::default() }
    }

    fn set_border_side(&mut self, side: usize, v: &str) {
        let mut width = None;
        let mut color = None;
        let mut none = false;
        for t in split_ws(v) {
            let tl = t.to_ascii_lowercase();
            if let Some(g) = values::parse_gradient(&t) {
                self.border_gradient = Some(g);
                color = Some(Color::WHITE);
            } else if tl == "none" || tl == "hidden" {
                none = true;
            } else if matches!(tl.as_str(), "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset") {
            } else if let Some(w) = parse_len(&t) {
                width = Some(w);
            } else if let Some(c) = parse_color(&t) {
                color = Some(c);
            }
        }
        let w = if none { 0.0 } else { width.unwrap_or(if color.is_some() { 1.0 } else { 0.0 }) };
        match side {
            0 => self.border.top = w,
            1 => self.border.right = w,
            2 => self.border.bottom = w,
            _ => self.border.left = w,
        }
        if let Some(c) = color {
            self.border_color[side] = c;
        }
    }

    pub fn apply(&mut self, k: &str, v: &str, parent: &Style) {
        let sides = |s: &str| -> Option<usize> {
            match s {
                "top" => Some(0),
                "right" => Some(1),
                "bottom" => Some(2),
                "left" => Some(3),
                _ => None,
            }
        };
        match k {
            "color" => {
                if v.eq_ignore_ascii_case("inherit") {
                    self.color = parent.color;
                } else if let Some(c) = parse_color(v) {
                    self.color = c;
                }
            }
            "background-color" => {
                if let Some(g) = values::parse_gradient(v) {
                    self.gradient = Some(g);
                    self.background = None;
                } else {
                    self.background = parse_color(v);
                    self.gradient = None;
                }
            }
            "background" => {
                let mut col = None;
                for t in split_ws(v) {
                    if t.starts_with("url(") {
                        self.background_image = Some(url_of(&t));
                    } else if let Some(g) = values::parse_gradient(&t) {
                        self.gradient = Some(g);
                        self.background = None;
                    } else if let Some(c) = parse_color(&t) {
                        col = Some(c);
                        self.gradient = None;
                    } else if t.eq_ignore_ascii_case("none") {
                        col = Some(Color::TRANSPARENT);
                        self.gradient = None;
                        self.background_image = None;
                    }
                }
                if col.is_some() {
                    self.background = col;
                }
            }
            "background-image" => {
                if v.trim().starts_with("url(") {
                    self.background_image = Some(url_of(v));
                } else if let Some(g) = values::parse_gradient(v) {
                    self.gradient = Some(g);
                } else if v.trim() == "none" {
                    self.background_image = None;
                }
            }
            "image" => {
                self.image = if v.trim().starts_with("url(") { Some(url_of(v)) } else { None };
            }
            "background-size" => self.bg_size = v.trim().to_ascii_lowercase(),
            "background-position" => {
                let f = |t: &str| match t {
                    "left" | "top" => Some(0.0),
                    "center" => Some(0.5),
                    "right" | "bottom" => Some(1.0),
                    p => p.strip_suffix('%').and_then(|x| x.trim().parse::<f32>().ok()).map(|x| x / 100.0),
                };
                let toks = split_ws(&v.to_ascii_lowercase());
                let x = toks.first().and_then(|t| f(t)).unwrap_or(0.0);
                let y = toks.get(1).and_then(|t| f(t)).unwrap_or(if toks.len() == 1 { 0.5 } else { 0.0 });
                self.bg_pos = (x, y);
            }
            "box-shadow" => self.shadows = values::parse_shadows(v),
            "-tachyon-image-scale" | "tachyon-image-scale" => {
                if let Ok(x) = v.trim().parse::<f32>() {
                    self.image_scale = x.clamp(0.1, 4.0);
                }
            }
            "font-family" => {
                let fams: Vec<String> = split_commas(v).iter().map(|f| unquote(f)).filter(|f| !f.is_empty()).collect();
                if !fams.is_empty() {
                    self.font.families = Rc::from(fams);
                }
            }
            "font-size" => {
                if let Some(s) = parse_len(v) {
                    self.font.size = s.max(1.0);
                }
            }
            "font-weight" => {
                if let Some(w) = parse_weight(v) {
                    self.font.weight = w;
                }
            }
            "font-style" => self.font.italic = matches!(v.trim(), "italic" | "oblique"),
            "font" => {
                // for example: `bold 14px "Segoe UI"`
                let toks = split_ws(v);
                let mut fam_start = toks.len();
                for (i, t) in toks.iter().enumerate() {
                    if let Some(w) = parse_weight(t)
                        && (!t.chars().next().is_some_and(|c| c.is_ascii_digit()) || t.len() == 3) {
                            self.font.weight = w;
                            continue;
                        }
                    if t == "italic" {
                        self.font.italic = true;
                    } else if let Some(s) = parse_len(t) {
                        self.font.size = s;
                        fam_start = i + 1;
                        break;
                    }
                }
                if fam_start < toks.len() {
                    self.apply("font-family", &toks[fam_start..].join(" "), parent);
                }
            }
            "padding" => {
                if let Some(e) = parse_edges(v) {
                    self.padding = e;
                }
            }
            "margin" => {
                if let Some(e) = parse_edges(v) {
                    self.margin = e;
                }
            }
            "border" => {
                for s in 0..4 {
                    self.set_border_side(s, v);
                }
            }
            "border-width" => {
                if let Some(e) = parse_edges(v) {
                    self.border = e;
                }
            }
            "border-color" if values::parse_gradient(v).is_some() => {
                self.border_gradient = values::parse_gradient(v);
                self.border_color = [Color::WHITE; 4];
            }
            "border-color" => {
                self.border_gradient = None;
                let cs: Vec<Color> = split_ws(v).iter().filter_map(|t| parse_color(t)).collect();
                self.border_color = match cs.as_slice() {
                    [a] => [*a; 4],
                    [a, b] => [*a, *b, *a, *b],
                    [a, b, c] => [*a, *b, *c, *b],
                    [a, b, c, d, ..] => [*a, *b, *c, *d],
                    _ => self.border_color,
                };
            }
            "border-style" => {
                if v.trim() == "none" {
                    self.border = Edges::default();
                }
            }
            "border-radius" => {
                let r: Vec<f32> = split_ws(v).iter().filter_map(|t| parse_len(t)).collect();
                self.radius = match r.as_slice() {
                    [a] => [*a; 4],
                    [a, b] => [*a, *b, *a, *b],
                    [a, b, c] => [*a, *b, *c, *b],
                    [a, b, c, d, ..] => [*a, *b, *c, *d],
                    _ => self.radius,
                };
            }
            "border-top-left-radius" => self.radius[0] = parse_len(v).unwrap_or(0.0),
            "border-top-right-radius" => self.radius[1] = parse_len(v).unwrap_or(0.0),
            "border-bottom-right-radius" => self.radius[2] = parse_len(v).unwrap_or(0.0),
            "border-bottom-left-radius" => self.radius[3] = parse_len(v).unwrap_or(0.0),
            "min-width" => self.min_w = parse_len(v),
            "max-width" => self.max_w = parse_len(v),
            "min-height" => self.min_h = parse_len(v),
            "max-height" => self.max_h = parse_len(v),
            "width" => {
                self.min_w = parse_len(v);
                self.max_w = self.min_w;
            }
            "height" => {
                self.min_h = parse_len(v);
                self.max_h = self.min_h;
            }
            "opacity" => {
                if let Ok(o) = v.trim().parse::<f32>() {
                    self.opacity = if o > 1.0 { o / 255.0 } else { o }.clamp(0.0, 1.0);
                }
            }
            "transition" => {
                self.transitions = split_commas(v)
                    .iter()
                    .filter_map(|t| {
                        let p = split_ws(t);
                        let prop = p.first()?.to_ascii_lowercase();
                        let ms = p.iter().skip(1).find_map(|x| parse_duration(x))?;
                        Some(Transition { prop, ms })
                    })
                    .collect();
            }
            "text-align" | "qproperty-alignment" => self.text_align = Some(v.trim().to_ascii_lowercase()),
            "spacing" => self.spacing = parse_len(v),
            "icon-size" | "qproperty-iconsize" => self.icon_size = split_ws(v).first().and_then(|x| parse_len(x)),
            _ => {
                if let Some(rest) = k.strip_prefix("padding-") {
                    if let (Some(s), Some(l)) = (sides(rest), parse_len(v)) {
                        match s {
                            0 => self.padding.top = l,
                            1 => self.padding.right = l,
                            2 => self.padding.bottom = l,
                            _ => self.padding.left = l,
                        }
                    }
                } else if let Some(rest) = k.strip_prefix("margin-") {
                    if let (Some(s), Some(l)) = (sides(rest), parse_len(v)) {
                        match s {
                            0 => self.margin.top = l,
                            1 => self.margin.right = l,
                            2 => self.margin.bottom = l,
                            _ => self.margin.left = l,
                        }
                    }
                } else if let Some(rest) = k.strip_prefix("border-") {
                    let (side, sub) = rest.split_once('-').unwrap_or((rest, ""));
                    if let Some(s) = sides(side) {
                        match sub {
                            "" => self.set_border_side(s, v),
                            "width" => {
                                let w = parse_len(v).unwrap_or(0.0);
                                match s {
                                    0 => self.border.top = w,
                                    1 => self.border.right = w,
                                    2 => self.border.bottom = w,
                                    _ => self.border.left = w,
                                }
                            }
                            "color" => {
                                if let Some(c) = parse_color(v) {
                                    self.border_color[s] = c;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    pub fn transition_ms(&self, prop: &str) -> Option<f32> {
        self.transitions.iter().find(|t| t.prop == prop || t.prop == "all").map(|t| t.ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct E<'a> {
        nodes: &'a [(&'a str, &'a str, u32, Option<usize>)],
        i: usize,
    }
    impl Elem for E<'_> {
        fn is_type(&self, t: &str) -> bool {
            self.nodes[self.i].0 == t || t == "QWidget"
        }
        fn id_is(&self, _: &str) -> bool {
            false
        }
        fn has_class(&self, c: &str) -> bool {
            self.nodes[self.i].1.split_whitespace().any(|x| x == c)
        }
        fn states(&self) -> u32 {
            self.nodes[self.i].2
        }
        fn parent(&self) -> Option<Self> {
            self.nodes[self.i].3.map(|p| E { nodes: self.nodes, i: p })
        }
    }

    #[test]
    fn vars_and_match() {
        let css = ":root { --a: #ff0000; --b: var(--a); }\n/* c */ .widget .icon { color: var(--b); font-size: 16px }\n.widget:hover { background-color: rgba(255,255,255,0.08) }";
        let pre = preprocess(css, Path::new("."));
        let sheet = Stylesheet::parse(&pre);
        assert_eq!(sheet.rule_count(), 2);
        let nodes = [("QFrame", "widget home-widget", state::HOVER, None), ("QLabel", "icon", 0, Some(0))];
        let root = Style::default();
        let fs = sheet.compute(E { nodes: &nodes, i: 0 }, None, &root);
        assert!(fs.background.is_some());
        let ls = sheet.compute(E { nodes: &nodes, i: 1 }, None, &fs);
        assert_eq!(ls.color, Color::rgba8(255, 0, 0, 255));
        assert_eq!(ls.font.size, 16.0);
    }

    #[test]
    fn element_and_child() {
        let css = ".a > .b::item:selected { color: #fff } .a .b { color: #000 }";
        let sheet = Stylesheet::parse(css);
        let nodes = [("QFrame", "a", 0, None), ("QTableView", "b", state::SELECTED, Some(0))];
        let s = sheet.compute(E { nodes: &nodes, i: 1 }, Some("item"), &Style::default());
        assert_eq!(s.color, Color::WHITE);
    }
}
