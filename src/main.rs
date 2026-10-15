mod rules;
mod scan;
mod treemap;
mod ui;

use clap::Parser;
use rules::{Cat, Rec, Rule};
use scan::{Node, Prog};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::{env, fs};

#[derive(Parser)]
#[command(name = "sift", version, about = "Explore disk usage and reclaim space")]
struct Cli {
    /// Directory to scan [default: home]
    path: Option<PathBuf>,
    /// Print a JSON report instead of opening the UI
    #[arg(long)]
    json: bool,
    /// Tree depth in the JSON report
    #[arg(long, default_value_t = 2)]
    depth: usize,
    /// Skip entries with this name (repeatable)
    #[arg(short = 'x', long = "exclude", value_name = "NAME")]
    exclude: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Config {
    exclude: Vec<String>,
    rules: Vec<Rule>,
}

#[derive(Serialize)]
struct JNode<'a> {
    name: &'a str,
    size: u64,
    files: u64,
    cat: Cat,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    kids: Vec<JNode<'a>>,
}

#[derive(Serialize)]
struct Report<'a> {
    root: &'a Path,
    size: u64,
    files: u64,
    unreadable: u64,
    free: Option<u64>,
    total: Option<u64>,
    recommendations: Vec<Rec>,
    tree: JNode<'a>,
}

fn jn(n: &Node, d: usize) -> JNode<'_> {
    let kids = if d == 0 { vec![] } else { n.kids.iter().take(50).map(|k| jn(k, d - 1)).collect() };
    JNode { name: &n.name, size: n.size, files: n.files, cat: n.cat, kids }
}

fn die(s: String) -> ! {
    eprintln!("sift: {s}");
    exit(1)
}

fn main() {
    let cli = Cli::parse();
    let home: PathBuf = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")).unwrap_or(".".into()).into();
    let home = home.canonicalize().unwrap_or(home);
    let cdir = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let cp = cdir.join("sift/config.json");
    let cfg: Config = match fs::read_to_string(&cp) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| die(format!("{}: {e}", cp.display()))),
        Err(_) => Config::default(),
    };
    let root = cli.path.unwrap_or_else(|| home.clone());
    let root = root.canonicalize().unwrap_or_else(|e| die(format!("{}: {e}", root.display())));
    if !root.is_dir() {
        die(format!("{}: not a directory", root.display()));
    }
    let mut ex = cli.exclude;
    ex.extend(cfg.exclude);
    let mut rules = rules::defaults();
    rules.extend(cfg.rules);

    if cli.json {
        let p = Prog::default();
        let n = scan::scan(&root, &ex, &p);
        let disk = scan::disk(&root);
        let r = Report {
            root: &root,
            size: n.size,
            files: n.files,
            unreadable: p.errs.into_inner(),
            free: disk.map(|d| d.0),
            total: disk.map(|d| d.1),
            recommendations: rules::recommend(&n, &home, &rules),
            tree: jn(&n, cli.depth),
        };
        println!("{}", serde_json::to_string_pretty(&r).unwrap());
        return;
    }
    if let Err(e) = ui::run(root, home, ex, rules) {
        die(e.to_string());
    }
}
