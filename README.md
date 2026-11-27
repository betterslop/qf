<p align="center">
  <img src=".github/assets/banner.png" alt="qf" width="640">
</p>

# qf

Search code from the terminal, see each hit in a live preview, and press Enter to open it in your editor at that line.

The usual terminal setup pipes one regex into a fuzzy finder, so `foo bar` finds `foo bar` in that order and nothing else. qf runs the search itself with the ripgrep crates. `foo bar -test` finds lines with `foo` and `bar` in any order, but not `test`. Add `path:src` or `git:changed`, and the preview keeps up while you type.

![qf: a live search with preview, a regex, a path: filter from the file picker, and grouped results](.github/assets/demo.gif)

## Install

```sh
cargo install --locked --git https://github.com/betterslop/qf
```

qf needs a Unix system (macOS or Linux).

## Use

```sh
qf                    # open, then type
qf 'fn main'          # start with a query (quote it if it has spaces)
qf TODO src tests     # search only src/ and tests/
qf -t rust unwrap     # only Rust files
qf --changed          # only files changed in git
```

Type to search. Hits show above the prompt, with a preview next to them. `Enter` opens the hit.

```
unwrap path:src -test       unwrap, under src/, on lines without "test"
(load OR save) ext:rs       load or save, in .rs files
TODO git:changed            TODO in the files you changed
```

The query syntax is the same in every betterslop tool: [QUERY.md](https://github.com/betterslop/bs-tui-common/blob/main/QUERY.md). The shared betterslop keys work as the [KEYS.md spec](https://github.com/betterslop/bs-tui-common/blob/main/KEYS.md) says. qf types from the start, so `?` opens the help at an empty prompt, and `j`/`k`/`q` work after `Esc` (vim normal mode). These keys are special to qf:

| Key | Does |
|---|---|
| `Enter` | Open the hit, or the marked hits as a quickfix list |
| `Tab` / `Shift-Tab` | Mark the hit and move to the next / previous hit |
| `Ctrl-q` | Send all hits to the quickfix list |
| `Ctrl-o` | Menu of your `[open.NAME]` commands |
| `Ctrl-j` / `Ctrl-k`, arrows | Move the selection while typing |
| `Shift-Up` / `Shift-Down` | Scroll the preview |
| `Ctrl-/` | Show or hide the preview |
| `Ctrl-r` | Search your query history |
| `@` | Fuzzy pick a file or folder, which becomes a `path:` field |
| `:` | Command line, at an empty prompt or in normal mode (`:sort modified`, `:context 2`, `:set group`) |

Settings are `:` commands, not Alt keys, so every key works in a default macOS terminal. [GUIDE.md](GUIDE.md) has every key, flag, command and config key.

## Config

| What | Where |
|---|---|
| Config file | `~/.config/qf/config.toml` (or `$XDG_CONFIG_HOME/qf/config.toml`). An unknown key is an error. `qf --help` prints an example. |
| History | `~/.local/state/qf/history` (or `$XDG_STATE_HOME/qf/history`) |
| Editor | config `editor`, then `$EDITOR`, then `nvim`. For editors other than vim, use a template: `editor = "code -g {file}:{line}:{col}"`. When the editor exits, you are back in qf. `editor_return = false` exits qf instead. |
| No colors | `NO_COLOR=1` |
| Light or dark | `TUI_THEME=dark` or `TUI_THEME=light` (beats the `theme` config key). Default: from the terminal. |

A search stops at 10,000 hits. The preview skips files over 4 MB.

## Build from source

```sh
cargo build --release    # target/release/qf
cargo test --release
```

## License

[MIT](LICENSE)

<br>
<br>

<table align="center">
  <tr>
    <td valign="middle"><a href="https://github.com/betterslop"><img src=".github/assets/betterslop.png" alt="Better Slop Industries" width="60" align="left"></a></td>
    <td valign="middle"><small>Another quality project from <a href="https://github.com/betterslop">Better Slop Industries</a><br>
    See our other work on <a href="https://github.com/betterslop">GitHub</a></small></td>
  </tr>
</table>
