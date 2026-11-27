# qf

`qf` (Quick Find) is a code search TUI in Rust. The user types a query, sees
hits with a live, syntax-highlighted preview, and presses Enter to open the hit
in an editor at the line. It searches in-process with the ripgrep crates
(`ignore`, `grep-searcher`, `grep-regex`). License: MIT.

User docs: `README.md` (short) and `GUIDE.md` (every key, flag, command and
config key). Keep `GUIDE.md` in sync with the code.

## Files

| File | Holds |
|---|---|
| `src/main.rs` | CLI args (clap), startup, event loop (one channel, no polling). Runs the `Launch` from `Action::Open` after the TUI closes. For `Action::Run`, pauses the input thread, closes the TUI, waits for the command, and shows the TUI again |
| `src/app.rs` | App state and all key handling (insert, normal, menus, command line, help), sort, narrowing, marks |
| `src/search.rs` | Search. Cached file list + `notify` watcher, rayon over the list in 64-file steps (stable 10,000 hit limit, `MAX_HITS`), relevance score per hit, context lines (custom `Sink`), git status cache |
| `src/query.rs` | Query evaluation. `bs_tui_common::query::parse` parses; this file evaluates AND/OR/NOT per line, or per file in file mode. Fields `path: ext: type: git: sort:`; unknown or partial values match nothing and set `Query::hint`. Case comes from the parsed `Pattern.ignore_case`. Matching runs on the raw file line; `remap` maps ranges onto drawn text |
| `src/syntax.rs` | One scan of the prompt text: colors, paren pairs, quotes, problems (`missing )`, `unclosed "`) |
| `src/complete.rs` | Completion candidates: fields, values and operators from `bs_tui_common::filter::field_candidates`, plus paths, words from hits, history. `@`/`path:` and `Ctrl-r` use `nucleo-matcher`. The data types, popup keys and popup widget are `bs_tui_common::complete` |
| `src/command.rs` | qf's own `:` commands: table and parse. The line, unique prefixes, completion and the shared `:theme` `:help` `:quit` are `bs_tui_common::cmdline` |
| `src/editor.rs` | Line editor with vim motions and text objects |
| `src/open.rs` | Command lines for the editor and `[open.NAME]` commands (default `+LINE FILE`, `-q` quickfix file for several hits, `{file} {line} {col} {text}` templates, `stdin` template). `spawn` (background, qf stays), `wait` (in the terminal, then back to qf) and `run` (exec, or pipe and wait) |
| `src/preview.rs` | Preview worker thread: syntect highlighting, per-file cache (mtime checked), theme switch |
| `src/theme.rs` | Embedded themes (Monokai Extended Bright / Light), `auto` detection via `bs_tui_common::theme::init_with`, `Palette` for the syntax theme (preview, selection) and prompt styles built from the shared roles; every other color comes from `bs_tui_common::theme::palette()` |
| `src/git.rs` | `git status --porcelain`, branch name |
| `src/history.rs` | Query history file |
| `src/config.rs` | `config.toml` (serde, `deny_unknown_fields`), `[open.NAME]` tables and their key check, `EXAMPLE` text for `--help` |
| `src/ui/` | `mod.rs` layout (header, prompt line, results and preview as bordered panes via `frame::pane`, status bar as the last row; auto/horizontal/vertical), `list.rs` results, `preview.rs`, `prompt.rs` (prompt, status bar, `:` line on the status bar row, completion menus), `header.rs` top bar, `help.rs` (`KEYS` text, shared with `--help`), `splash.rs` start logo, `widgets.rs` (`Message`, `Popup`, helpers) |
| `build.rs` | Compiles syntect's default grammars + `assets/syntaxes/*` into one dump that `preview.rs` embeds |
| `assets/` | Extra grammars (TypeScript, TSX, TOML, Dockerfile, Swift, Kotlin, Nix, Terraform, Zig), each with its license, and the two themes (MIT) |
| `.claude/docs/plans/` | Old implementation plans. History only |
| `.github/assets/` | `banner.png` (README top), `demo.gif` (README demo) and `betterslop.png` (footer logo) |
| `demo/` | Git-ignored, local only. `make-repo.py` writes a synthetic repo, `demo.tape` is the VHS script, `record.sh` builds, records and shrinks `demo.gif`. Needs `vhs`, `ffmpeg` and Hack Nerd Font Mono |

## Build, test, lint

```sh
cargo build --release                 # target/release/qf
cargo test --release
cargo clippy --release --all-targets
cargo fmt --check
cargo test --release -- --ignored --nocapture fuzzy_speed   # picker timing, 100k paths
```

