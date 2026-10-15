use crate::scan::Node;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Cat {
    Media,
    Code,
    Sdk,
    Ai,
    Cache,
    Sys,
    Other,
}

impl Cat {
    pub const ALL: [Cat; 7] = [Cat::Media, Cat::Code, Cat::Sdk, Cat::Ai, Cat::Cache, Cat::Sys, Cat::Other];

    pub fn label(self) -> &'static str {
        match self {
            Cat::Media => "Documents & media",
            Cat::Code => "Code projects",
            Cat::Sdk => "iOS & Android SDKs",
            Cat::Ai => "AI models",
            Cat::Cache => "Package caches",
            Cat::Sys => "System & apps",
            Cat::Other => "Dev tools & other",
        }
    }

    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            Cat::Media => (240, 150, 105),
            Cat::Code => (160, 145, 250),
            Cat::Sdk => (90, 200, 205),
            Cat::Ai => (235, 125, 175),
            Cat::Cache => (105, 155, 250),
            Cat::Sys => (120, 125, 140),
            Cat::Other => (165, 170, 180),
        }
    }

    /// col scaled by k (0..1), for dim tile bg
    pub fn col(self, k: f32) -> Color {
        let (r, g, b) = self.rgb();
        let f = |c: u8| (c as f32 * k) as u8;
        Color::Rgb(f(r), f(g), f(b))
    }
}

pub fn classify(name: &str, dir: bool) -> Option<Cat> {
    use Cat::*;
    if !dir {
        let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
        return match ext.as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "heic" | "webp" | "tiff" | "raw" | "dng" | "psd" | "mp4" | "mov" | "mkv"
            | "avi" | "mp3" | "wav" | "flac" | "m4a" | "aac" | "pdf" | "epub" => Some(Media),
            "gguf" | "safetensors" | "ckpt" | "onnx" | "pth" => Some(Ai),
            _ => None,
        };
    }
    Some(match name {
        "Documents" | "Pictures" | "Movies" | "Music" | "Downloads" | "Photos Library.photoslibrary" => Media,
        ".ollama" | ".lmstudio" | "lm-studio" | "huggingface" => Ai,
        ".npm" | ".cache" | "Caches" | ".gradle" | ".cargo" | ".pnpm-store" | "pnpm" | ".yarn" | ".bun"
        | "node_modules" | ".m2" | ".pub-cache" | "CocoaPods" | ".venv" | "DerivedData" => Cache,
        "Android" | ".android" | "Xcode" | "CoreSimulator" | "Developer" | "iOS DeviceSupport" => Sdk,
        "Library" | "Applications" | "System" | ".Trash" => Sys,
        _ => return None,
    })
}

#[derive(Deserialize, Clone)]
pub struct Rule {
    pub label: String,
    /// paths rel to home
    #[serde(default)]
    pub paths: Vec<String>,
    /// dir names matched anywhere
    #[serde(default)]
    pub names: Vec<String>,
    /// sibling req'd for a name match, e.g. Cargo.toml next to target
    #[serde(default)]
    pub sib: Option<String>,
    #[serde(default)]
    pub safe: bool,
}

fn r(label: &str, safe: bool, paths: &[&str], names: &[&str], sib: Option<&str>) -> Rule {
    let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect();
    Rule { label: label.into(), paths: v(paths), names: v(names), sib: sib.map(Into::into), safe }
}

