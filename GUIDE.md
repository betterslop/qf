# qf guide

`qf` (Quick Find) is an interactive code search for the terminal. Type a query, see matching
lines with a live preview, and press Enter to open the hit in your editor at that line.

`qf` searches in its own process with the ripgrep crates. It respects
`.gitignore`. It searches again as you type and starts no other programs.

```
 qf  ~/repos/myapp · main · 80 files
┌ Hits 3 ───────────────────────────────┐┌ src/ui/list.rs:42 ── 2 matches · Rust ┐
│ src/ui/list.rs:42: fn draw(f: &mut Fr…││ 40 /// The results list.              │
│ src/ui/list.rs:17: fn rows(app: &App)…││ 41                                    │
│ src/main.rs:66: fn main() -> Result<(…││ 42 pub fn draw(f: &mut Frame, app: &m…│
└───────────────────────────────────────┘└───────────────────────────────────────┘
> fn path:src
 3 hits · 2/80 files · 4 ms · F1 help
```

The results and the preview are bordered panes. The prompt and the status bar sit outside them.

## Contents

- [Install](#install)
- [Start](#start)
- [Query syntax](#query-syntax)
- [Fields: filter files](#fields-filter-files)
- [File mode](#file-mode)
- [Context lines](#context-lines)
- [Git](#git)
- [Sort](#sort)
- [Which files to search](#which-files-to-search)
- [Editing the prompt](#editing-the-prompt)
- [Completion](#completion)
- [Commands](#commands)
- [Results and preview](#results-and-preview)
- [Open hits](#open-hits)
- [Layout and themes](#layout-and-themes)
- [Config file](#config-file)
- [Reference](#reference)
- [Limits](#limits)

## Install

```sh
cargo install --locked --git https://github.com/betterslop/qf
```

This puts `qf` in `~/.cargo/bin`. From a clone, `cargo install --path .` does the
same. Or build with `cargo build --release` and copy `target/release/qf` to a
folder on your `PATH`.

## Start

```sh
qf                         # open, then type
qf 'fn main'               # start with a query (quote it if it has spaces)
qf TODO src tests          # search only src/ and tests/
qf -t rust 'unwrap'        # only Rust files
qf --changed               # list the files changed in git
```

The prompt is at the bottom. Hits grow up from it, so the first hit is next to
the prompt. Type to search. The search starts 30 ms after you stop typing.

- `Enter` opens the selected hit.
- `Ctrl-l` clears the query.
- `?` at an empty prompt (or `F1`) shows all keys.
- `Ctrl-c` quits.

## Query syntax

The default query syntax has boolean operators. Each term is a regex. cloudtail,
annals and actionator use the same syntax (spec:
[bs-tui-common QUERY.md](https://github.com/betterslop/bs-tui-common/blob/main/QUERY.md)).

| Query | Finds lines with |
|---|---|
| `foo bar` | `foo` and `bar` (any order) |
| `foo AND bar` | the same |
| `foo OR bar` | `foo`, or `bar`, or both |
| `(foo OR bar) AND baz` | `baz`, and `foo` or `bar` |
| `baz foo \| bar` | the same: `\|` binds tighter than a space (fzf) |
| `foo -bar` or `foo !bar` | `foo` but not `bar` |
| `foo AND NOT bar` | the same |
| `NOT foo` | every line without `foo` |
| `"a.b"` or `'a.b'` | the exact text `a.b` (no regex) |
| `'a.b` | the same (fzf exact match) |
| `/fn\s+(new\|load)/` | a regex that can contain spaces |
| `fn.*search` | a regex (bare words are regex) |
| `foo(` or `C++` | the literal text, because it is not a valid regex |
| `^start`, `end$` | regex anchors |

Operators must be uppercase. `qf` changes them for you. When you type the space
after `and`, `or` or `not` between terms, the word becomes `AND`, `OR` or `NOT`.
`foo or bar` becomes `foo OR bar`. A last word stays as typed, so you can still
search for `and`. To search for the word `or` between other terms, quote it
(`"or"`) or set `auto_uppercase = false`.

### Case

Each term uses smart case: a term with a capital letter is case-sensitive.

### What is highlighted

qf matches file lines as they are on disk. The list and the preview draw the line
changed: the list drops the leading whitespace and shows tabs and control
characters as spaces, and the preview expands tabs to 4 spaces. qf maps the match
ranges onto the drawn text, so `/^\s+fn/` and `/foo\tbar/` match and highlight in
both panes.

| Query | Matches |
|---|---|
| `search` | `search`, `Search`, `SEARCH` |
| `Search` | only `Search` |
| `search Status` | any case of `search`, and exactly `Status` |

`qf -I` or `:case ignore` makes all terms case-insensitive.

### Partial input

The parser accepts input that is not finished, so results update as you type.
It closes an open `(` or `"` at the end. It ignores a trailing `OR`, `AND` or `-`. The status bar shows a hint such as `missing )`. The only error is an
invalid `/regex/`.

### Regex mode

Regex mode searches the whole input as one regex.

```sh
qf -r 'fn \w+\('           # regex mode
qf -i 'fn search'          # ivy style: spaces become .*, so this is fn.*search
```

In the app: `:mode regex` / `:mode query`, and `:set ivy` / `:set noivy`.

## Fields: filter files

Fields filter files by path, type or git status. They combine with text terms and
operators.

| Field | Example | Keeps files |
|---|---|---|
| `path:` word | `path:src/ui` | whose path matches the word (a regex, or literal text if it is not one; smart case) |
| `path:` literal | `path:"a.b"` | whose path contains this exact text |
| `path:` regex | `path:/ui.*rs$/` | whose path matches the regex |
| `path:` glob | `path:*.rs`, `path:*.{rs,toml}` | an unquoted value with `*`, `?`, `[` or `{` is a glob. A quoted value is never a glob |
| `ext:` | `ext:rs` or `ext:.rs` | with this extension |
| `type:` | `type:rust` | of this ripgrep type (`rg --type-list`) |
| `git:` | `git:changed` | changed in git (see [Git](#git)) |

Examples:

```
unwrap path:src            unwrap, only under src/
TODO -path:vendor          TODO, not in vendor/
fn main ext:rs             fn and main in .rs files
needle -ext:md -ext:toml   needle, not in Markdown or TOML files
config OR path:docs        lines with config, or any line in docs/
```

A query with **only fields** lists the matching files, one row each:

```
type:markdown              all Markdown files
ext:toml git:changed       changed TOML files
path:*.lock                all lock files
```

A field value that is not complete or not known (`git:c`, `type:ru`, `sort:p`) is not
an error. It matches nothing, and the status bar shows a hint. An empty value
(`git:`) matches all. The only error is an invalid `/regex/`.

A word with an unknown field name stays text, so `http:x` and `std::io` still
search as written.

## File mode

By default, every term must be on the same line. In file mode, terms apply to the
whole file. The results show the lines that match the positive terms.

```
struct impl -Other         line mode: lines with struct and impl (often none)
                           file mode: files with struct and impl, without Other
```

| How | |
|---|---|
| `:mode file` / `:mode query` | switch file mode |
| `qf --file-mode` | start in file mode |
| `file_mode = true` | config |

`NOT foo` in file mode lists the files that do not contain `foo`.

## Context lines

Show lines around each hit, like `rg -C`. Context lines are dim and use `-`:

```
 src/a.rs-3- fn beta() {}
 src/a.rs:4: // needle one
 src/a.rs-5- let y = 2;
```

| How | |
|---|---|
| `qf -C 2` | start with 2 context lines |
| `:context 3` / `:context 0` | set the count / turn off |
| `context = 2` | config |

## Git

| Field | Keeps files that are |
|---|---|
| `git:changed` | modified, staged or untracked |
| `git:modified` | changed in the working tree (tracked files) |
| `git:staged` | changed in the index |
| `git:untracked` | not tracked |

```
TODO git:changed           TODO in the files you changed
git:untracked              list the new files
qf --changed              start with only changed files (works with no query)
```

`:changed` turns the `--changed` filter on and off. Outside a git repository, git
fields match no file.

## Sort

| Order | |
|---|---|
| relevance (default) | the best files first, lines in line order |
| path | by path, then line |
| modified | the most recently changed file first |

Relevance ranks each file by its best line, then by its hit count (up to 20), then
by its last change. A line scores higher for each match, more for a match at the
start of a word, and most for a whole word: `search` in `fn search(` beats
`research`. A file whose name matches a term gets a bonus, so `search` puts
`search.rs` first.

The search stops at 10,000 hits. It searches the files in path order, so the same
search always keeps the same hits.

| How | |
|---|---|
| `sort:path` / `sort:modified` in the query | for this query |
| `qf --sort modified` | at start |
| `:sort modified` | command |
| `sort = "modified"` | config |

## Which files to search

`qf` searches the current folder. It skips hidden files, binary files, and files
in `.gitignore` and `.ignore`, also outside git repositories. The flags work as
in ripgrep:

```sh
qf --hidden foo                 # also hidden files (and .git/)
qf --hidden -g '!.git' foo      # hidden files, but not .git/
qf --no-ignore foo              # ignore .gitignore and .ignore
qf -g '*.rs' -g '!*_test.rs' x  # include and exclude globs
qf -t rust -T toml x            # include and exclude file types
qf x src/ docs/                 # only these folders
```

`qf` reads the file list once. Then it watches the folder and updates the list
when you create, remove or rename a file. When a file changes, `qf` also runs the
search again and keeps the selected hit. `:set nowatch`, `watch = false` or `R` in
normal mode turns this off.

## Editing the prompt

The prompt has vim modes. It starts in insert mode (bar cursor, cyan `>`).
`Esc` enters normal mode (block cursor, yellow `>`, `[normal]` in the status bar).

### Insert mode

| Key | Does |
|---|---|
| typing | edits the query |
| `Left` `Right` `Home` `End` | move the cursor |
| `Ctrl-a` / `Ctrl-e` | start / end of the line |
| `Ctrl-b` / `Ctrl-f` | back / forward one character |
| `Backspace` / `Ctrl-h`, `Delete` | delete before / under the cursor |
| `Ctrl-w` / `Ctrl-u` | delete a word / delete to the start |
| `Ctrl-l` | clear the whole query (also in normal mode) |
| `Ctrl-p` / `Ctrl-n` | older / newer query from history |
| `Ctrl-r` | search the history: type to filter (fuzzy), `Enter` replaces the query |
| `Esc` | normal mode |

Insert mode uses the keys of fzf and readline (bash, zsh). Normal mode uses vim keys.

#### Auto-pairs

`(`, `"`, `'` and `/` add their closing character.

```
type (        → (|)
type foo      → (foo|)
type )        → (foo)|          types over the existing )
Backspace in (|) or "|"         deletes both characters
```

`(` and `'` pair only at the start of a term, so `foo(` and `don't` stay as typed.
Turn auto-pairs off with `auto_pairs = false`.

#### Syntax help in the prompt

- Operators, quotes, regexes and parentheses have colors.
- An unclosed `(` or `"` is red, and the status bar shows `missing )` or `unclosed "` first, in yellow.
- The parenthesis at the cursor and its partner show in reverse video.

### Normal mode

| Key | Does |
|---|---|
| `i` `a` `I` `A` | insert mode (at / after the cursor, at the start, at the end) |
| `h` `l` `0` `^` `$` `w` `b` `e` | move the cursor |
| `x` `X` `D` `C` `s` `S` | edit, as in vim |
| `dw` `cw` `de` `db` `d$` `dd` `cc` … | operator + motion |
| `ci"` `di(` `da'` `ciw` `daw` `ci/` | text objects: in / around quotes, `()`, `//`, words |
| `%` | jump to the matching parenthesis |
| `gUiw` `guw` `gUU` | uppercase / lowercase (`gUiw` on `or` gives `OR`) |
| `j` `k` | move the selection |
| `gg` `G` | the hit at the top / bottom end of the whole list, in screen order |
| `R` | turn on / off the new search after a file changes |
| `Ctrl-d` `Ctrl-u` / `Ctrl-f` `Ctrl-b` | half page / page |
| `Ctrl-e` / `Ctrl-y` | scroll the preview down / up |
| `zh` / `zl` | scroll the preview left / right |
| `:` | command line |
| `?` | help |
| `q` | quit |

`Esc` in normal mode clears the query, one step. It never quits: use `q`, `:q` or `Ctrl-c`.
`/` starts typing again, at the end of the query.

## Completion

All menus work the same way:

- The first item sits next to the prompt. The `:` command line is on the status bar row, so its menu is always above it.
- `Up` / `Down` and `Ctrl-k` / `Ctrl-j` move as they point on the screen, as in the results list. `Ctrl-n` / `Ctrl-p` move to the next / previous item.
- `Tab` or `Enter` accepts. A complete term gets a space after it. A folder or a field name opens the next menu.
- A space ends the word and closes the menu. The `Ctrl-r` history search is the exception: it filters by the whole query, spaces included.
- `Esc` closes the menu. A second `Esc` enters normal mode.
- The help popup (`?` at an empty prompt or in normal mode, or `F1`) scrolls with vim keys: `j` `k`, `Ctrl-d` `Ctrl-u`, `g` `G` (a single `g` goes to the top). `q`, `Esc`, `?` or `F1` closes it.

### `@` files and folders

`@` at the start of a term opens a fuzzy file and folder picker, as in Claude Code.
Accepting replaces `@…` with a `path:` field.

```
fn @uilist     → fn path:src/ui/list.rs
TODO -@vendor  → TODO -path:vendor/
@              → the top-level folders and files
```

If you close the menu, `@` stays text, so `@Override` still searches for `@Override`.
`foo@bar` does not open the menu.

### Field values

Typing a field name and `:` opens its values:

```
git:     → changed  modified  staged  untracked
sort:    → relevance  path  modified
ext:     → the extensions in this folder, most common first
type:    → ripgrep type names
path:    → the same fuzzy picker as @
```

The `@` and `path:` pickers and the `Ctrl-r` history search use fzf-style fuzzy
matching (the `nucleo-matcher` crate from the Helix editor). It finds the best
placement of the typed letters. The menu highlights the matched letters.

### Other words

`Ctrl-Space` completes the word under the cursor from:

- field names: `pa` → `path:`
- operators: `o` → `OR`
- words in the current results: `Searc` → `Search`, `SearchStatus`
- history, when the prompt is empty

## Commands

`:` opens the command line: in normal mode, or at an empty prompt in insert mode.
It takes the place of the status bar, on the last row. A menu above it lists the
commands. `Tab` completes names and arguments. `Enter` runs a complete command.
In the menu, `Enter` on an argument accepts it and runs the command, and `Enter`
on a command name that takes an argument completes the name. Short forms work:
`:so m` is `:sort modified`, `:q` quits. `:theme`, `:help` and `:q` work the same
in every betterslop tool.

The command line has the readline keys: `Left` `Right` `Home` `End` `Ctrl-a` `Ctrl-e`
`Ctrl-b` `Ctrl-f`, `Backspace` `Ctrl-h` `Delete` `Ctrl-w` `Ctrl-u`. `Ctrl-Space` reopens the completion menu. `Esc` closes the
line and keeps its text: the next `:` opens with it. `Backspace` or `Enter` in an
empty line closes it. `F1` opens the help.

| Command | Does |
|---|---|
| `:sort relevance` / `path` / `modified` | sort order |
| `:context N` | context lines (`0` turns them off) |
| `:mode query` / `regex` / `file` | query mode |
| `:case smart` / `ignore` | case mode |
| `:changed` | only files changed in git (on / off) |
| `:theme dark` / `light` | color theme |
| `:layout auto` / `horizontal` / `vertical` | preview position |
| `:split N` | percent of the screen for the results (10–90) |
| `:prompt bottom` / `top` | prompt position |
| `:set wrap` / `nowrap` | wrap long preview lines |
| `:set group` / `nogroup` | group hits under file headers |
| `:set preview` / `nopreview` | show / hide the preview |
| `:set scrollbar` / `noscrollbar` | scrollbars |
| `:set watch` / `nowatch` | search again when a file changes (also `R` in normal mode) |
| `:set header` / `noheader` | top bar |
| `:set ivy` / `noivy` | ivy style (regex mode) |
| `:marks all` / `:marks clear` | mark all hits / clear the marks |
| `:open` | open the hit or the marked hits |
| `:open NAME` | run the `[open.NAME]` command (see [Open commands](#open-commands)) |
| `:help` | help |
| `:q` | quit |

Errors show first in the status bar, in red, for example `unknown command :bogus`. A notice is cut at 60 characters.

## Results and preview

### Results

- Each row shows the path, the line number and the line: `src/ui/list.rs:42: fn draw(…`.
- `:set group` (or `group = true`) groups hits under file headers with a count: `src/ui/list.rs (3)`.
- Matches are black on yellow (the shared `matched` style), also on the selected row. Line numbers use the `info` role (cyan in the dark theme). The pane title shows the hit count: ` Hits 12 `.
- A `path:` match is highlighted in the path that the row draws (`fn path:main` marks `main` in `src/main.rs`).
- The status bar is the last row of the screen, in the shared `bar` style, with the prompt above it (or at the top). It shows the notice first, then `164 hits · 25/2074 files · 37 ms` (hits, files with hits / files searched, time), then the flags, then `F1 help` (`? help` in normal mode).
- While a search runs, it shows a spinner and `searched 812/2074 files`.

| Key | Does |
|---|---|
| `Up` `Down` `Ctrl-k` `Ctrl-j` | move the selection |
| `PgUp` `PgDn` | move one page |
| mouse wheel | scroll the list |
| click | select a row |

### Preview

The preview shows the file around the hit with syntax colors. It marks the hit
line and shows all matches in yellow. The pane title is the path and line, ` src/main.rs:98 `.
The right side of the top border shows the match count, the language and the
file size: `6 matches · Rust · 22.6 KB`.

| Key | Does |
|---|---|
| `Shift-Up` / `Shift-Down` | scroll up / down (as in fzf) |
| `Ctrl-e` / `Ctrl-y` (normal mode) | scroll down / up (as in vim) |
| `zh` / `zl` (normal mode) | scroll left / right, when not wrapped |
| `Ctrl-/` | show / hide (as in fzf) |
| `:set wrap` / `:set nowrap` | wrap long lines |
| mouse wheel | scroll |

Syntax colors cover the `syntect` default languages (Rust, Go, Python,
JavaScript, YAML, JSON, Markdown, shell, Lua and others) plus TypeScript, TSX,
TOML, Dockerfile, Swift, Kotlin, Nix, Terraform and Zig.

## Open hits

| Key | Opens |
|---|---|
| `Enter` | the selected hit: `$EDITOR +LINE FILE` |
| `Tab` / `Shift-Tab` | mark the hit and go to the next / previous hit |
| `Enter` with marks | the marked hits as a quickfix list: `$EDITOR -q FILE` |
| `Ctrl-q` | all hits as a quickfix list (as in telescope) |

The editor is the config `editor`, else `$EDITOR`, else `nvim`. `qf` runs the editor
in the terminal. When you quit the editor, you are back in `qf` with the same query
and hits. With `editor_return = false`, `qf` replaces itself with the editor, and
quitting the editor returns you to the shell.

By default, `qf` runs `EDITOR +LINE FILE`, and opens several marks as a quickfix
list with `EDITOR -q LIST` (vim and nvim). The list has the match column, so the
editor puts the cursor on the match.

For other editors, use a template with `{file}`, `{line}` and `{col}` (the column of
the first match):

```toml
editor = "code -g {file}:{line}:{col}"     # VS Code and its forks (cursor, windsurf …)
editor = "devin-desktop -g {file}:{line}:{col}"   # Devin Desktop (not `devin`, the CLI agent)
editor = "hx {file}:{line}:{col}"          # Helix
editor = "subl {file}:{line}:{col}"        # Sublime Text
```

With several marks, `qf` repeats the arguments after the program name for each
hit: `code -g a.rs:3:5 -g b.rs:9:1`.

`Enter` also saves the query to the history (`~/.local/state/qf/history`).

### Open commands

`Enter` always opens your editor. For other tools (a second editor, the clipboard,
a browser, a script), add `[open.NAME]` tables to the config. Each command runs on
the selected hit, or on the marked hits.

```toml
[open.code]
cmd = "code -g {file}:{line}:{col}"
key = "ctrl-g"

[open.copy]
cmd = "pbcopy"              # any clipboard tool: wl-copy, xclip -selection clipboard …
stdin = "{file}:{line}"
key = "ctrl-t"

[open.hx]
cmd = "hx {file}:{line}:{col}"
stay = false
```

| Key | Does |
|---|---|
| `cmd` | the command. Same rules as `editor`. Templates also have `{text}` (the hit line) |
| `stdin` | optional: a template for the command's input, one line per hit. With `stdin`, `cmd` gets no file arguments |
| `key` | optional: `ctrl-g`, `ctrl-s`, `ctrl-t`, `ctrl-v`, `ctrl-x` or `ctrl-z`. Other Ctrl keys belong to `qf` |
| `label` | optional: the text shown in the `Ctrl-o` menu and the status bar. Default: the NAME |
| `stay` | `true` (default): run in the background, `qf` stays open. `false`: `qf` exits and runs the command in the terminal (for terminal editors) |

Run a command in one of three ways:

- `Ctrl-o`: a popup menu of the commands. `j` `k`, `Ctrl-j` `Ctrl-k` or the arrows move, `Enter` or
  the letter shown runs a command, `Esc` or `q` closes the menu.
- `:open NAME`. `Tab` completes the name.
- The `key` of the command.

With `stay = true`, `qf` discards the command output and the status bar shows
`ran NAME`. `qf` does not use a shell. It splits `cmd` at spaces. For pipes, use
a script. `qf` checks the names, commands and keys when it starts. A bad or
duplicate key stops it with an error.

## Layout and themes

The top bar shows the current folder, the git branch and the file count on the
left, and the search scope from the command-line flags on the right:

```
 qf  ~/repos/myapp · main · 2,155 files                     src · hidden · -g !*.lock
```

`:set noheader` or `header = false` hides it.

| Setting | Values |
|---|---|
| prompt | `bottom` (default, hits go up) or `top` |
| layout | `auto` (default), `horizontal` (preview at the side), `vertical` (preview above or below) |
| split | percent of the screen for the results, default 50 |
| narrow_width | `auto` picks vertical below this width, default 100 columns |
| theme | `auto` (default), `dark` (Monokai Extended Bright) or `light` (Monokai Extended Light) |

Change them with `:prompt`, `:layout`, `:split` and `:theme`, or in the config file.

`TUI_THEME=dark` or `TUI_THEME=light` sets the theme and wins over the config.
`theme = "auto"` picks dark or light at startup. `qf` asks the terminal for its
background color (OSC 11). If the terminal does not answer, it uses `$COLORFGBG`,
then the macOS appearance setting, then dark.

`NO_COLOR=1 qf` turns all colors off. The selection then uses reverse video and a
`▌` marker.

## Config file

`~/.config/qf/config.toml` (or `$XDG_CONFIG_HOME/qf/config.toml`). All keys are
optional. Command-line flags override the file. An unknown key is an error.

```toml
# Search
mode = "query"          # "query" or "regex"
ivy = false             # regex mode: spaces become .*
smart_case = true       # false: all terms case-insensitive
file_mode = false       # terms apply to the whole file
context = 0             # context lines around each hit
sort = "relevance"      # "relevance", "path" or "modified"

# Prompt
auto_pairs = true       # ( " ' / add the closing character
auto_uppercase = true   # "and", "or", "not" between terms become AND, OR, NOT

# Layout
prompt = "bottom"       # "bottom" or "top"
layout = "auto"         # "auto", "horizontal" or "vertical"
split = 50              # percent for the results (10-90)
narrow_width = 100      # auto layout: vertical below this width
group = false           # group hits under file headers
preview = true          # show the preview
preview_wrap = false    # wrap long preview lines
scrollbar = true        # scrollbars
watch = true            # search again when a file changes
header = true           # top bar: folder, git branch, file count, search scope

# Look
theme = "auto"          # "auto" (ask the terminal), "dark" or "light"
theme_background = false # paint the preview with the theme background
marker = true           # selection marker (always on with NO_COLOR)

# Other
mouse = true            # false keeps the terminal's text selection
editor = "nvim"         # default: $EDITOR, then nvim; or a template (see Open hits)
editor_return = true    # back to qf when the editor exits; false: qf exits

# Open commands (see Open commands), any number
[open.copy]
cmd = "pbcopy"
stdin = "{file}:{line}"
key = "ctrl-t"
```

## Reference

### Command-line flags

| Flag | Does |
|---|---|
| `QUERY` | initial query. Quote a query with spaces (`qf 'foo bar'`): a second word is a path |
| `PATHS…` | files or folders to search |
| `-r`, `--regex` | regex mode |
| `-i`, `--ivy` | regex mode, spaces become `.*` |
| `-I`, `--ignore-case` | all terms case-insensitive |
| `--hidden` | search hidden files |
| `--no-ignore` | do not respect ignore files |
| `-g`, `--glob GLOB` | include (or `!` exclude) files, repeatable |
| `-t`, `--type TYPE` | only this file type, repeatable |
| `-T`, `--type-not TYPE` | not this file type, repeatable |
| `--changed` | only files changed in git |
| `--file-mode` | terms apply to the whole file |
| `-C`, `--context NUM` | context lines |
| `--sort relevance\|path\|modified` | sort order |

### Keys in both modes

No key needs Alt (Option), so all keys work in a default macOS terminal. Settings
use `:` commands.

| Key | Does |
|---|---|
| `Enter` | open the hit (or the marked hits) |
| `Tab` / `Shift-Tab` | mark and move |
| `Up` `Down` `Ctrl-k` `Ctrl-j` `PgUp` `PgDn` | move the selection |
| `Shift-Up` `Shift-Down` | scroll the preview |
| `Ctrl-/` | show / hide the preview |
| `Ctrl-q` | all hits to the quickfix list |
| `Ctrl-o` | menu of the `[open.NAME]` commands |
| `Ctrl-l` | clear the query, insert mode |
| `F1` | help |
| `Ctrl-c` | quit |

## Limits

- A search stops at 10,000 hits (the status bar shows `10000+ hits`). Before the file list is ready (just after start), the walk order decides which hits `qf` keeps.
- Operators work on one line, except in file mode.
- The preview skips files larger than 4 MB.
- A narrowed search (adding an AND term) does not see files created after the last full search.
- `:theme` and the config change colors. There is no theme file support yet.