CI (`.github/workflows/ci.yml`, Ubuntu and macOS) runs `cargo fmt --check`,
`cargo clippy --all-targets --locked -- -D warnings` and `cargo test --locked`.

## Release

- cargo-dist 0.34.0. Config in `dist-workspace.toml`, profile `[profile.dist]`
  in `Cargo.toml`. Targets: macOS and Linux, arm64 and x86_64. Shell installer.
- `.github/workflows/release.yml` is generated. Do not edit it by hand. After
  a change to `dist-workspace.toml`, run `dist generate`.
- `release.yml` runs `dist plan` on pull requests. A pushed version tag (for
  example `v0.1.0`) builds the binaries and publishes the GitHub release.

## Check the TUI by hand

```sh
tmux new-session -d -s t -x 120 -y 20 -c "$PWD" "XDG_CONFIG_HOME=/nonexistent ./target/release/qf; sleep 30"
tmux send-keys -t t -l 'fn main'
tmux capture-pane -p -t t             # add -e to see colors
```

- `XDG_CONFIG_HOME=/nonexistent` gives the default settings.
- tmux sends two fast `Esc` presses as one `Alt-Esc`. Pause between them.
- An old `qf` may be first on `PATH`. Run `./target/release/qf`.

## Shared look and keys

qf follows the bs-tui-common specs:
[QUERY.md](https://github.com/betterslop/bs-tui-common/blob/main/QUERY.md),
[KEYS.md](https://github.com/betterslop/bs-tui-common/blob/main/KEYS.md),
[LOOK.md](https://github.com/betterslop/bs-tui-common/blob/main/LOOK.md).

- `bs-tui-common` is a git dependency from
  https://github.com/betterslop/bs-tui-common. qf uses its query parser, theme
  detection and shared roles (list matches use `palette().matched`; `:theme` calls `theme::switch`), `term` (init, restore, `main` for `error: …`), help
  popup, `Ctrl-o` menu, `:` command line (`cmdline`) and completion popup (`complete`).
- Change a shared behavior in the spec and the crate first, not in this repo.

## Rules and gotchas

- Use only popular, well-known crates.
- Follow established tools. Do not invent one-off keys. Insert mode follows
  fzf and readline. Normal mode follows vim. Settings are `:` commands (vim
  `:set`). Helpers follow Claude Code (`@`, and `:` / `?` at an empty prompt).
- No `Alt` key bindings. Option does not send Alt in default macOS terminals.
- Keep the UI quiet. Speed and quality are the goals.
- `CTRL_USED` in `src/config.rs` must match the Ctrl keys that `src/app.rs`
  handles. Free Ctrl keys for `[open.NAME]`: g s t v x z.
- `ui/help.rs` `KEYS` is both the help popup and part of `qf --help`. Change
  them together with `GUIDE.md`.
- Enter always runs `editor`. By default qf waits for it and then shows the
  TUI again. `editor_return = false` makes qf replace itself with it (exec). `[open.NAME]` commands
  run with `Ctrl-o` (popup menu), `:open NAME` or their key. `stay` defaults
  to true. qf does not use a shell: it splits `cmd` at spaces. There is no
  built-in clipboard.
- Prompt at the bottom by default; hits grow up from it. The first hit is next
  to the prompt. In screen order, `gg` / `G` select the hit at the top / bottom
  end of the whole list, not of the visible rows.
- `.gitignore` applies also outside git repos (`require_git(false)`).
- Menus: an exact match stays in the list. A space closes the menu (not the
  `Ctrl-r` history menu). Enter on an item that equals the typed word opens
  the hit.
- Relevance: file rank = best line score, then hit count (max 20), then mtime.
  Line score rises per match, more at a word start, most for a whole word,
  plus a bonus when the file name matches.

## Known limits

- Search stops at 10,000 hits. Before the file list is ready, walk order
  decides which.
- Operators work per line, except in file mode.
- Preview skips files over 4 MB. Highlighting stops after 20,000 lines.
- A narrowed search (adding an AND term) does not see files created after the
  last full search.
- The git branch and theme `auto` are read once at startup.

## README

Keep `README.md` short, plain and humble, and true to the code: say what qf
does, with no backstory. Keep the banner and the footer unchanged. After a
change to the screen or the keys, run `demo/record.sh` (if you have the local
`demo/` folder) to record `demo.gif` again, and check its frames. Demo data
must stay synthetic.
