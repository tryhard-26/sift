use crate::rules::{recommend, Rec, Rule};
use crate::scan::{self, human, locate, prune, Node, Prog};
use crate::treemap;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode as K, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Alignment, Constraint::*, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const ACC: Color = Color::Rgb(125, 220, 170);
const TXT: Color = Color::Rgb(225, 228, 235);
const DIM: Color = Color::Rgb(115, 120, 135);
const EDGE: Color = Color::Rgb(58, 61, 74);
const SEL: Color = Color::Rgb(38, 41, 54);
const SAFE: Color = Color::Rgb(110, 210, 140);
const WARN: Color = Color::Rgb(240, 180, 90);
const BAD: Color = Color::Rgb(240, 105, 105);
const SPIN: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const LOGO: [&str; 3] = ["┌─┐┬┌─┐┌┬┐", "└─┐│├┤  │ ", "└─┘┴└   ┴ "];

pub fn run(root: PathBuf, home: PathBuf, ex: Vec<String>, rules: Vec<Rule>) -> io::Result<()> {
    let mut t = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture)?;
    let r = (|| -> io::Result<()> {
        loop {
            let Some((n, errs)) = scanning(&mut t, &root, &home, &ex)? else { return Ok(()) };
            if !App::new(n, errs, home.clone(), &rules).run(&mut t)? {
                return Ok(());
            }
        }
    })();
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    r
}

fn quit(k: &KeyEvent) -> bool {
    k.code == K::Char('q') || (k.code == K::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL))
}

fn tilde(home: &Path, p: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(r) if r.as_os_str().is_empty() => "~".into(),
        Ok(r) => format!("~/{}", r.display()),
        Err(_) => p.display().to_string(),
    }
}

fn fit(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        format!("{s:<w$}")
    } else if w == 0 {
        String::new()
    } else {
        s.chars().take(w - 1).chain(['…']).collect()
    }
}

fn center(a: Rect, w: u16, h: u16) -> Rect {
    let (w, h) = (w.min(a.width), h.min(a.height));
    Rect::new(a.x + (a.width - w) / 2, a.y + (a.height - h) / 2, w, h)
}

fn card<'a>(title: Line<'a>, focus: bool) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focus { ACC } else { EDGE }))
        .title(title)
}

fn scanning(t: &mut DefaultTerminal, root: &Path, home: &Path, ex: &[String]) -> io::Result<Option<(Node, u64)>> {
    let p = Arc::new(Prog::default());
    let (tx, rx) = mpsc::channel();
    {
        let (p, root, ex) = (p.clone(), root.to_path_buf(), ex.to_vec());
        std::thread::spawn(move || tx.send(scan::scan(&root, &ex, &p)));
    }
    let t0 = Instant::now();
    loop {
        if let Ok(n) = rx.try_recv() {
            return Ok(Some((n, p.errs.load(Relaxed))));
        }
        let el = t0.elapsed();
        t.draw(|f| {
            let a = center(f.area(), 46, 9);
            let tick = (el.as_millis() / 80) as usize;
            let mut ls: Vec<Line> = LOGO.iter().map(|l| Line::from(l.fg(ACC).bold())).collect();
            ls.push(Line::raw(""));
            ls.push(Line::from(vec![
                format!("{} ", SPIN[tick % SPIN.len()]).fg(ACC),
                "scanning ".fg(DIM),
                tilde(home, root).fg(TXT),
            ]));
            let bw = 30usize;
            let pos = tick % (bw * 2);
            let pos = if pos < bw { pos } else { bw * 2 - pos };
            let bar: String = (0..bw).map(|i| if i.abs_diff(pos) < 4 { '━' } else { '─' }).collect();
            ls.push(Line::from(bar.fg(EDGE)));
            ls.push(Line::from(vec![
                format!("{} files", p.files.load(Relaxed)).fg(TXT),
                "  ·  ".fg(DIM),
                human(p.bytes.load(Relaxed)).fg(TXT),
                "  ·  ".fg(DIM),
                format!("{:.1}s", el.as_secs_f32()).fg(DIM),
            ]));
            f.render_widget(Paragraph::new(ls).alignment(Alignment::Center), a);
        })?;
        if event::poll(Duration::from_millis(60))? {
            if let Event::Key(k) = event::read()? {
                if quit(&k) {
                    return Ok(None);
                }
            }
        }
    }
}

enum Modal {
    Help,
    Del(Vec<PathBuf>, u64),
}

