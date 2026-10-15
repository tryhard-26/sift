use crate::rules::{classify, Cat};
use rayon::prelude::*;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use walkdir::WalkDir;

const MARKERS: [&str; 7] = [".git", "Cargo.toml", "package.json", "go.mod", "pyproject.toml", "pom.xml", "build.gradle"];

pub struct Node {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub files: u64,
    pub dir: bool,
    pub cat: Cat,
    pub kids: Vec<Node>,
}

#[derive(Default)]
pub struct Prog {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub errs: AtomicU64,
}

pub fn scan(root: &Path, ex: &[String], p: &Prog) -> Node {
    let dev = std::fs::metadata(root).map(|m| dev(&m)).unwrap_or(0);
    walk(root, root.display().to_string(), Cat::Other, dev, ex, p)
}

fn walk(path: &Path, name: String, cat: Cat, dv: u64, ex: &[String], p: &Prog) -> Node {
    let ents: Vec<_> = WalkDir::new(path)
        .min_depth(1)
        .max_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.map_err(|_| p.errs.fetch_add(1, Relaxed)).ok())
        .filter(|e| !ex.iter().any(|x| e.file_name() == x.as_str()))
        .collect();
    let cat = if cat == Cat::Other && ents.iter().any(|e| MARKERS.iter().any(|m| e.file_name() == *m)) {
        Cat::Code
    } else {
        cat
    };
    let mut kids: Vec<Node> = ents
        .par_iter()
        .filter_map(|e| {
            let md = e.metadata().map_err(|_| p.errs.fetch_add(1, Relaxed)).ok()?;
            let nm = e.file_name().to_string_lossy().into_owned();
            let c = classify(&nm, md.is_dir()).unwrap_or(cat);
            if md.is_dir() {
                // stay on one fs
                if dev(&md) != dv {
                    return None;
                }
                return Some(walk(e.path(), nm, c, dv, ex, p));
            }
            let sz = disk_size(&md);
            p.files.fetch_add(1, Relaxed);
            p.bytes.fetch_add(sz, Relaxed);
            Some(Node { name: nm, path: e.path().to_path_buf(), size: sz, files: 1, dir: false, cat: c, kids: vec![] })
        })
        .collect();
    kids.sort_unstable_by(|a, b| b.size.cmp(&a.size));
    Node {
        name,
        path: path.to_path_buf(),
        size: kids.iter().map(|k| k.size).sum(),
        files: kids.iter().map(|k| k.files).sum(),
        dir: true,
        cat,
        kids,
    }
}

// ponytail: hardlinks counted per link, track (dev, ino) if totals matter
#[cfg(unix)]
fn disk_size(m: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.blocks() * 512
}

#[cfg(not(unix))]
fn disk_size(m: &Metadata) -> u64 {
    m.len()
}

#[cfg(unix)]
fn dev(m: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    m.dev()
}

#[cfg(not(unix))]
fn dev(_: &Metadata) -> u64 {
    0
}

/// rm a path from the tree, returns (bytes, files) removed
pub fn prune(n: &mut Node, p: &Path) -> (u64, u64) {
    let r = if let Some(i) = n.kids.iter().position(|k| k.path == p) {
        let k = n.kids.remove(i);
        (k.size, k.files)
    } else if let Some(k) = n.kids.iter_mut().find(|k| k.dir && p.starts_with(&k.path)) {
        prune(k, p)
    } else {
        (0, 0)
    };
    n.size -= r.0;
    n.files -= r.1;
    n.kids.sort_by(|a, b| b.size.cmp(&a.size));
    r
}

/// idx path to deepest existing node on the way to `p`
pub fn locate(mut n: &Node, p: &Path) -> Vec<usize> {
    let mut out = vec![];
    while let Some(i) = n.kids.iter().position(|k| k.dir && p.starts_with(&k.path)) {
        out.push(i);
        n = &n.kids[i];
    }
    out
}

#[cfg(unix)]
pub fn disk(p: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let f = s.f_frsize as u64;
    Some((s.f_bavail as u64 * f, s.f_blocks as u64 * f))
}

#[cfg(not(unix))]
pub fn disk(_: &Path) -> Option<(u64, u64)> {
    None
}

pub fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1000.0 && i < 4 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

#[cfg(test)]
mod t {
    use super::*;

    #[test]
    fn scan_prune_locate() {
        let d = std::env::temp_dir().join(format!("sift-t-{}", std::process::id()));
        std::fs::create_dir_all(d.join("a/node_modules/x")).unwrap();
        std::fs::write(d.join("a/package.json"), "{}").unwrap();
        std::fs::write(d.join("a/node_modules/x/f"), vec![1u8; 9000]).unwrap();
        std::fs::write(d.join("b.png"), vec![1u8; 100]).unwrap();
        let mut n = scan(&d, &[], &Prog::default());
        assert_eq!(n.files, 3);
        let a = &n.kids[0];
        assert_eq!((a.name.as_str(), a.cat), ("a", Cat::Code));
        assert_eq!(a.kids.iter().find(|k| k.name == "node_modules").unwrap().cat, Cat::Cache);
        assert_eq!(n.kids[1].cat, Cat::Media);
        let nm = d.join("a/node_modules");
        assert_eq!(locate(&n, &nm.join("x")).len(), 3);
        let (b, f) = prune(&mut n, &nm);
        assert!(b > 0 && f == 1 && n.files == 2);
        assert_eq!(human(1_500_000), "1.5 MB");
        std::fs::remove_dir_all(d).unwrap();
    }
}
