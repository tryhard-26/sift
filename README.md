# sift

Interactive disk usage explorer for the terminal. Scans a directory in parallel, shows it as a navigable list and treemap, and suggests what can be cleaned up.

## Install

```
cargo install sift-disk
```

The binary is called `sift`.

## Usage

```
sift              # scan home
sift ~/code       # scan a directory
sift --json       # print a JSON report instead of the UI
sift -x node_modules -x .git
```

Keys: arrows or `hjkl` to move and open, `tab` to switch panes, `space` to toggle a recommendation, `a` to toggle all safe items, `d` to delete (asks first), `x` to write `sift-cleanup.sh`, `o` to reveal in the file manager, `t` to toggle the treemap, `r` to rescan, `q` to quit. Mouse clicks work on the list and treemap.

`~/Library/CloudStorage` and `Mobile Documents` are skipped because they are backed by cloud storage.

## Config

Optional, at `~/.config/sift/config.json`:

```json
{
  "exclude": ["Steam"],
  "rules": [
    { "label": "Unity cache", "paths": ["Library/Unity/cache"], "safe": true },
    { "label": "Zig cache", "names": [".zig-cache"], "safe": true }
  ]
}
```

`paths` are relative to home. `names` match a directory name anywhere; add `"sib": "file"` to require a sibling file.

## License

Unlicense