struct App {
    root: Node,
    errs: u64,
    home: PathBuf,
    rules: Vec<Rule>,
    stack: Vec<usize>,
    list: ListState,
    rlist: ListState,
    rsel: usize,
    recs: Vec<Rec>,
    on_recs: bool,
    map: bool,
    modal: Option<Modal>,
    toast: Option<(String, Color, Instant)>,
    tiles: Vec<(Rect, usize)>,
    list_area: Rect,
    rec_area: Rect,
    disk: Option<(u64, u64)>,
}

impl App {
    fn new(root: Node, errs: u64, home: PathBuf, rules: &[Rule]) -> Self {
        let recs = recommend(&root, &home, rules);
        let disk = scan::disk(&root.path);
        App {
            root,
            errs,
            home,
            rules: rules.to_vec(),
            stack: vec![],
            list: ListState::default().with_selected(Some(0)),
            rlist: ListState::default(),
            rsel: 0,
            recs,
            on_recs: false,
            map: true,
            modal: None,
            toast: None,
            tiles: vec![],
            list_area: Rect::default(),
            rec_area: Rect::default(),
            disk,
        }
    }

    fn cur(&self) -> &Node {
        self.stack.iter().fold(&self.root, |n, &i| &n.kids[i])
    }

    fn sel(&self) -> Option<&Node> {
        self.cur().kids.get(self.list.selected()?)
    }

