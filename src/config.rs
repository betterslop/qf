//! Config file: `$XDG_CONFIG_HOME/qf/config.toml` (default `~/.config/qf/config.toml`).
//! All keys are optional. Command-line flags override the file.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::query::{Mode, SortBy};
use crate::theme::ThemeChoice;

pub const EXAMPLE: &str = "\
Config file (~/.config/qf/config.toml), all keys optional:
  mode = \"query\"        # \"query\" or \"regex\"
  ivy = false           # regex mode: spaces become .*
  smart_case = true     # false: all terms case-insensitive
  group = false         # group hits under file headers
  preview = true        # show the preview
  preview_wrap = false  # wrap long preview lines (false: scroll with zh / zl)
  mouse = true          # mouse support (false keeps terminal text selection)
  editor = \"nvim\"       # Enter. Default: $EDITOR, then nvim. Runs: editor +LINE FILE
                        # template: \"code -g {file}:{line}:{col}\"
  editor_return = true  # back to qf when the editor exits. false: qf exits
  prompt = \"bottom\"     # \"bottom\" (hits go up from the prompt) or \"top\"
  layout = \"auto\"       # \"auto\", \"horizontal\" (preview at the side) or \"vertical\"
  split = 50            # percent of the screen for the results (10-90)
  narrow_width = 100    # auto layout: vertical below this terminal width
  scrollbar = true      # scrollbars on the list and the preview
  watch = true          # search again when a file changes (:set watch, R)
  header = true         # top bar: folder, git branch, file count, search scope
  theme_background = false # paint the preview with the theme background
  marker = true         # marker left of the selected row; always on with NO_COLOR
  theme = \"auto\"        # \"auto\" (ask the terminal), \"dark\" or \"light\"
  file_mode = false     # terms apply to the whole file, not to one line
  context = 0           # context lines around each hit (:context N changes it)
  sort = \"relevance\"    # \"relevance\", \"path\" or \"modified\" (newest file first)
  auto_pairs = true     # typing ( \" ' or / adds the closing character
  auto_uppercase = true   # \"and\", \"or\", \"not\" between terms become AND, OR, NOT

Open commands: run with C-o (menu), :open NAME or the key. Any number of tables:
  [open.code]
  cmd = \"code -g {file}:{line}:{col}\"   # same rules as editor; also {text}
  key = \"ctrl-g\"         # optional: ctrl-g, ctrl-s, ctrl-t, ctrl-v, ctrl-x or ctrl-z
  stay = true            # default: keep qf open. false: qf exits and runs the command
  [open.copy]
  cmd = \"pbcopy\"         # with stdin, cmd gets no file arguments
  stdin = \"{file}:{line}\"   # optional: one line per hit to the command's input";

/// Ctrl keys that qf uses. `i` and `m` are the same codes as Tab and Enter, `o`
/// opens the open menu. Keep this in sync with the key handling in `app.rs`.
const CTRL_USED: &str = "abcdefhijklmnopqruwy";

/// One `[open.NAME]` table: a command that runs on the selected or marked hits.
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct OpenCmd {
    pub cmd: String,
    /// Template for the command's input, one line per hit.
    #[serde(default)]
    pub stdin: Option<String>,
    /// A Ctrl key, as `ctrl-t`.
    #[serde(default)]
    pub key: Option<String>,
    /// Text shown in the menu and the status line. Default: the NAME.
    #[serde(default)]
    pub label: Option<String>,
    /// Keep qf open. False: qf exits and runs the command in the terminal.
    #[serde(default = "yes")]
    pub stay: bool,
}

fn yes() -> bool {
    true
}

/// The letter of a `ctrl-X` key.
pub fn ctrl_key(key: &str) -> Option<char> {
    let mut rest = key.strip_prefix("ctrl-")?.chars();
    match (rest.next(), rest.next()) {
        (Some(c), None) if c.is_ascii_lowercase() => Some(c),
        _ => None,
    }
}

