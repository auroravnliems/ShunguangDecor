//! Ported from TachyonTaskbar (MIT), based on YASB (MIT).
//! CSS values: colors, lengths, edges (padding/margin), tokenizing with parentheses.

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
    pub const WHITE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
    pub const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };

    pub fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a: a as f32 / 255.0 }
    }

    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    pub fn lerp(self, o: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        Color {
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
            a: self.a + (o.a - self.a) * t,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.a > 0.001
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Edges {
    pub fn h(&self) -> f32 {
        self.left + self.right
    }
    pub fn v(&self) -> f32 {
        self.top + self.bottom
    }
}

/// Splits a string on whitespace but keeps parenthesized/quoted parts intact, e.g. `1px solid rgba(1, 2, 3, 0.5)`.
pub fn split_ws(s: &str) -> Vec<String> {
    split_by(s, |c| c.is_whitespace())
}

/// Splits on top-level commas.
pub fn split_commas(s: &str) -> Vec<String> {
    split_by(s, |c| c == ',')
}

fn split_by(s: &str, sep: impl Fn(char) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        if let Some(q) = quote {
            cur.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                cur.push(c);
            }
            '(' | '[' => {
                depth += 1;
                cur.push(c);
            }
            ')' | ']' => {
                depth -= 1;
                cur.push(c);
            }
            c if depth == 0 && sep(c) => {
                let t = cur.trim();
                if !t.is_empty() {
                    out.push(t.to_string());
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    out
}

/// Length in logical px. Qt understands `px` and `pt` (1pt = 4/3 px at 96 DPI); a bare number is treated as px.
pub fn parse_len(s: &str) -> Option<f32> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty() {
        return None;
    }
    let (num, unit) = match s.find(|c: char| c.is_ascii_alphabetic() || c == '%') {
        Some(i) => (&s[..i], &s[i..]),
        None => (s.as_str(), ""),
    };
    let v: f32 = num.trim().parse().ok()?;
    match unit {
        "" | "px" => Some(v),
        "pt" => Some(v * 4.0 / 3.0),
        "em" => Some(v * 16.0),
        "ex" => Some(v * 8.0),
        _ => None,
    }
}

pub fn parse_edges(s: &str) -> Option<Edges> {
    let parts: Vec<f32> = split_ws(s).iter().filter_map(|p| parse_len(p)).collect();
    let e = match parts.as_slice() {
        [a] => Edges { top: *a, right: *a, bottom: *a, left: *a },
        [v, h] => Edges { top: *v, right: *h, bottom: *v, left: *h },
        [t, h, b] => Edges { top: *t, right: *h, bottom: *b, left: *h },
        [t, r, b, l, ..] => Edges { top: *t, right: *r, bottom: *b, left: *l },
        _ => return None,
    };
    Some(e)
}

fn hex(s: &str) -> Option<u8> {
    u8::from_str_radix(s, 16).ok()
}

fn channel(s: &str, max: f32) -> Option<f32> {
    let s = s.trim();
    if let Some(p) = s.strip_suffix('%') {
        return Some((p.trim().parse::<f32>().ok()? / 100.0).clamp(0.0, 1.0));
    }
    Some((s.parse::<f32>().ok()? / max).clamp(0.0, 1.0))
}

fn alpha(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Some(p) = s.strip_suffix('%') {
        return Some((p.trim().parse::<f32>().ok()? / 100.0).clamp(0.0, 1.0));
    }
    let v: f32 = s.parse().ok()?;
    // Qt accepts alpha 0-255; CSS uses 0-1. YASB uses both styles.
    Some(if v > 1.0 { (v / 255.0).clamp(0.0, 1.0) } else { v.clamp(0.0, 1.0) })
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let h = ((h % 360.0) + 360.0) % 360.0 / 360.0;
    if s == 0.0 {
        return (l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    (f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    if let Some(h) = lower.strip_prefix('#') {
        let b = h.as_bytes();
        return match b.len() {
            3 => {
                let d = |i: usize| hex(&h[i..i + 1]).map(|v| v * 17);
                Some(Color::rgba8(d(0)?, d(1)?, d(2)?, 255))
            }
            4 => {
                let d = |i: usize| hex(&h[i..i + 1]).map(|v| v * 17);
                Some(Color::rgba8(d(0)?, d(1)?, d(2)?, d(3)?))
            }
            6 => Some(Color::rgba8(hex(&h[0..2])?, hex(&h[2..4])?, hex(&h[4..6])?, 255)),
            // CSS #RRGGBBAA (YASB converts it to #AARRGGBB for Qt; here it is read directly in CSS order)
            8 => Some(Color::rgba8(hex(&h[0..2])?, hex(&h[2..4])?, hex(&h[4..6])?, hex(&h[6..8])?)),
            _ => None,
        };
    }
    if let Some(open) = lower.find('(') {
        let func = lower[..open].trim();
        let inner = lower[open + 1..].trim_end_matches(')').replace('/', ",");
        let args: Vec<String> = if inner.contains(',') {
            inner.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
        } else {
            split_ws(&inner)
        };
        return match func {
            "rgb" | "rgba" => {
                if args.len() < 3 {
                    return None;
                }
                let a = if args.len() > 3 { alpha(&args[3])? } else { 1.0 };
                Some(Color { r: channel(&args[0], 255.0)?, g: channel(&args[1], 255.0)?, b: channel(&args[2], 255.0)?, a })
            }
            "hsl" | "hsla" => {
                if args.len() < 3 {
                    return None;
                }
                let h: f32 = args[0].trim_end_matches("deg").parse().ok()?;
                let sat = channel(&args[1], 100.0)?;
                let l = channel(&args[2], 100.0)?;
                let a = if args.len() > 3 { alpha(&args[3])? } else { 1.0 };
                let (r, g, b) = hsl_to_rgb(h, sat, l);
                Some(Color { r, g, b, a })
            }
            _ => None,
        };
    }
    named_color(&lower)
}

fn named_color(n: &str) -> Option<Color> {
    let v: u32 = match n {
        "transparent" => return Some(Color::TRANSPARENT),
        "black" => 0x000000,
        "white" => 0xffffff,
        "red" => 0xff0000,
        "green" => 0x008000,
        "lime" => 0x00ff00,
        "blue" => 0x0000ff,
        "yellow" => 0xffff00,
        "cyan" | "aqua" => 0x00ffff,
        "magenta" | "fuchsia" => 0xff00ff,
        "gray" | "grey" => 0x808080,
        "darkgray" | "darkgrey" => 0xa9a9a9,
        "lightgray" | "lightgrey" => 0xd3d3d3,
        "silver" => 0xc0c0c0,
        "maroon" => 0x800000,
        "olive" => 0x808000,
        "navy" => 0x000080,
        "purple" => 0x800080,
        "teal" => 0x008080,
        "orange" => 0xffa500,
        "pink" => 0xffc0cb,
        "gold" => 0xffd700,
        "brown" => 0xa52a2a,
        "violet" => 0xee82ee,
        "indigo" => 0x4b0082,
        "coral" => 0xff7f50,
        "salmon" => 0xfa8072,
        "tomato" => 0xff6347,
        "crimson" => 0xdc143c,
        "orchid" => 0xda70d6,
        "plum" => 0xdda0dd,
        "khaki" => 0xf0e68c,
        "beige" => 0xf5f5dc,
        "ivory" => 0xfffff0,
        "skyblue" => 0x87ceeb,
        "steelblue" => 0x4682b4,
        "royalblue" => 0x4169e1,
        "dodgerblue" => 0x1e90ff,
        "deepskyblue" => 0x00bfff,
        "lightblue" => 0xadd8e6,
        "darkblue" => 0x00008b,
        "darkred" => 0x8b0000,
        "darkgreen" => 0x006400,
        "lightgreen" => 0x90ee90,
        "limegreen" => 0x32cd32,
        "seagreen" => 0x2e8b57,
        "slategray" | "slategrey" => 0x708090,
        "dimgray" | "dimgrey" => 0x696969,
        "whitesmoke" => 0xf5f5f5,
        "gainsboro" => 0xdcdcdc,
        "turquoise" => 0x40e0d0,
        "chocolate" => 0xd2691e,
        "tan" => 0xd2b48c,
        "wheat" => 0xf5deb3,
        "lavender" => 0xe6e6fa,
        "mintcream" => 0xf5fffa,
        "hotpink" => 0xff69b4,
        "deeppink" => 0xff1493,
        "orangered" => 0xff4500,
        "darkorange" => 0xff8c00,
        "yellowgreen" => 0x9acd32,
        _ => return None,
    };
    Some(Color::rgba8((v >> 16) as u8, (v >> 8) as u8, v as u8, 255))
}

/// Linear gradient: coordinates are fractions of the box (0..1), like Qt's ObjectBoundingMode.
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub stops: Vec<(f32, Color)>,
}

/// `qlineargradient(x1:0, y1:0, x2:1, y2:0, stop:0 #a, stop:1 #b)` or `linear-gradient(to right|90deg, a 0%, b 100%)`.
pub fn parse_gradient(s: &str) -> Option<Gradient> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    let open = lower.find('(')?;
    let func = lower[..open].trim();
    let inner = &s[open + 1..s.rfind(')')?];
    let args = split_commas(inner);
    match func {
        "qlineargradient" => {
            let mut g = Gradient { x1: 0.0, y1: 0.0, x2: 0.0, y2: 1.0, stops: Vec::new() };
            for a in args {
                let (k, v) = a.split_once(':')?;
                let k = k.trim().to_ascii_lowercase();
                let v = v.trim();
                match k.as_str() {
                    "x1" => g.x1 = v.parse().ok()?,
                    "y1" => g.y1 = v.parse().ok()?,
                    "x2" => g.x2 = v.parse().ok()?,
                    "y2" => g.y2 = v.parse().ok()?,
                    "stop" => {
                        let mut it = split_ws(v).into_iter();
                        let pos: f32 = it.next()?.parse().ok()?;
                        let col = parse_color(&it.collect::<Vec<_>>().join(" "))?;
                        g.stops.push((pos.clamp(0.0, 1.0), col));
                    }
                    _ => {}
                }
            }
            (!g.stops.is_empty()).then_some(g)
        }
        "linear-gradient" => {
            let mut args = args.into_iter().peekable();
            let (mut x1, mut y1, mut x2, mut y2) = (0.0, 0.0, 0.0, 1.0);
            if let Some(first) = args.peek() {
                let f = first.to_ascii_lowercase();
                let dir = if f.starts_with("to ") {
                    Some(match f.trim_start_matches("to ").trim() {
                        "right" => (0.0, 0.0, 1.0, 0.0),
                        "left" => (1.0, 0.0, 0.0, 0.0),
                        "top" => (0.0, 1.0, 0.0, 0.0),
                        "bottom right" | "right bottom" => (0.0, 0.0, 1.0, 1.0),
                        _ => (0.0, 0.0, 0.0, 1.0),
                    })
                } else if let Some(d) = f.strip_suffix("deg") {
                    let a: f32 = d.trim().parse().ok()?;
                    let r = a.to_radians();
                    let (dx, dy) = (r.sin() / 2.0, -r.cos() / 2.0);
                    Some((0.5 - dx, 0.5 - dy, 0.5 + dx, 0.5 + dy))
                } else {
                    None
                };
                if let Some(d) = dir {
                    (x1, y1, x2, y2) = d;
                    args.next();
                }
            }
            let list: Vec<String> = args.collect();
            let n = list.len().max(2) - 1;
            let mut stops = Vec::new();
            for (i, a) in list.iter().enumerate() {
                let toks = split_ws(a);
                let col = parse_color(toks.first()?)?;
                let pos = toks.get(1).and_then(|p| p.trim_end_matches('%').parse::<f32>().ok()).map(|p| p / 100.0).unwrap_or(i as f32 / n as f32);
                stops.push((pos.clamp(0.0, 1.0), col));
            }
            (!stops.is_empty()).then_some(Gradient { x1, y1, x2, y2, stops })
        }
        _ => None,
    }
}

/// `box-shadow: dx dy blur [spread] color` (inset is not supported).
#[derive(Clone, Debug, PartialEq)]
pub struct Shadow {
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
}

pub fn parse_shadows(s: &str) -> Vec<Shadow> {
    if s.trim().eq_ignore_ascii_case("none") {
        return Vec::new();
    }
    split_commas(s)
        .iter()
        .filter_map(|one| {
            let toks = split_ws(one);
            if toks.iter().any(|t| t.eq_ignore_ascii_case("inset")) {
                return None;
            }
            let lens: Vec<f32> = toks.iter().filter_map(|t| parse_len(t)).collect();
            let color = toks.iter().find_map(|t| parse_color(t)).unwrap_or(Color::BLACK.with_alpha(0.5));
            if lens.len() < 2 {
                return None;
            }
            Some(Shadow { dx: lens[0], dy: lens[1], blur: *lens.get(2).unwrap_or(&0.0), spread: *lens.get(3).unwrap_or(&0.0), color })
        })
        .collect()
}

/// CSS duration (`0.1s`, `150ms`) → milliseconds.
pub fn parse_duration(s: &str) -> Option<f32> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(v) = s.strip_suffix("ms") {
        return v.trim().parse().ok();
    }
    if let Some(v) = s.strip_suffix('s') {
        return v.trim().parse::<f32>().ok().map(|x| x * 1000.0);
    }
    None
}

pub fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\''))) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("#fff"), Some(Color::WHITE));
        let c = parse_color("rgba(36, 36, 36, 0.60)").unwrap();
        assert!((c.a - 0.6).abs() < 1e-4 && (c.r - 36.0 / 255.0).abs() < 1e-4);
        let c = parse_color("#4cc2ff80").unwrap();
        assert!((c.a - 128.0 / 255.0).abs() < 1e-4);
        assert_eq!(parse_color("transparent"), Some(Color::TRANSPARENT));
        assert!(parse_color("rgba(255,255,255,200)").unwrap().a > 0.7);
    }

    #[test]
    fn gradients() {
        let g = parse_gradient("qlineargradient(x1:0, y1:0, x2:1, y2:0, stop:0 #0d1427, stop:1 rgba(212, 175, 55, 0.5))").unwrap();
        assert_eq!((g.x2, g.stops.len()), (1.0, 2));
        assert!((g.stops[1].1.a - 0.5).abs() < 1e-3);
        let g = parse_gradient("linear-gradient(to right, #000, #fff)").unwrap();
        assert_eq!((g.x1, g.x2, g.stops[1].0), (0.0, 1.0, 1.0));
        let s = parse_shadows("0 0 10px rgba(242, 210, 122, 0.6)");
        assert_eq!(s[0].blur, 10.0);
    }

    #[test]
    fn edges() {
        let e = parse_edges("0px 6px").unwrap();
        assert_eq!((e.top, e.right, e.bottom, e.left), (0.0, 6.0, 0.0, 6.0));
        let e = parse_edges("8px 48px 8px 16px").unwrap();
        assert_eq!(e.left, 16.0);
        assert_eq!(split_ws("1px solid rgba(1, 2, 3, 0.5)").len(), 3);
    }
}