    /// true = rescan, false = quit
    fn run(&mut self, t: &mut DefaultTerminal) -> io::Result<bool> {
        loop {
            t.draw(|f| self.draw(f))?;
            if !event::poll(Duration::from_millis(500))? {
                continue;
            }
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if let Some(r) = self.key(k) {
                        return Ok(r);
                    }
                }
                Event::Mouse(m) => self.mouse(m),
                _ => {}
            }
        }
    }

    fn key(&mut self, k: KeyEvent) -> Option<bool> {
        if let Some(m) = self.modal.take() {
            if let (Modal::Del(ps, _), K::Char('y' | 'Y')) = (m, k.code) {
                self.delete(ps);
            }
            return None;
        }
        if quit(&k) {
            return Some(false);
        }
        match k.code {
            K::Char('r') => return Some(true),
            K::Char('?') => self.modal = Some(Modal::Help),
            K::Tab | K::BackTab => self.on_recs = !self.on_recs,
            K::Char('t') => self.map = !self.map,
            K::Char('a') => {
                let all = self.recs.iter().filter(|r| r.safe).all(|r| r.on);
                self.recs.iter_mut().filter(|r| r.safe).for_each(|r| r.on = !all);
            }
            K::Char('x') => self.script(),
            K::Char('o') => self.reveal(),
            K::Char('d') | K::Delete => self.ask_del(),
            K::Up | K::Char('k') => self.mv(-1),
            K::Down | K::Char('j') => self.mv(1),
            K::PageUp => self.mv(-10),
            K::PageDown => self.mv(10),
            K::Home | K::Char('g') => self.mv(isize::MIN / 2),
            K::End | K::Char('G') => self.mv(isize::MAX / 2),
            K::Char(' ') if self.on_recs => {
                if let Some(r) = self.recs.get_mut(self.rsel) {
                    r.on = !r.on;
                }
            }
            K::Enter | K::Right | K::Char('l') if self.on_recs => self.jump(),
            K::Enter | K::Right | K::Char('l') => self.open(),
            K::Left | K::Char('h') | K::Backspace | K::Esc => self.back(),
            _ => {}
        }
        None
    }

    fn mouse(&mut self, m: MouseEvent) {
        let p = Position::new(m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                self.on_recs = self.rec_area.contains(p);
                self.mv(if m.kind == MouseEventKind::ScrollDown { 3 } else { -3 });
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(&(_, i)) = self.tiles.iter().find(|(r, _)| r.contains(p)) {
                    self.on_recs = false;
                    self.list.select(Some(i));
                    self.open();
                } else if self.list_area.contains(p) {
                    self.on_recs = false;
                    let i = self.list.offset() + (p.y - self.list_area.y) as usize;
                    if i < self.cur().kids.len() {
                        // 2nd click on same row opens
                        if self.list.selected() == Some(i) {
                            self.open();
                        } else {
                            self.list.select(Some(i));
                        }
                    }
                } else if self.rec_area.contains(p) {
                    self.on_recs = true;
                }
            }
            _ => {}
        }
    }

    fn mv(&mut self, d: isize) {
        let (n, i) = if self.on_recs {
            (self.recs.len(), self.rsel)
        } else {
            (self.cur().kids.len(), self.list.selected().unwrap_or(0))
        };
        if n == 0 {
            return;
        }
        let j = (i as isize).saturating_add(d).clamp(0, n as isize - 1) as usize;
        if self.on_recs {
            self.rsel = j;
        } else {
            self.list.select(Some(j));
        }
    }

    fn open(&mut self) {
        if let Some(i) = self.list.selected().filter(|_| self.sel().is_some_and(|n| n.dir)) {
            self.stack.push(i);
            self.list.select(Some(0));
        }
    }

    fn back(&mut self) {
        if let Some(i) = self.stack.pop() {
            self.list.select(Some(i));
        }
    }

    fn jump(&mut self) {
        let Some(p) = self.recs.get(self.rsel).and_then(|r| r.paths.first()).cloned() else { return };
        let mut v = locate(&self.root, &p);
        let last = v.pop();
        self.stack = v;
        self.list.select(last.or(Some(0)));
        self.on_recs = false;
    }

    fn toast(&mut self, s: String, c: Color) {
        self.toast = Some((s, c, Instant::now()));
    }

    fn picked(&self) -> impl Iterator<Item = &Rec> {
        self.recs.iter().filter(|r| r.on)
    }

    fn ask_del(&mut self) {
        let (ps, sz): (Vec<PathBuf>, u64) = if self.on_recs {
            (self.picked().flat_map(|r| r.paths.clone()).collect(), self.picked().map(|r| r.size).sum())
        } else {
            self.sel().map(|n| (vec![n.path.clone()], n.size)).unwrap_or_default()
        };
        if ps.is_empty() {
            self.toast("Nothing selected".into(), WARN);
        } else {
            self.modal = Some(Modal::Del(ps, sz));
        }
    }

    fn delete(&mut self, ps: Vec<PathBuf>) {
        let here = self.cur().path.clone();
        let i = self.list.selected();
        let (mut freed, mut fail) = (0, 0);
        for p in &ps {
            // never touch anything outside or equal to scan root
            if !p.starts_with(&self.root.path) || *p == self.root.path {
                fail += 1;
                continue;
            }
            let r = std::fs::symlink_metadata(p)
                .and_then(|m| if m.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) });
            match r {
                Ok(()) => freed += prune(&mut self.root, p).0,
                Err(_) => fail += 1,
            }
        }
        self.stack = locate(&self.root, &here);
        let n = self.cur().kids.len();
        self.list.select(i.map(|i| i.min(n.saturating_sub(1))));
        let prev: Vec<(String, bool)> = self.recs.iter().map(|r| (r.label.clone(), r.on)).collect();
        self.recs = recommend(&self.root, &self.home, &self.rules);
        for r in &mut self.recs {
            if let Some((_, on)) = prev.iter().find(|(l, _)| *l == r.label) {
                r.on = *on;
            }
        }
        self.rsel = self.rsel.min(self.recs.len().saturating_sub(1));
        self.disk = scan::disk(&self.root.path);
        if fail > 0 {
            self.toast(format!("Freed {}, {fail} failed (press r to rescan)", human(freed)), BAD);
        } else {
            self.toast(format!("Freed {}", human(freed)), SAFE);
        }
    }

    fn script(&mut self) {
        if self.picked().next().is_none() {
            return self.toast("Nothing selected".into(), WARN);
        }
        let mut s = String::from("#!/bin/sh\n# generated by sift\nset -e\n");
        for r in self.picked() {
            s += &format!("\n# {} ({})\n", r.label, human(r.size));
            for p in &r.paths {
                s += &format!("rm -rf -- '{}'\n", p.display().to_string().replace('\'', r"'\''"));
            }
        }
        let f = "sift-cleanup.sh";
        match std::fs::write(f, s) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755));
                }
                self.toast(format!("Wrote ./{f}"), SAFE)
            }
            Err(e) => self.toast(format!("Write failed: {e}"), BAD),
        }
    }

    fn reveal(&mut self) {
        let p = if self.on_recs {
            self.recs.get(self.rsel).and_then(|r| r.paths.first()).cloned()
        } else {
            self.sel().map(|n| n.path.clone())
        };
        let Some(p) = p else { return };
        let mut c = if cfg!(target_os = "macos") {
            let mut c = std::process::Command::new("open");
            c.arg("-R").arg(&p);
            c
        } else if cfg!(windows) {
            let mut c = std::process::Command::new("explorer");
            c.arg(format!("/select,{}", p.display()));
            c
        } else {
            let mut c = std::process::Command::new("xdg-open");
            c.arg(p.parent().unwrap_or(&p));
            c
        };
        let ok = c.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().is_ok();
        if !ok {
            self.toast("Could not open file manager".into(), BAD);
        }
    }

    fn draw(&mut self, f: &mut Frame) {
        let [head, stats, body, foot] = Layout::vertical([Length(1), Length(4), Min(6), Length(1)]).areas(f.area());
        self.header(f, head);
        self.stats(f, stats);
        let wide = self.map && body.width >= 110;
        let cols = if wide {
            Layout::horizontal([Percentage(30), Min(30), Length(46)]).split(body)
        } else {
            Layout::horizontal([Min(30), Length(46)]).split(body)
        };
        self.explorer(f, cols[0]);
        if wide {
            self.treemap(f, cols[1]);
        }
        self.recs(f, cols[cols.len() - 1]);
        self.footer(f, foot);
        self.modal(f);
    }

    fn header(&self, f: &mut Frame, a: Rect) {
        let mut l = vec![" sift ".fg(Color::Black).bg(ACC).bold(), "  ".into()];
        let p = tilde(&self.home, &self.cur().path);
        match p.rsplit_once('/') {
            Some((d, b)) => l.extend([format!("{d}/").fg(DIM), b.to_string().fg(TXT).bold()]),
            None => l.push(p.fg(TXT).bold()),
        }
        f.render_widget(Line::from(l), a);
        if let Some((free, tot)) = self.disk {
            let r = Line::from(vec![human(free).fg(TXT), format!(" free of {} ", human(tot)).fg(DIM)]);
            f.render_widget(r.alignment(Alignment::Right), a);
        }
    }

    fn stats(&self, f: &mut Frame, a: Rect) {
        let sum = |safe: bool| {
            let v: Vec<&Rec> = self.recs.iter().filter(|r| r.safe == safe).collect();
            (v.iter().map(|r| r.size).sum::<u64>(), v.len())
        };
        let picked: u64 = self.picked().map(|r| r.size).sum();
        let (s, sn) = sum(true);
        let (r, rn) = sum(false);
        let items = |n: usize| format!("{n} item{}", if n == 1 { "" } else { "s" });
        let mut cards = vec![];
        match self.disk {
            Some((free, _)) => cards.push((
                Line::from(" Free after cleanup ".fg(DIM)),
                Line::from(vec![human(free + picked).fg(TXT).bold(), format!("  +{}", human(picked)).fg(SAFE)]),
                format!("{} free now", human(free)),
            )),
            None => cards.push((Line::from(" Selected ".fg(DIM)), Line::from(human(picked).fg(TXT).bold()), String::new())),
        }
        cards.push((Line::from(vec![" ● ".fg(SAFE), "Safe to remove ".fg(DIM)]), Line::from(human(s).fg(TXT).bold()), items(sn)));
        cards.push((Line::from(vec![" ● ".fg(WARN), "Needs review ".fg(DIM)]), Line::from(human(r).fg(TXT).bold()), items(rn)));
        let mut sub = format!("{} files", self.root.files);
        if self.errs > 0 {
            sub += &format!(" · {} unreadable", self.errs);
        }
        cards.push((Line::from(" Scanned ".fg(DIM)), Line::from(human(self.root.size).fg(TXT).bold()), sub));
        let areas = Layout::horizontal([Ratio(1, 4); 4]).split(a);
        for ((t, v, s), a) in cards.into_iter().zip(areas.iter()) {
            let p = Paragraph::new(vec![v, Line::from(s.fg(DIM))]).block(card(t, false).padding(ratatui::widgets::Padding::horizontal(1)));
            f.render_widget(p, *a);
        }
    }

    fn explorer(&mut self, f: &mut Frame, a: Rect) {
        let cur = self.stack.iter().fold(&self.root, |n, &i| &n.kids[i]);
        let blk = card(Line::from(" Explorer ".fg(TXT).bold()), !self.on_recs)
            .title(Line::from(format!(" {} ", cur.kids.len()).fg(DIM)).right_aligned());
        let inner = blk.inner(a);
        self.list_area = inner;
        if cur.kids.is_empty() {
            f.render_widget(Paragraph::new("\n  empty".fg(DIM)).block(blk), a);
            return;
        }
        let tot = cur.size.max(1) as f64;
        let w = inner.width as usize;
        let nw = w.saturating_sub(3 + 1 + 8 + 10);
        let items: Vec<ListItem> = cur
            .kids
            .iter()
            .map(|k| {
                let c = k.cat.col(1.0);
                let name = if k.dir { format!("{}/", k.name) } else { k.name.clone() };
                let ns = if k.dir { Style::new().fg(TXT) } else { Style::new().fg(Color::Rgb(170, 174, 186)) };
                ListItem::new(Line::from(vec![
                    " ● ".fg(c),
                    Span::styled(fit(&name, nw), ns),
                    " ".into(),
                    bar(k.size as f64 / tot, 8).fg(c),
                    format!("{:>9} ", human(k.size)).fg(TXT),
                ]))
            })
            .collect();
        let l = List::new(items).block(blk).highlight_style(Style::new().bg(SEL).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(l, a, &mut self.list);
    }

    fn treemap(&mut self, f: &mut Frame, a: Rect) {
        let cur = self.cur();
        let blk = card(Line::from(format!(" {} ", tilde(&self.home, &cur.path)).fg(TXT).bold()), false)
            .title(Line::from(format!(" {} ", human(cur.size)).fg(DIM)).right_aligned());
        let inner = blk.inner(a);
        f.render_widget(blk, a);
        let [map, legend] = Layout::vertical([Min(1), Length(1)]).areas(inner);
        let hot: Vec<(&Path, Color)> =
            self.recs.iter().flat_map(|r| r.paths.iter().map(move |p| (p.as_path(), if r.safe { SAFE } else { WARN }))).collect();
        let hf = |p: &Path| hot.iter().find(|(q, _)| p.starts_with(q)).map(|x| x.1);
        let tiles = treemap::draw(f.buffer_mut(), map, cur, self.list.selected(), &hf);
        let mut cats: Vec<_> = crate::rules::Cat::ALL.into_iter().filter(|c| cur.kids.iter().any(|k| k.cat == *c)).collect();
        cats.truncate(6);
        let lg: Vec<Span> = cats.iter().flat_map(|c| [" ■ ".fg(c.col(1.0)), c.label().fg(DIM)]).collect();
        f.render_widget(Line::from(lg), legend);
        self.tiles = tiles;
    }

    fn recs(&mut self, f: &mut Frame, a: Rect) {
        let blk = card(Line::from(" Recommendations ".fg(TXT).bold()), self.on_recs);
        let inner = blk.inner(a);
        f.render_widget(blk, a);
        let [list, sum] = Layout::vertical([Min(1), Length(2)]).areas(inner);
        self.rec_area = a;
        let w = list.width as usize;
        let mut items = vec![];
        let mut at = 0;
        for (i, r) in self.recs.iter().enumerate() {
            if i == 0 || r.safe != self.recs[i - 1].safe {
                let (t, c) = if r.safe { (" Safe to remove ", SAFE) } else { (" Review first ", WARN) };
                let tot: u64 = self.recs.iter().filter(|x| x.safe == r.safe).map(|x| x.size).sum();
                let ts = format!("{} ", human(tot));
                let pad = w.saturating_sub(t.len() + ts.len() + 1);
                let hdr = Line::from(vec![" ".into(), t.fg(c).bg(Color::Rgb(32, 36, 42)), " ".repeat(pad).into(), ts.fg(DIM)]);
                items.push(ListItem::new(Text::from(vec![Line::raw(""), hdr])));
            }
            if i == self.rsel {
                at = items.len();
            }
            let sz = format!("{:>9} ", human(r.size));
            let (mark, mc) = if r.on { ("■", if r.safe { SAFE } else { WARN }) } else { ("□", DIM) };
            let lw = w.saturating_sub(4 + sz.len());
            let mut p = r.paths.first().map(|p| tilde(&self.home, p)).unwrap_or_default();
            if r.paths.len() > 1 {
                p += &format!("  +{} more", r.paths.len() - 1);
            }
            items.push(ListItem::new(Text::from(vec![
                Line::from(vec![format!(" {mark}  ").fg(mc), fit(&r.label, lw).fg(TXT), sz.fg(TXT)]),
                Line::from(fit(&format!("    {p}"), w).fg(DIM)),
            ])));
        }
        if items.is_empty() {
            f.render_widget(Paragraph::new("\n  nothing to clean up".fg(DIM)), list);
        } else {
            self.rlist.select(Some(at));
            let hs = if self.on_recs { Style::new().bg(SEL) } else { Style::new() };
            f.render_stateful_widget(List::new(items).highlight_style(hs), list, &mut self.rlist);
        }
        let picked: Vec<&Rec> = self.picked().collect();
        let tot: u64 = picked.iter().map(|r| r.size).sum();
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![" ".into(), human(tot).fg(TXT).bold(), format!(" selected · {} items", picked.len()).fg(DIM)]),
                Line::from(vec![" x".fg(ACC).bold(), " script  ".fg(DIM), "d".fg(ACC).bold(), " delete  ".fg(DIM), "a".fg(ACC).bold(), " select safe".fg(DIM)]),
            ]),
            sum,
        );
    }

    fn footer(&mut self, f: &mut Frame, a: Rect) {
        if let Some((s, c, t)) = &self.toast {
            if t.elapsed() < Duration::from_secs(4) {
                f.render_widget(Line::from(vec![" ● ".fg(*c), s.clone().fg(TXT)]), a);
                return;
            }
            self.toast = None;
        }
        let keys = [
            ("↑↓", "move"),
            ("→", "open"),
            ("←", "back"),
            ("tab", "pane"),
            ("space", "toggle"),
            ("d", "delete"),
            ("o", "reveal"),
            ("t", "map"),
            ("r", "rescan"),
            ("?", "help"),
            ("q", "quit"),
        ];
        let l: Vec<Span> = keys.iter().flat_map(|(k, d)| [format!(" {k}").fg(ACC).bold(), format!(" {d} ").fg(DIM)]).collect();
        f.render_widget(Line::from(l), a);
    }

    fn modal(&self, f: &mut Frame) {
        let Some(m) = &self.modal else { return };
        let (title, col, lines): (&str, Color, Vec<Line>) = match m {
            Modal::Help => {
                let rows = [
                    ("↑ ↓  j k", "move"),
                    ("→ ⏎  l", "open folder / jump to item"),
                    ("← ⌫  h", "go up"),
                    ("g G", "top / bottom"),
                    ("tab", "switch pane"),
                    ("space", "toggle recommendation"),
                    ("a", "toggle all safe items"),
                    ("d", "delete selection (asks first)"),
                    ("x", "write ./sift-cleanup.sh"),
                    ("o", "reveal in file manager"),
                    ("t", "toggle treemap"),
                    ("r", "rescan"),
                    ("q", "quit"),
                ];
                let mut v = vec![Line::raw("")];
                v.extend(rows.iter().map(|(k, d)| Line::from(vec![format!("  {k:<10}").fg(ACC).bold(), d.fg(TXT)])));
                (" keys ", ACC, v)
            }
            Modal::Del(ps, sz) => {
                let mut v = vec![
                    Line::raw(""),
                    Line::from(vec![
                        "  Permanently delete ".fg(TXT),
                        format!("{} item{}", ps.len(), if ps.len() == 1 { "" } else { "s" }).fg(TXT).bold(),
                        " · ".fg(DIM),
                        human(*sz).fg(BAD).bold(),
                    ]),
                    Line::raw(""),
                ];
                v.extend(ps.iter().take(8).map(|p| Line::from(format!("  {}", tilde(&self.home, p)).fg(DIM))));
                if ps.len() > 8 {
                    v.push(Line::from(format!("  +{} more", ps.len() - 8).fg(DIM)));
                }
                v.extend([Line::raw(""), Line::from(vec!["  y".fg(BAD).bold(), " delete   ".fg(DIM), "any key".fg(TXT).bold(), " cancel".fg(DIM)])]);
                (" delete ", BAD, v)
            }
        };
        let a = center(f.area(), 64, lines.len() as u16 + 3);
        f.render_widget(Clear, a);
        let blk = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(col))
            .title(Line::from(title.fg(col).bold()));
        f.render_widget(Paragraph::new(lines).block(blk), a);
    }
}

fn bar(r: f64, w: usize) -> String {
    const P: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];
    let e = (r.clamp(0.0, 1.0) * (w * 8) as f64).round() as usize;
    let mut s: String = "█".repeat(e / 8);
    if !e.is_multiple_of(8) {
        s.push(P[e % 8 - 1]);
    }
    let n = s.chars().count();
    s + &" ".repeat(w - n)
}