pub fn defaults() -> Vec<Rule> {
    vec![
        r("npm cache", true, &[".npm"], &[], None),
        r("pnpm, Yarn & Bun caches", true, &["Library/pnpm", ".pnpm-store", "Library/Caches/Yarn", ".yarn/berry/cache", ".bun/install/cache"], &[], None),
        r("Next.js & Turbo build output", true, &[], &[".next", ".turbo"], None),
        r("Rust target dirs", true, &[], &["target"], Some("Cargo.toml")),
        r("Gradle caches", true, &[".gradle/caches"], &[], None),
        r("Cargo registry & git cache", true, &[".cargo/registry", ".cargo/git"], &[], None),
        r("Xcode DerivedData", true, &["Library/Developer/Xcode/DerivedData"], &[], None),
        r("uv, pip & Puppeteer caches", true, &[".cache/uv", ".cache/pip", "Library/Caches/pip", ".cache/puppeteer", "Library/Caches/ms-playwright"], &[], None),
        r("Homebrew & Go caches", true, &["Library/Caches/Homebrew", "Library/Caches/go-build", ".cache/go-build"], &[], None),
        r("Trash", true, &[".Trash"], &[], None),
        r("Android SDK & emulators", false, &["Library/Android/sdk", ".android/avd"], &[], None),
        r("Ollama models", false, &[".ollama/models"], &[], None),
        r("Hugging Face & LM Studio models", false, &[".cache/huggingface", ".lmstudio/models", ".cache/lm-studio"], &[], None),
        r("node_modules", false, &[], &["node_modules"], None),
        r("Python venvs", false, &[], &[".venv"], None),
        r("iOS simulators & device support", false, &["Library/Developer/CoreSimulator", "Library/Developer/Xcode/iOS DeviceSupport", "Library/Developer/Xcode/Archives"], &[], None),
        r("Docker data", false, &["Library/Containers/com.docker.docker"], &[], None),
        r("Rust toolchains", false, &[".rustup/toolchains"], &[], None),
        r("Downloads", false, &["Downloads"], &[], None),
    ]
}

#[derive(Serialize)]
pub struct Rec {
    pub label: String,
    pub safe: bool,
    pub size: u64,
    pub paths: Vec<PathBuf>,
    #[serde(skip)]
    pub on: bool,
}

/// match rules against tree; no recursion into a hit so nothing is double counted
pub fn recommend(root: &Node, home: &Path, rules: &[Rule]) -> Vec<Rec> {
    let abs: Vec<Vec<PathBuf>> = rules.iter().map(|r| r.paths.iter().map(|p| home.join(p)).collect()).collect();
    let mut hits: Vec<(u64, Vec<PathBuf>)> = vec![(0, vec![]); rules.len()];
    visit(root, rules, &abs, &mut hits);
    let mut out: Vec<Rec> = rules
        .iter()
        .zip(hits)
        .filter(|(_, (s, _))| *s > 0)
        .map(|(r, (size, paths))| Rec { label: r.label.clone(), safe: r.safe, size, paths, on: r.safe })
        .collect();
    out.sort_by(|a, b| b.safe.cmp(&a.safe).then(b.size.cmp(&a.size)));
    out
}

fn visit(n: &Node, rules: &[Rule], abs: &[Vec<PathBuf>], hits: &mut [(u64, Vec<PathBuf>)]) {
    for k in n.kids.iter().filter(|k| k.dir) {
        let hit = rules.iter().enumerate().position(|(i, r)| {
            abs[i].contains(&k.path)
                || (r.names.contains(&k.name)
                    && r.sib.as_ref().is_none_or(|s| n.kids.iter().any(|x| &x.name == s)))
        });
        match hit {
            Some(i) => {
                hits[i].0 += k.size;
                hits[i].1.push(k.path.clone());
            }
            None => visit(k, rules, abs, hits),
        }
    }
}

#[cfg(test)]
mod t {
    use super::*;

    fn d(name: &str, path: &str, size: u64, kids: Vec<Node>) -> Node {
        Node { name: name.into(), path: path.into(), size, files: 1, dir: true, cat: Cat::Other, kids }
    }

    #[test]
    fn recs() {
        let tree = d("h", "/h", 30, vec![
            d(".npm", "/h/.npm", 10, vec![d("node_modules", "/h/.npm/node_modules", 5, vec![])]),
            d("p", "/h/p", 20, vec![
                d("target", "/h/p/target", 7, vec![]),
                d("node_modules", "/h/p/node_modules", 13, vec![d("node_modules", "/h/p/node_modules/node_modules", 3, vec![])]),
            ]),
        ]);
        let rs = recommend(&tree, Path::new("/h"), &defaults());
        assert_eq!(rs.len(), 2);
        assert_eq!((rs[0].label.as_str(), rs[0].size, rs[0].on), ("npm cache", 10, true));
        assert_eq!((rs[1].label.as_str(), rs[1].size, rs[1].on), ("node_modules", 13, false));
        assert_eq!(classify("x.GGUF", false), Some(Cat::Ai));
    }
}