/// Check the `[open.NAME]` tables: names, commands and keys.
fn check_open(open: &BTreeMap<String, OpenCmd>) -> Result<()> {
    let free: Vec<String> = ('a'..='z')
        .filter(|c| !CTRL_USED.contains(*c))
        .map(|c| format!("ctrl-{c}"))
        .collect();
    let mut used: BTreeMap<char, &str> = BTreeMap::new();
    for (name, o) in open {
        if name.is_empty() || name.contains(char::is_whitespace) {
            bail!("open.{name:?}: the name must be one word");
        }
        if o.cmd.trim().is_empty() {
            bail!("open.{name}: cmd is empty");
        }
        let Some(key) = &o.key else { continue };
        let Some(c) = ctrl_key(key) else {
            bail!(
                "open.{name}: key {key:?} must be ctrl and a letter: {}",
                free.join(", ")
            );
        };
        if CTRL_USED.contains(c) {
            bail!("open.{name}: qf already uses {key}; free keys: {}", free.join(", "));
        }
        if let Some(other) = used.insert(c, name) {
            bail!("open.{other} and open.{name} both use {key}");
        }
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptSide {
    Top,
    Bottom,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayoutMode {
    Auto,
    Horizontal,
    Vertical,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub mode: Mode,
    pub ivy: bool,
    pub smart_case: bool,
    pub group: bool,
    pub preview: bool,
    pub preview_wrap: bool,
    pub mouse: bool,
    pub editor: Option<String>,
    /// Come back to qf when the editor exits. False: qf exits and the editor replaces it.
    pub editor_return: bool,
    pub prompt: PromptSide,
    pub layout: LayoutMode,
    pub split: u16,
    pub narrow_width: u16,
    pub scrollbar: bool,
    pub watch: bool,
    pub header: bool,
    pub theme_background: bool,
    pub marker: bool,
    pub theme: ThemeChoice,
    pub file_mode: bool,
    pub context: usize,
    pub sort: SortBy,
    pub auto_pairs: bool,
    pub auto_uppercase: bool,
    pub open: BTreeMap<String, OpenCmd>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Query,
            ivy: false,
            smart_case: true,
            group: false,
            preview: true,
            preview_wrap: false,
            mouse: true,
            editor: None,
            editor_return: true,
            prompt: PromptSide::Bottom,
            layout: LayoutMode::Auto,
            split: 50,
            narrow_width: 100,
            scrollbar: true,
            watch: true,
            header: true,
            theme_background: false,
            marker: true,
            theme: ThemeChoice::Auto,
            file_mode: false,
            context: 0,
            sort: SortBy::Relevance,
            auto_pairs: true,
            auto_uppercase: true,
            open: BTreeMap::new(),
        }
    }
}

fn config_path() -> Option<PathBuf> {
    Some(bs_tui_common::paths::config_dir("qf")?.join("config.toml"))
}

impl Config {
    /// The editor for Enter: config `editor`, then $EDITOR, then `nvim`.
    pub fn editor(&self) -> String {
        self.editor
            .clone()
            .or_else(|| env::var("EDITOR").ok())
            .filter(|e| !e.trim().is_empty())
            .unwrap_or_else(|| "nvim".into())
    }

    /// Load the config file. A missing file gives the defaults.
    pub fn load() -> Result<Self> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };
        match fs::read_to_string(&path) {
            Ok(text) => {
                let mut config: Self =
                    toml::from_str(&text).with_context(|| format!("invalid config {}", path.display()))?;
                config.split = config.split.clamp(10, 90);
                check_open(&config.open).with_context(|| format!("invalid config {}", path.display()))?;
                Ok(config)
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("failed to read {}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        let c: Config = toml::from_str("mode = \"regex\"\ngroup = true\neditor = \"vim\"").unwrap();
        assert!(c.mode == Mode::Regex && c.group && c.smart_case && c.editor.as_deref() == Some("vim"));
        assert!(!Config::default().group);
        assert!(toml::from_str::<Config>("colour = 1").is_err());
    }

    fn open(text: &str) -> Result<()> {
        check_open(&toml::from_str::<Config>(text)?.open)
    }

    #[test]
    fn open_commands() {
        let c: Config = toml::from_str("[open.copy]\ncmd = \"pbcopy\"\nstdin = \"{file}\"\nkey = \"ctrl-t\"").unwrap();
        let copy = &c.open["copy"];
        assert!(copy.stay && copy.stdin.as_deref() == Some("{file}") && copy.key.as_deref() == Some("ctrl-t"));
        assert!(copy.label.is_none());
        assert!(
            open("[open.a]\ncmd = \"x\"\nkey = \"ctrl-g\"\nlabel = \"Go\"\n[open.b]\ncmd = \"y\"\nstay = false")
                .is_ok()
        );
        assert!(open("[open.a]\ncmd = \"x\"\nwhat = 1").is_err());
        assert!(
            open("[open.a]\ncmd = \"x\"\nkey = \"ctrl-y\"")
                .unwrap_err()
                .to_string()
                .contains("already uses")
        );
        assert!(open("[open.a]\ncmd = \"x\"\nkey = \"ctrl-o\"").is_err());
        assert!(open("[open.a]\ncmd = \"x\"\nkey = \"alt-x\"").is_err());
        assert!(
            open("[open.a]\ncmd = \"x\"\nkey = \"ctrl-x\"\n[open.b]\ncmd = \"y\"\nkey = \"ctrl-x\"")
                .unwrap_err()
                .to_string()
                .contains("both use")
        );
        assert!(open("[open.a]\ncmd = \" \"").is_err());
        assert!(open("[open.\"a b\"]\ncmd = \"x\"").is_err());
    }
}
