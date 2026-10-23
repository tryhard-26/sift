use crate::scan::{human, Node};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
};
use std::path::Path;

const MAX: usize = 300;

fn worst(row: &[f64], side: f64) -> f64 {
    let s: f64 = row.iter().sum();
    let mx = row.iter().cloned().fold(0.0, f64::max);
    let mn = row.iter().cloned().fold(f64::MAX, f64::min);
    f64::max(side * side * mx / (s * s), s * s / (side * side * mn))
}

/// squarified layout; y is doubled internally since cells are ~2:1
pub fn squarify(sizes: &[u64], a: Rect) -> Vec<(usize, Rect)> {
    let n = sizes.iter().take(MAX).take_while(|&&s| s > 0).count();
    let total: u64 = sizes[..n].iter().sum();
    if total == 0 || a.width == 0 || a.height == 0 {
        return vec![];
    }
    let (mut x, mut y, mut w, mut h) = (0.0, 0.0, a.width as f64, a.height as f64 * 2.0);
    let k = w * h / total as f64;
    let v: Vec<f64> = sizes[..n].iter().map(|&s| s as f64 * k).collect();
    let cell = |x: f64, y: f64, w: f64, h: f64| {
        let (x0, x1, y0, y1) = (x.round(), (x + w).round(), (y / 2.0).round(), ((y + h) / 2.0).round());
        Rect::new(a.x + x0 as u16, a.y + y0 as u16, (x1 - x0) as u16, (y1 - y0) as u16)
    };
    let mut out = vec![];
    let mut i = 0;
    while i < n && w > 0.5 && h > 0.5 {
        let side = w.min(h);
        let mut j = i + 1;
        while j < n && worst(&v[i..=j], side) <= worst(&v[i..j], side) {
            j += 1;
        }
        let s: f64 = v[i..j].iter().sum();
        let mut off = 0.0;
        if w >= h {
            let rw = s / h;
            for (m, &a) in v[i..j].iter().enumerate() {
                out.push((i + m, cell(x, y + off, rw, a / rw)));
                off += a / rw;
            }
            x += rw;
            w -= rw;
        } else {
            let rh = s / w;
            for (m, &a) in v[i..j].iter().enumerate() {
                out.push((i + m, cell(x + off, y, a / rh, rh)));
                off += a / rh;
            }
            y += rh;
            h -= rh;
        }
        i = j;
    }
    out.retain(|(_, r)| r.width > 0 && r.height > 0);
    out
}

pub type Hot<'a> = &'a dyn Fn(&Path) -> Option<Color>;

pub fn draw(buf: &mut Buffer, a: Rect, n: &Node, sel: Option<usize>, hot: Hot) -> Vec<(Rect, usize)> {
    let sizes: Vec<u64> = n.kids.iter().map(|k| k.size).collect();
    let tiles = squarify(&sizes, a);
    for &(i, r) in &tiles {
        tile(buf, r, &n.kids[i], sel == Some(i), hot);
    }
    tiles.into_iter().map(|(i, r)| (r, i)).collect()
}

fn fill(buf: &mut Buffer, r: Rect, bg: Color) {
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            buf[(x, y)].reset();
            buf[(x, y)].set_bg(bg);
        }
    }
}

fn tile(buf: &mut Buffer, r: Rect, k: &Node, sel: bool, hot: Hot) {
    let h = hot(&k.path);
    if r.width < 4 || r.height < 3 {
        fill(buf, r, k.cat.col(if sel { 0.9 } else { 0.42 }));
        return;
    }
    // cleanup candidates get a safe/warn border instead of the category colour
    let fg = if sel { Color::White } else { h.unwrap_or(k.cat.col(0.95)) };
    fill(buf, r, k.cat.col(if sel { 0.26 } else { 0.15 }));
    let st = Style::new().fg(fg);
    let (x0, x1, y0, y1) = (r.left(), r.right() - 1, r.top(), r.bottom() - 1);
    let [tl, tr, bl, br, hz, vt] = if sel { ['┏', '┓', '┗', '┛', '━', '┃'] } else { ['╭', '╮', '╰', '╯', '─', '│'] };
    for x in x0..=x1 {
        buf[(x, y0)].set_char(hz).set_style(st);
        buf[(x, y1)].set_char(hz).set_style(st);
    }
    for y in y0..=y1 {
        buf[(x0, y)].set_char(vt).set_style(st);
        buf[(x1, y)].set_char(vt).set_style(st);
    }
    buf[(x0, y0)].set_char(tl);
    buf[(x1, y0)].set_char(tr);
    buf[(x0, y1)].set_char(bl);
    buf[(x1, y1)].set_char(br);
    let w = (r.width - 2) as usize;
    let bold = st.add_modifier(Modifier::BOLD);
    buf.set_stringn(x0 + 1, y0, format!(" {} ", k.name), w, bold);
    let sz = format!(" {} ", human(k.size));
    if sz.len() <= w {
        buf.set_stringn(x0 + 1, y1, &sz, w, bold);
    }
    let inner = Rect::new(x0 + 1, y0 + 1, r.width - 2, r.height - 2);
    if !k.dir || inner.width < 6 || inner.height < 2 {
        return;
    }
    let sizes: Vec<u64> = k.kids.iter().map(|c| c.size).collect();
    for (j, (i, rr)) in squarify(&sizes, inner).into_iter().enumerate() {
        let c = &k.kids[i];
        let g = Rect { width: rr.width.saturating_sub((rr.right() < inner.right()) as u16), ..rr };
        fill(buf, g, c.cat.col(if j % 2 == 0 { 0.3 } else { 0.38 }));
        if g.width >= 4 {
            let fg = hot(&c.path).unwrap_or(c.cat.col(1.0));
            buf.set_stringn(g.x, g.y, &c.name, g.width as usize, Style::new().fg(fg));
        }
    }
}

#[cfg(test)]
mod t {
    use super::*;

    #[test]
    fn tiles_cover_area() {
        let a = Rect::new(2, 3, 80, 24);
        let ts = squarify(&[50, 25, 12, 8, 5], a);
        assert_eq!(ts.len(), 5);
        let area: u32 = ts.iter().map(|(_, r)| r.area()).sum();
        assert_eq!(area, a.area());
        for (i, r) in &ts {
            assert!(a.contains(r.as_position()) && r.right() <= a.right() && r.bottom() <= a.bottom());
            for (j, s) in &ts {
                assert!(i == j || !r.intersects(*s));
            }
        }
    }
}
