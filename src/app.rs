//! App state and key handling.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bs_tui_common::cmdline::{self, CommandLine, Shared};
use bs_tui_common::complete::{Candidate, Completion};
use bs_tui_common::notice::Notice;
use bs_tui_common::theme::Background;
use crossbeam_channel::Sender;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::command::{self, Command, QueryMode};
use crate::complete::{self, Sources};
use crate::config::{self, Config, LayoutMode, OpenCmd, PromptSide};
use crate::editor::LineEditor;
use crate::history::History;
use crate::open::{self, Launch};
use crate::preview::{self, Request, Response};
use crate::query::{self, Mode, Options, Query, SortBy};
use crate::search::{self, Files, Hit, MAX_HITS, Search};
use crate::syntax;
use crate::theme::{self, Palette, ThemeName};
use crate::ui::header::HeaderInfo;

/// Columns per horizontal preview scroll step.
const HSCROLL_STEP: u16 = 8;
/// Rows per mouse wheel step.
const WHEEL_STEP: isize = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EditMode {
    Insert,
    Normal,
}

/// A hit to open in the editor.
#[derive(Clone)]
pub struct Target {
    pub path: Arc<str>,
    pub line: u64,
    /// 1-based column of the first match.
    pub col: u32,
    pub text: String,
}

impl Target {
    fn of(hit: &Hit) -> Self {
        Self {
            path: Arc::clone(&hit.path),
            line: hit.line,
            col: hit.col,
            text: hit.text.clone(),
        }
    }

    fn is(&self, hit: &Hit) -> bool {
        self.line == hit.line && self.path == hit.path
    }
}

pub enum Action {
    None,
    Quit,
    /// Close the TUI and run this command.
    Open(Launch),
    /// Close the TUI, run this command, then show the TUI again.
    Run(Launch),
}

/// Numbers for the status line, from the last finished search.
pub struct Stats {
    pub files_searched: usize,
    pub files_matched: usize,
    pub elapsed: Duration,
}

/// A finished search that did not stop at the hit limit. A query that only adds
/// AND terms to it can search only the files in `files`.
struct Complete {
    text: String,
    opts: Options,
    files: Files,
}

/// Data derived from one file list, for completion.
struct ListCache {
    /// Address of the file list it was built from.
    key: usize,
    exts: Arc<complete::ExtCounts>,
    dirs: Arc<Vec<Arc<str>>>,
}

/// UI inputs for `App::new`.
pub struct Look {
    pub palette: Palette,
    /// False with `NO_COLOR`.
    pub color: bool,
    pub header: HeaderInfo,
}

/// The `C-o` menu: the selected row and `(key, [open.NAME] name, label)` rows.
pub struct OpenMenu {
    pub selected: usize,
    pub items: Vec<(char, String, String)>,
}

impl OpenMenu {
    /// The rows as shared menu items.
    pub fn menu_items(&self) -> Vec<bs_tui_common::menu::Item<'_>> {
        self.items
            .iter()
            .map(|(key, _, label)| bs_tui_common::menu::Item { key: *key, label })
            .collect()
    }
}

/// The key for a menu row: the first letter or digit of the label, then of the
/// name, that no row has and that the menu does not use for moving (`j` `k` `q`).
/// With none left, the first free letter or digit.
fn menu_key(label: &str, name: &str, taken: &str) -> char {
    let free = |c: &char| !taken.contains(*c) && !matches!(c, 'j' | 'k' | 'q');
    label
        .chars()
        .chain(name.chars())
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .find(free)
        .or_else(|| ('a'..='z').chain('0'..='9').find(free))
        .unwrap_or(' ')
}

pub struct App {
    pub input: LineEditor,
    pub mode: EditMode,
    pub opts: Options,
    pub hits: Vec<Hit>,
    pub selected: usize,
    pub scroll: usize,
    /// Height of the results list, set by the UI.
    pub page: usize,
    pub searching: bool,
    pub error: Option<String>,
    /// Marked hits, in the order the user marked them.
    pub marks: Vec<Target>,
    /// The query of the last started search. The UI uses it to highlight the preview.
    pub active: Option<Query>,
    pub show_preview: bool,
    /// Group hits under file headers.
    pub group: bool,
    /// Wrap long preview lines. When false, `hscroll` scrolls them.
    pub wrap: bool,
    pub hscroll: u16,
    /// First visible column of the prompt text.
    pub prompt_offset: u16,
    /// Lines to scroll the preview away from the hit.
    pub preview_scroll: isize,
    /// Screen areas and the hit on each visible list row, set by the UI for mouse events.
    pub list_area: Rect,
    pub preview_area: Option<Rect>,
    pub list_rows: Vec<Option<usize>>,
    pub stats: Option<Stats>,
    /// Start of the current search. The spinner also uses it.
    pub started: Instant,
    /// Prompt at the bottom: the list goes up from the prompt.
    pub bottom: bool,
    pub layout: LayoutMode,
    /// Percent of the screen for the results.
    pub split: u16,
    pub narrow_width: u16,
    pub scrollbar: bool,
    /// Search again when a file changes.
    pub watch: bool,
    /// A refresh search is running. Its hits wait in `refresh_hits`, so the list does
    /// not jump. `refresh_target` is the hit to select again.
    refreshing: bool,
    refresh_target: Option<Target>,
    refresh_hits: Vec<Hit>,
    pub palette: Palette,
    /// The help popup is open. The value is its scroll position.
    pub help: Option<u16>,
    /// The `C-o` menu of `[open.NAME]` commands, while it is open.
    pub open_menu: Option<OpenMenu>,
    /// The top bar and what it shows.
    pub show_header: bool,
    pub header: HeaderInfo,
    /// Sort order when the query has no `sort:` field.
    pub sort_by: SortBy,
    /// Context lines that `:context` last turned on.
    context_size: usize,
    /// The completion menu of the query prompt, while it is open.
    pub completion: Option<Completion>,
    /// The open menu is the `C-r` history search: it filters by the whole query.
    history_menu: bool,
    /// Words of the shown hits (by search id), and extensions and folders of the file
    /// list (by list address), for completion.
    words_cache: Option<(u64, Arc<Vec<String>>)>,
    list_cache: Option<ListCache>,
    auto_pairs: bool,
    auto_uppercase: bool,
    /// `i` or `a` after an operator: a text object follows (`ci"`, `da(`).
    pending_obj: Option<char>,
    /// The `:` command line and its completion menu.
    pub cmdline: CommandLine,
    /// The query as a shared filter, to highlight `path:` matches in drawn paths.
    path_filter: Option<bs_tui_common::query::Filter>,
    /// Result of the last command, shown first in the status line.
    pub message: Option<Notice>,
    /// Palette inputs, for `:theme`.
    color: bool,
    theme_background: bool,
    marker: bool,
    last_complete: Option<Complete>,
    pub preview: Option<Response>,
    /// Time of the last query change. A search starts after the debounce.
    pub search_due: Option<Instant>,
    preview_sent: Option<Request>,
    preview_tx: Sender<preview::Msg>,
    history: History,
    /// The editor for Enter, and the `[open.NAME]` commands.
    editor: String,
    /// Come back to qf when the editor exits.
    editor_return: bool,
    open_cmds: BTreeMap<String, OpenCmd>,
    /// vim operator (`d` or `c`) that waits for a motion.
    pending_op: Option<char>,
    /// First `g` of `gg`.
    pending_g: bool,
    /// First `z` of `zh` / `zl`.
    pending_z: bool,
    current: u64,
    shown: u64,
}

impl App {
    pub fn new(
        query: &str,
        opts: Options,
        sort_by: SortBy,
        config: &Config,
        look: Look,
        preview_tx: Sender<preview::Msg>,
    ) -> Self {
        let Look { palette, color, header } = look;
        // `:open NAME` completes the `[open.NAME]` commands.
        let mut cmdline = CommandLine::new(command::specs().to_vec());
        let open = config
            .open
            .iter()
            .map(|(name, o)| {
                let mut c = Candidate::new(name.clone(), "");
                if let Some(label) = &o.label {
                    c.label = label.clone();
                }
                c
            })
            .collect();
        cmdline.set_args("open", open);
        Self {
            input: LineEditor::new(query),
            mode: EditMode::Insert,
            opts,
            hits: Vec::new(),
            selected: 0,
            scroll: 0,
            page: 1,
            searching: false,
            error: None,
            marks: Vec::new(),
            active: None,
            show_preview: config.preview,
            group: config.group,
            wrap: config.preview_wrap,
            hscroll: 0,
            prompt_offset: 0,
            preview_scroll: 0,
            list_area: Rect::default(),
            preview_area: None,
            list_rows: Vec::new(),
            stats: None,
            started: Instant::now(),
            bottom: config.prompt == PromptSide::Bottom,
            layout: config.layout,
            split: config.split,
            narrow_width: config.narrow_width,
            scrollbar: config.scrollbar,
            watch: config.watch,
            refreshing: false,
            refresh_target: None,
            refresh_hits: Vec::new(),
            palette,
            help: None,
            open_menu: None,
            show_header: config.header,
            header,
            sort_by,
            context_size: if opts.context > 0 {
                opts.context
            } else {
                config.context.max(2)
            },
            completion: None,
            history_menu: false,
            words_cache: None,
            list_cache: None,
            auto_pairs: config.auto_pairs,
            auto_uppercase: config.auto_uppercase,
            pending_obj: None,
            cmdline,
            path_filter: None,
            message: None,
            color,
            theme_background: config.theme_background,
            marker: config.marker,
            last_complete: None,
            preview: None,
            search_due: None,
            preview_sent: None,
            preview_tx,
            history: History::load(),
            editor: config.editor(),
            editor_return: config.editor_return,
            open_cmds: config.open.clone(),
            pending_op: None,
            pending_g: false,
            pending_z: false,
            current: 0,
            shown: 0,
        }
    }

    pub fn restart(&mut self, search: &Search) {
        self.refreshing = false;
        self.search_due = None;
        let text = self.input.text();
        match query::compile(&text, self.opts) {
            Ok(query) => {
                self.error = None;
                self.path_filter = (self.opts.mode == Mode::Query)
                    .then(|| path_filter(&text, self.opts.smart_case))
                    .flatten();
                self.active = query.clone();
                let only = self.narrow_files(&text);
                self.started = Instant::now();
                self.current = search.start(query, self.opts.context, only);
                self.searching = true;
            }
            Err(msg) => {
                // Keep the old hits on screen while the query is incomplete.
                self.error = Some(msg);
                self.searching = false;
            }
        }
    }

    /// A file changed. Search again, and keep the list as it is until the new hits are
    /// complete. Do nothing while the user types or while the search has an error.
    pub fn on_changed(&mut self, search: &Search) {
        if !self.watch || self.search_due.is_some() || self.error.is_some() {
            return;
        }
        if self.input.text().trim().is_empty() && !self.opts.changed {
            return;
        }
        let target = self.hits.get(self.selected).map(Target::of);
        self.restart(search);
        if self.searching {
            self.searching = false;
            self.refreshing = true;
            self.refresh_target = target;
            self.refresh_hits.clear();
        }
    }

    pub fn on_search(&mut self, ev: search::Event) {
        if self.refreshing {
            match ev {
                search::Event::Hits(id, hits) if id == self.current => self.refresh_hits.extend(hits),
                search::Event::Done(id, files_searched) if id == self.current => {
                    self.refreshing = false;
                    self.hits = std::mem::take(&mut self.refresh_hits);
                    self.shown = id;
                    if self.hits.len() > MAX_HITS {
                        self.hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
                        self.hits.truncate(MAX_HITS);
                    }
                    self.sort();
                    // Select the same hit, or the row nearest to the old one.
                    let old = self.selected;
                    self.selected = self
                        .refresh_target
                        .take()
                        .and_then(|t| self.hits.iter().position(|h| t.is(h)))
                        .unwrap_or_else(|| old.min(self.hits.len().saturating_sub(1)));
                    // The file content changed: the preview must read it again.
                    self.preview_sent = None;
                    self.finished(files_searched);
                }
                _ => {}
            }
            return;
        }
        match ev {
            search::Event::Hits(id, hits) if id == self.current => {
                self.show(id);
                self.hits.extend(hits);
            }
            search::Event::Done(id, files_searched) if id == self.current => {
                self.show(id);
                self.searching = false;
                // A step can pass the limit. Keep the first hits by path, so every run
                // of the same search keeps the same hits.
                if self.hits.len() > MAX_HITS {
                    self.hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
                    self.hits.truncate(MAX_HITS);
                }
                self.sort();
                self.finished(files_searched);
            }
            _ => {}
        }
    }

    /// Replace old hits with the hits of search `id` when its first result arrives.
    /// This prevents a blank list between keystrokes.
    fn show(&mut self, id: u64) {
        if self.shown != id {
            self.shown = id;
            self.hits.clear();
            self.select_to(0);
            self.scroll = 0;
        }
    }

    /// The sort order: `sort:` in the query, else the app setting.
    pub fn sort_order(&self) -> SortBy {
        self.active.as_ref().and_then(|q| q.sort).unwrap_or(self.sort_by)
    }

    /// Sort by path and line, or by file time (newest first). If the user moved the
    /// selection, it stays on the same hit. Otherwise it stays on the top row.
    fn sort(&mut self) {
        let selected = self
            .hits
            .get(self.selected)
            .filter(|_| self.selected > 0)
            .map(Target::of);
        match self.sort_order() {
            SortBy::Path => self.hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line))),
            SortBy::Modified => self.hits.sort_by(|a, b| {
                b.modified
                    .cmp(&a.modified)
                    .then_with(|| a.path.cmp(&b.path))
                    .then(a.line.cmp(&b.line))
            }),
            SortBy::Relevance => {
                // Files by their best hit, then by hit count (up to 20), then newest.
                // Lines in a file stay in line order.
                let mut files: HashMap<Arc<str>, (u32, usize)> = HashMap::new();
                for h in &self.hits {
                    let f = files.entry(Arc::clone(&h.path)).or_default();
                    f.0 = f.0.max(h.score);
                    f.1 += 1;
                }
                self.hits.sort_by_cached_key(|h| {
                    let (best, count) = files[&h.path];
                    (
                        Reverse(best),
                        Reverse(count.min(20)),
                        Reverse(h.modified),
                        Arc::clone(&h.path),
                        h.line,
                    )
                });
            }
        }
        if let Some(t) = selected {
            self.selected = self.hits.iter().position(|h| t.is(h)).unwrap_or(0);
        }
    }

    /// Record stats, and remember the search for narrowing if it is complete.
    fn finished(&mut self, files_searched: usize) {
        let mut files: Vec<Arc<str>> = Vec::new();
        for hit in &self.hits {
            if files.last() != Some(&hit.path) {
                files.push(Arc::clone(&hit.path));
            }
        }
        // No stats for an empty query: nothing was searched.
        self.stats = (self.active.is_some()).then(|| Stats {
            files_searched,
            files_matched: files.len(),
            elapsed: self.started.elapsed(),
        });
        let text = self.input.text();
        let complete = self.hits.len() < MAX_HITS && self.opts.mode == Mode::Query;
        self.last_complete = (complete && !text.trim().is_empty()).then(|| Complete {
            text,
            opts: self.opts,
            files: Arc::new(files),
        });
    }

    /// If `text` only adds AND terms to the last complete search, return its files.
    /// The new hits are a subset of the old hits, so no other file can match.
    fn narrow_files(&self, text: &str) -> Option<Files> {
        let prev = self.last_complete.as_ref()?;
        let same_opts = prev.opts == self.opts && self.opts.mode == Mode::Query;
        let rest = text.strip_prefix(prev.text.as_str())?;
        // The new text must start a new term, not extend the last one ("fo" -> "foo" is
        // not safe for regex terms).
        let new_term = prev.text.ends_with(char::is_whitespace) || rest.starts_with(char::is_whitespace);
        // OR anywhere changes the meaning, and so does a `|` in the new text (it binds
        // to the term before it). A trailing operator applies to the new term.
        let has_or = text.split_whitespace().any(|t| t == "OR") || rest.contains('|');
        let open_op = matches!(
            prev.text.split_whitespace().last(),
            Some("AND" | "NOT" | "-" | "!" | "|")
        );
        (same_opts && new_term && !rest.trim().is_empty() && !has_or && !open_op).then(|| Arc::clone(&prev.files))
    }

    /// The byte ranges of `path:` matches in a drawn path (QUERY.md, rule 4).
    pub fn path_ranges(&self, path: &str) -> Vec<std::ops::Range<usize>> {
        self.path_filter
            .as_ref()
            .map(|f| f.field_highlights("path", path))
            .unwrap_or_default()
    }

    pub fn is_marked(&self, hit: &Hit) -> bool {
        self.marks.iter().any(|m| m.is(hit))
    }

    fn toggle_mark(&mut self) {
        let Some(hit) = self.hits.get(self.selected) else {
            return;
        };
        match self.marks.iter().position(|m| m.is(hit)) {
            Some(i) => {
                self.marks.remove(i);
            }
            None => self.marks.push(Target::of(hit)),
        }
    }

    fn select_to(&mut self, i: usize) {
        let i = i.min(self.hits.len().saturating_sub(1));
        if i != self.selected {
            self.preview_scroll = 0;
            self.hscroll = 0;
        }
        self.selected = i;
    }

    fn select(&mut self, delta: isize) {
        self.select_to(self.selected.saturating_add_signed(delta));
    }

    /// Move the selection `dy` rows down the screen. With the prompt at the bottom,
    /// the list goes up, so down the screen is toward the first hit.
    fn move_visual(&mut self, dy: isize) {
        self.select(if self.bottom { -dy } else { dy });
    }

    /// Select the hit at the top (`top`) or bottom of the screen order.
    fn select_edge(&mut self, top: bool) {
        self.select_to(if top == self.bottom { usize::MAX } else { 0 });
    }

    /// Send a preview request unless the same request is already sent.
    pub fn request_preview(&mut self, req: Request) {
        if self.preview_sent.as_ref() != Some(&req) {
            let _ = self.preview_tx.send(preview::Msg::Window(req.clone()));
            self.preview_sent = Some(req);
        }
    }

    /// The query text or options changed. Stop the running search and wait for the debounce.
    fn query_changed(&mut self, search: &Search) {
        self.search_due = Some(Instant::now());
        self.current = search.cancel();
        self.searching = false;
    }

    /// The marked hits, else the selected hit.
    fn targets(&self) -> Vec<Target> {
        if !self.marks.is_empty() {
            self.marks.clone()
        } else {
            self.hits.get(self.selected).map(Target::of).into_iter().collect()
        }
    }

    fn open(&mut self) -> Action {
        self.open_targets(self.targets())
    }

    /// Open these hits in the editor and save the query to the history.
    fn open_targets(&mut self, targets: Vec<Target>) -> Action {
        if targets.is_empty() {
            return Action::None;
        }
        let _ = self.history.save(&self.input.text());
        let launch = Launch {
            cmd: self.editor.clone(),
            stdin: None,
            targets,
        };
        if self.editor_return {
            Action::Run(launch)
        } else {
            Action::Open(launch)
        }
    }

    /// Run the `[open.NAME]` command on the marked or selected hits. With `stay`, it
    /// runs in the background and `qf` stays open.
    fn open_with(&mut self, name: &str) -> Action {
        let Some(spec) = self.open_cmds.get(name).cloned() else {
            self.message = Some(Notice::error(format!("no [open.{name}] in the config")));
            return Action::None;
        };
        let targets = self.targets();
        if targets.is_empty() {
            return Action::None;
        }
        let _ = self.history.save(&self.input.text());
        let n = targets.len();
        let label = spec.label.unwrap_or_else(|| name.to_string());
        let launch = Launch {
            cmd: spec.cmd,
            stdin: spec.stdin,
            targets,
        };
        if !spec.stay {
            return Action::Open(launch);
        }
        self.message = Some(match open::spawn(&launch) {
            Ok(()) if n == 1 => Notice::info(format!("ran {label}")),
            Ok(()) => Notice::info(format!("ran {label} on {n} hits")),
            Err(e) => Notice::error(format!("{e:#}")),
        });
        Action::None
    }

    /// `C-o`: a popup menu of the `[open.NAME]` commands (the shared bs-tui-common
    /// menu, as in annals). `:open` in the command line still completes the names.
    fn show_open_menu(&mut self) {
        if self.open_cmds.is_empty() {
            self.message = Some(Notice::error("no [open.NAME] commands in the config (see qf --help)"));
            return;
        }
        let mut taken = String::new();
        let items = self
            .open_cmds
            .iter()
            .map(|(name, cmd)| {
                let label = cmd.label.clone().unwrap_or_else(|| name.clone());
                let key = menu_key(&label, name, &taken);
                taken.push(key);
                (key, name.clone(), label)
            })
            .collect();
        self.open_menu = Some(OpenMenu { selected: 0, items });
    }

    /// A key while the `C-o` menu is open.
    fn open_menu_key(&mut self, key: KeyEvent) -> Action {
        let Some(menu) = self.open_menu.as_mut() else {
            return Action::None;
        };
        let mut selected = menu.selected;
        let outcome = bs_tui_common::menu::key(&mut selected, &menu.menu_items(), &key);
        menu.selected = selected;
        match outcome {
            bs_tui_common::menu::Outcome::Open => Action::None,
            bs_tui_common::menu::Outcome::Close => {
                self.open_menu = None;
                Action::None
            }
            bs_tui_common::menu::Outcome::Pick(i) => {
                let name = menu.items[i].1.clone();
                self.open_menu = None;
                self.open_with(&name)
            }
        }
    }

    /// The `[open.NAME]` command with this Ctrl key.
    fn open_key(&self, c: char) -> Option<String> {
        self.open_cmds
            .iter()
            .find(|(_, o)| o.key.as_deref().and_then(config::ctrl_key) == Some(c))
            .map(|(name, _)| name.clone())
    }

    pub fn on_key(&mut self, key: KeyEvent, search: &Search) -> Action {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let page = self.page as isize;

        if self.help.is_some() {
            self.help_key(key);
            return if ctrl && key.code == KeyCode::Char('c') {
                Action::Quit
            } else {
                Action::None
            };
        }
        if self.open_menu.is_some() {
            return if ctrl && key.code == KeyCode::Char('c') {
                Action::Quit
            } else {
                self.open_menu_key(key)
            };
        }
        self.message = None;

        if self.cmdline.is_open() {
            return self.cmdline_key(key, ctrl, search);
        }

        // In the query prompt, Enter on an item that equals the typed word changes
        // nothing: close the menu and let Enter open the hit.
        if key.code == KeyCode::Enter && self.accept_is_noop() {
            self.close_menu();
        }
        if self.mode == EditMode::Insert
            && let Some(c) = self.completion.as_mut()
        {
            // Arrows move the way they point on the screen. With the prompt at
            // the bottom, the menu grows up from it, so Up is the next item.
            match bs_tui_common::complete::key(c, &key, self.bottom) {
                bs_tui_common::complete::Outcome::Moved => return Action::None,
                bs_tui_common::complete::Outcome::Accept => {
                    self.accept_completion(search);
                    return Action::None;
                }
                bs_tui_common::complete::Outcome::Close => {
                    self.close_menu();
                    return Action::None;
                }
                bs_tui_common::complete::Outcome::Ignored => {}
            }
        }

        // Keys for both modes.
        if ctrl
            && let KeyCode::Char(c) = key.code
            && let Some(name) = self.open_key(c)
        {
            return self.open_with(&name);
        }
        match key.code {
            KeyCode::Char('c') if ctrl => return Action::Quit,
            // Clear the whole query and type a new one, like C-l in a shell.
            KeyCode::Char('l') if ctrl => {
                self.input = LineEditor::new("");
                self.mode = EditMode::Insert;
                self.completion = None;
                self.history.reset();
                self.query_changed(search);
            }
            KeyCode::F(1) => self.help = Some(0),
            KeyCode::Enter => return self.open(),
            KeyCode::Char('o') if ctrl => self.show_open_menu(),
            KeyCode::Tab => {
                self.toggle_mark();
                self.select(1);
            }
            KeyCode::BackTab => {
                self.toggle_mark();
                self.select(-1);
            }
            // Shift-Up / Shift-Down scroll the preview, as in fzf.
            KeyCode::Up if shift => self.preview_scroll -= 1,
            KeyCode::Down if shift => self.preview_scroll += 1,
            KeyCode::Up => self.move_visual(-1),
            KeyCode::Down => self.move_visual(1),
            KeyCode::Char('k') if ctrl => self.move_visual(-1),
            KeyCode::Char('j') if ctrl => self.move_visual(1),
            KeyCode::PageUp => self.move_visual(-page),
            KeyCode::PageDown => self.move_visual(page),
            // C-/ shows or hides the preview, as in fzf. Terminals send it as C-_ or C-7.
            KeyCode::Char('/' | '_' | '7') if ctrl => self.show_preview = !self.show_preview,
            // C-q: all hits to the quickfix list, as in telescope.
            KeyCode::Char('q') if ctrl => {
                return self.open_targets(self.hits.iter().map(Target::of).collect());
            }
            _ => {
                return match self.mode {
                    EditMode::Insert => self.insert_key(key, ctrl, alt, search),
                    EditMode::Normal => self.normal_key(key, ctrl, search),
                };
            }
        }
        Action::None
    }

    /// Open the completion menu for the word under the cursor, or close it if there
    /// are no candidates.
    /// Accepting the selected item would not change the query text.
    fn accept_is_noop(&self) -> bool {
        let Some(c) = &self.completion else {
            return false;
        };
        let typed: String = self.input.chars()[c.start..c.end.min(self.input.len())]
            .iter()
            .collect();
        c.items.get(c.selected).is_some_and(|item| item.text == typed)
    }

    fn close_menu(&mut self) {
        self.completion = None;
        self.history_menu = false;
    }

    /// `C-r`: the history, filtered by the query text (fuzzy), newest first.
    fn open_history(&mut self) {
        self.completion = complete::history(self.history.items(), &self.input.text());
        self.history_menu = self.completion.is_some();
    }

    fn open_completion(&mut self, search: &Search) {
        if self.history_menu {
            self.open_history();
            return;
        }
        let files = search.files();
        let list: &[Arc<str>] = files.as_deref().map_or(&[], |f| f.as_slice());
        let key = files.as_ref().map_or(0, |f| Arc::as_ptr(f) as usize);
        if self.list_cache.as_ref().map(|c| c.key) != Some(key) {
            self.list_cache = Some(ListCache {
                key,
                exts: Arc::new(complete::ext_counts(list)),
                dirs: Arc::new(complete::dir_list(list)),
            });
        }
        if self.words_cache.as_ref().map(|c| c.0) != Some(self.shown) {
            let words = complete::hit_words(self.hits.iter().map(|h| h.text.as_str()));
            self.words_cache = Some((self.shown, Arc::new(words)));
        }
        let (exts, dirs) = match &self.list_cache {
            Some(c) => (Arc::clone(&c.exts), Arc::clone(&c.dirs)),
            None => Default::default(),
        };
        let words = self.words_cache.as_ref().map(|c| Arc::clone(&c.1)).unwrap_or_default();
        let src = Sources {
            files: list,
            dirs: &dirs,
            exts: &exts,
            words: &words,
            history: self.history.items(),
        };
        let completion = complete::complete(self.input.chars(), self.input.cursor(), &src);
        self.completion = completion;
    }

    fn accept_completion(&mut self, search: &Search) {
        let Some(c) = self.completion.take() else {
            return;
        };
        let item = &c.items[c.selected];
        if std::mem::take(&mut self.history_menu) {
            // A history entry replaces the whole query.
            self.input = LineEditor::new(&item.text);
            self.history.reset();
            self.query_changed(search);
            return;
        }
        self.input.replace_range(c.start, c.end, &item.text);
        if item.text.ends_with(':') || item.text.ends_with('/') {
            // A field name or a folder: offer what comes next.
            self.open_completion(search);
        } else if self
            .input
            .char_at(self.input.cursor())
            .is_none_or(|n| !n.is_whitespace())
        {
            // A complete term: add a space for the next one.
            self.input.insert(' ');
        }
        self.history.reset();
        self.query_changed(search);
    }

    /// Type `c`, with auto-pairs: `(` `"` `'` `/` add their closing character, and a
    /// closing character that is already next to the cursor is typed over.
    fn insert_char(&mut self, c: char) {
        let cur = self.input.cursor();
        let next = self.input.char_at(cur);
        let prev = cur.checked_sub(1).and_then(|i| self.input.char_at(i));
        if c == ' ' && self.auto_uppercase {
            self.uppercase_operator();
        }
        if !self.auto_pairs {
            self.input.insert(c);
            return;
        }
        let a = syntax::analyze(self.input.chars());
        let closes = match c {
            ')' => a.partner(cur).is_some_and(|o| o < cur),
            '"' | '\'' | '/' => a.quotes.iter().any(|q| q.1 == cur && q.2 == c),
            _ => false,
        };
        if next == Some(c) && closes {
            self.input.move_to(cur + 1);
            return;
        }
        // Inside an unclosed quote, a quote char closes it: no pair.
        let before = syntax::analyze(&self.input.chars()[..cur]);
        let in_quote = before.errors.iter().any(|(_, m)| m.starts_with("unclosed"));
        let term_start = prev.is_none_or(|p| p.is_whitespace() || p == '(');
        let next_free = next.is_none_or(|n| n.is_whitespace() || n == ')');
        let close = match c {
            _ if in_quote => None,
            '(' if term_start => Some(')'),
            '"' if next_free => Some('"'),
            '\'' | '/' if term_start && next_free => Some(c),
            _ => None,
        };
        self.input.insert(c);
        if let Some(close) = close {
            self.input.insert(close);
            self.input.move_to(cur + 1);
        }
    }

    /// Backspace. In an empty pair (`(|)`, `"|"`), delete both characters.
    fn backspace(&mut self) -> bool {
        let cur = self.input.cursor();
        if self.auto_pairs && cur > 0 {
            let a = syntax::analyze(self.input.chars());
            let empty_pair = a.partner(cur - 1) == Some(cur) || a.quotes.iter().any(|q| q.0 == cur - 1 && q.1 == cur);
            if empty_pair {
                return self.input.delete_range(cur - 1, cur + 1);
            }
        }
        self.input.backspace()
    }

    /// Before a space: `and`, `or` or `not` after another term become `AND`, `OR`, `NOT`.
    fn uppercase_operator(&mut self) {
        let chars = self.input.chars();
        let cur = self.input.cursor();
        let mut start = cur;
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        let word: String = chars[start..cur].iter().collect();
        let has_term_before = chars[..start].iter().any(|c| !c.is_whitespace());
        let is_op = match word.as_str() {
            "and" | "or" => has_term_before,
            "not" => true,
            _ => false,
        };
        if is_op {
            self.input.replace_range(start, cur, &word.to_uppercase());
        }
    }

    /// Keys in the `:` command line. `bs_tui_common::cmdline` edits it and
    /// drives its menu.
    fn cmdline_key(&mut self, key: KeyEvent, ctrl: bool, search: &Search) -> Action {
        match key.code {
            KeyCode::Char('c') if ctrl => return Action::Quit,
            KeyCode::F(1) => {
                self.help = Some(0);
                return Action::None;
            }
            // A complete command runs as typed, also with the menu open
            // (`:split 3`). In the `:open` menu, Enter runs the selected name.
            KeyCode::Enter if self.cmdline.completion().is_some_and(|c| c.start > 0) => {
                let line = self.cmdline.editor().map(|e| e.text()).unwrap_or_default();
                if command::name(&line) != Some("open")
                    && let Ok(c) = command::parse(&line)
                {
                    self.cmdline.close();
                    return self.run_command(c, search);
                }
            }
            _ => {}
        }
        match self.cmdline.key(&key) {
            cmdline::Outcome::Run(line) => match command::parse(&line) {
                Ok(c) => self.run_command(c, search),
                Err(e) => {
                    self.message = Some(Notice::error(e));
                    Action::None
                }
            },
            cmdline::Outcome::Error(e) => {
                self.message = Some(Notice::error(e));
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Open the `:` command line, with the text `Esc` left in it.
    fn open_cmdline(&mut self) {
        self.close_menu();
        self.cmdline.open();
    }

    fn run_command(&mut self, cmd: Command, search: &Search) -> Action {
        let mut changed = false;
        self.message = match cmd {
            Command::Sort(by) => {
                self.sort_by = by;
                if let Some(q) = self.active.as_mut() {
                    q.sort = None;
                }
                self.sort();
                None
            }
            Command::Context(n) => {
                self.opts.context = n;
                if n > 0 {
                    self.context_size = n;
                }
                changed = true;
                None
            }
            Command::Mode(m) => {
                self.opts.mode = if m == QueryMode::Regex {
                    Mode::Regex
                } else {
                    Mode::Query
                };
                self.opts.file_mode = m == QueryMode::File;
                changed = true;
                None
            }
            Command::SmartCase(on) => {
                self.opts.smart_case = on;
                changed = true;
                None
            }
            Command::Changed => {
                self.opts.changed = !self.opts.changed;
                changed = true;
                Some(Notice::info(format!(
                    "changed filter {}",
                    if self.opts.changed { "on" } else { "off" }
                )))
            }
            Command::Shared(Shared::Theme(bg)) if self.color => {
                self.set_theme(bg);
                None
            }
            Command::Shared(Shared::Theme(_)) => Some(Notice::error("colors are off (NO_COLOR)")),
            Command::Layout(l) => {
                self.layout = l;
                None
            }
            Command::Split(n) => {
                self.split = n;
                None
            }
            Command::Prompt(side) => {
                self.bottom = side == PromptSide::Bottom;
                None
            }
            Command::Set(name, on) => {
                match name {
                    "wrap" => {
                        self.wrap = on;
                        self.hscroll = 0;
                    }
                    "group" => self.group = on,
                    "preview" => self.show_preview = on,
                    "scrollbar" => self.scrollbar = on,
                    "watch" => self.watch = on,
                    "header" => self.show_header = on,
                    "ivy" => {
                        self.opts.ivy = on;
                        if on {
                            self.opts.mode = Mode::Regex;
                        }
                        changed = true;
                    }
                    _ => {}
                }
                None
            }
            Command::Marks { all } => {
                if all {
                    self.marks = self.hits.iter().map(Target::of).collect();
                } else {
                    self.marks.clear();
                }
                // The status line shows the mark count.
                None
            }
            Command::Open(None) => return self.open(),
            Command::Open(Some(name)) => return self.open_with(&name),
            Command::Shared(Shared::Help) => {
                self.help = Some(0);
                None
            }
            Command::Shared(Shared::Quit) => return Action::Quit,
        };
        if changed {
            self.query_changed(search);
        }
        Action::None
    }

    /// Switch the theme of the UI and the preview.
    fn set_theme(&mut self, bg: Background) {
        let name = match bg {
            Background::Dark => ThemeName::Dark,
            Background::Light => ThemeName::Light,
        };
        bs_tui_common::theme::switch(bg);
        self.palette = Palette::from_theme(&theme::load(name), self.theme_background, self.marker);
        let _ = self.preview_tx.send(preview::Msg::Theme(name));
        self.preview = None;
        self.preview_sent = None;
    }

    /// The shared help keys (bs-tui-common), plus F1 to close.
    fn help_key(&mut self, key: KeyEvent) {
        let scroll = self.help.unwrap_or(0);
        self.help = if key.code == KeyCode::F(1) {
            None
        } else {
            bs_tui_common::help::key(scroll, &key)
        };
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        if self.help.is_some() || self.open_menu.is_some() {
            return;
        }
        let pos = Position::new(ev.column, ev.row);
        let in_list = self.list_area.contains(pos);
        let in_preview = self.preview_area.is_some_and(|a| a.contains(pos));
        match ev.kind {
            MouseEventKind::ScrollDown if in_list => self.move_visual(WHEEL_STEP),
            MouseEventKind::ScrollUp if in_list => self.move_visual(-WHEEL_STEP),
            MouseEventKind::ScrollDown if in_preview => self.preview_scroll += WHEEL_STEP,
            MouseEventKind::ScrollUp if in_preview => self.preview_scroll -= WHEEL_STEP,
            MouseEventKind::Down(MouseButton::Left) if in_list => {
                // A click closes the completion menu, as in the other tools.
                self.close_menu();
                let row = (ev.row - self.list_area.y) as usize;
                if let Some(Some(i)) = self.list_rows.get(row) {
                    self.select_to(*i);
                }
            }
            _ => {}
        }
    }

    fn insert_key(&mut self, key: KeyEvent, ctrl: bool, alt: bool, search: &Search) -> Action {
        let e = &mut self.input;
        let changed = match key.code {
            KeyCode::Esc => {
                // Like vim: the cursor moves back onto the last typed char.
                let pos = e.cursor().saturating_sub(1);
                e.move_to(pos);
                self.mode = EditMode::Normal;
                false
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Char('h') if ctrl => self.backspace(),
            KeyCode::Char(' ') | KeyCode::Null if ctrl || key.code == KeyCode::Null => {
                self.open_completion(search);
                return Action::None;
            }
            KeyCode::Delete => {
                let c = e.cursor();
                e.delete_range(c, c + 1)
            }
            KeyCode::Char('w') if ctrl => {
                let to = e.blank_word_start();
                let c = e.cursor();
                e.delete_range(to, c)
            }
            KeyCode::Char('u') if ctrl => {
                let c = e.cursor();
                e.delete_range(0, c)
            }
            KeyCode::Left => {
                let pos = e.cursor().saturating_sub(1);
                e.move_to(pos);
                false
            }
            KeyCode::Right => {
                let pos = e.cursor() + 1;
                e.move_to(pos);
                false
            }
            KeyCode::Home => {
                e.move_to(0);
                false
            }
            KeyCode::End => {
                let len = e.len();
                e.move_to(len);
                false
            }
            // Readline: C-a / C-e start / end of line, C-b / C-f char back / forward.
            KeyCode::Char('a') if ctrl => {
                e.move_to(0);
                false
            }
            KeyCode::Char('e') if ctrl => {
                let len = e.len();
                e.move_to(len);
                false
            }
            KeyCode::Char('b') if ctrl => {
                let pos = e.cursor().saturating_sub(1);
                e.move_to(pos);
                false
            }
            KeyCode::Char('f') if ctrl => {
                let pos = e.cursor() + 1;
                e.move_to(pos);
                false
            }
            // C-r: search the history, as in shells and Claude Code.
            KeyCode::Char('r') if ctrl => {
                self.open_history();
                return Action::None;
            }
            KeyCode::Char('p') if ctrl => {
                if let Some(q) = self.history.prev(&self.input.text()) {
                    self.input = LineEditor::new(&q);
                    self.query_changed(search);
                }
                return Action::None;
            }
            KeyCode::Char('n') if ctrl => {
                if let Some(q) = self.history.next() {
                    self.input = LineEditor::new(&q);
                    self.query_changed(search);
                }
                return Action::None;
            }
            // `?` at an empty prompt opens the help, as in Claude Code. In a query,
            // `?` is a regex character.
            KeyCode::Char('?') if !ctrl && !alt && e.is_empty() => {
                self.help = Some(0);
                return Action::None;
            }
            // `:` at an empty prompt opens the command line, like `/` in Claude Code.
            _ if cmdline::opens(&key, true, e.is_empty()) => {
                self.open_cmdline();
                return Action::None;
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.insert_char(c);
                // After a field name, offer its values. `@` at the start of a term
                // opens the file picker.
                let at = self.input.cursor().saturating_sub(1);
                let term_start = at == 0
                    || self
                        .input
                        .char_at(at - 1)
                        .is_some_and(|p| p.is_whitespace() || matches!(p, '(' | '-' | '+'));
                let opens = c == ':' || (c == '@' && term_start);
                if opens && self.completion.is_none() {
                    self.open_completion(search);
                }
                true
            }
            _ => false,
        };
        if changed {
            if !self.history_menu {
                self.history.reset();
            }
            self.query_changed(search);
            if self.completion.is_some() {
                // A space ends the word, so the menu for it closes. Only the history
                // search filters by the whole query, spaces included.
                let cur = self.input.cursor();
                let word_ended = cur == 0 || self.input.char_at(cur - 1).is_some_and(char::is_whitespace);
                if word_ended && !self.history_menu {
                    self.close_menu();
                } else {
                    self.open_completion(search);
                }
            }
        } else if key.code != KeyCode::Esc {
            self.close_menu();
        }
        Action::None
    }

    fn normal_key(&mut self, key: KeyEvent, ctrl: bool, search: &Search) -> Action {
        let half = (self.page / 2).max(1) as isize;
        let page = self.page as isize;

        if let Some(op) = self.pending_op {
            let KeyCode::Char(c) = key.code else {
                self.pending_op = None;
                self.pending_obj = None;
                return Action::None;
            };
            match self.pending_obj.take() {
                Some(kind) => {
                    self.pending_op = None;
                    self.text_object(op, kind == 'a', c, search);
                }
                None if c == 'i' || c == 'a' => self.pending_obj = Some(c),
                None => {
                    self.pending_op = None;
                    self.operator(op, c, search);
                }
            }
            return Action::None;
        }
        if std::mem::take(&mut self.pending_z) {
            // vim: zh / zl scroll left / right.
            match key.code {
                KeyCode::Char('h') => self.hscroll = self.hscroll.saturating_sub(HSCROLL_STEP),
                KeyCode::Char('l') if !self.wrap => self.hscroll = self.hscroll.saturating_add(HSCROLL_STEP),
                _ => {}
            }
            return Action::None;
        }
        if std::mem::take(&mut self.pending_g) {
            match key.code {
                KeyCode::Char('g') => self.select_edge(true),
                // gU / gu + motion: uppercase / lowercase.
                KeyCode::Char(op @ ('U' | 'u')) => self.pending_op = Some(op),
                _ => {}
            }
            return Action::None;
        }

        let e = &mut self.input;
        match key.code {
            // vim: C-e / C-y scroll the view (here: the preview) by one line.
            KeyCode::Char('e') if ctrl => self.preview_scroll += 1,
            KeyCode::Char('y') if ctrl => self.preview_scroll -= 1,
            KeyCode::Char('d') if ctrl => self.move_visual(half),
            KeyCode::Char('u') if ctrl => self.move_visual(-half),
            KeyCode::Char('f') if ctrl => self.move_visual(page),
            KeyCode::Char('b') if ctrl => self.move_visual(-page),
            _ if ctrl => {}
            // Esc again clears the query (KEYS.md). It never quits: that is q, :q or C-c.
            KeyCode::Esc => {
                if !e.is_empty() {
                    self.input = LineEditor::new("");
                    self.completion = None;
                    self.history.reset();
                    self.query_changed(search);
                }
            }
            // `/` starts typing again, at the end of the query.
            KeyCode::Char('/') => {
                let len = e.len();
                e.move_to(len);
                self.mode = EditMode::Insert;
            }
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('?') => self.help = Some(0),
            KeyCode::Char(':') => self.open_cmdline(),
            KeyCode::Char('i') => self.mode = EditMode::Insert,
            KeyCode::Char('a') => {
                let pos = e.cursor() + 1;
                e.move_to(pos);
                self.mode = EditMode::Insert;
            }
            KeyCode::Char('I') => {
                let pos = e.first_non_blank();
                e.move_to(pos);
                self.mode = EditMode::Insert;
            }
            KeyCode::Char('A') => {
                let len = e.len();
                e.move_to(len);
                self.mode = EditMode::Insert;
            }
            KeyCode::Char('h') | KeyCode::Left => {
                let pos = e.cursor().saturating_sub(1);
                e.move_to(pos);
            }
            KeyCode::Char('l') | KeyCode::Right => {
                let pos = e.cursor() + 1;
                e.move_to(pos);
            }
            KeyCode::Char('0') | KeyCode::Home => e.move_to(0),
            KeyCode::Char('^') => {
                let pos = e.first_non_blank();
                e.move_to(pos);
            }
            KeyCode::Char('$') | KeyCode::End => {
                let len = e.len();
                e.move_to(len);
            }
            KeyCode::Char('w') => {
                let pos = e.word_forward();
                e.move_to(pos);
            }
            KeyCode::Char('b') => {
                let pos = e.word_backward();
                e.move_to(pos);
            }
            KeyCode::Char('e') => {
                let pos = e.word_end();
                e.move_to(pos);
            }
            KeyCode::Char('x') => self.operator('d', 'l', search),
            KeyCode::Char('X') => self.operator('d', 'h', search),
            KeyCode::Char('D') => self.operator('d', '$', search),
            KeyCode::Char('C') => self.operator('c', '$', search),
            KeyCode::Char('s') => self.operator('c', 'l', search),
            KeyCode::Char('S') => self.operator('c', 'c', search),
            KeyCode::Char(op @ ('d' | 'c')) => self.pending_op = Some(op),
            KeyCode::Char('j') => self.move_visual(1),
            KeyCode::Char('k') => self.move_visual(-1),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.select_edge(false),
            KeyCode::Char('R') => {
                self.watch = !self.watch;
                self.message = Some(Notice::info(format!("watch {}", if self.watch { "on" } else { "off" })));
            }
            KeyCode::Char('z') => self.pending_z = true,
            KeyCode::Char('%') => {
                // Jump to the matching parenthesis: of the one under the cursor, or
                // of the next one after it.
                let a = syntax::analyze(e.chars());
                let cur = e.cursor();
                let target = (cur..e.len()).find_map(|i| a.partner(i));
                if let Some(t) = target {
                    e.move_to(t);
                }
            }
            KeyCode::Backspace => {
                let pos = e.cursor().saturating_sub(1);
                e.move_to(pos);
            }
            _ => {}
        }
        if self.mode == EditMode::Normal {
            self.input.clamp_normal();
        }
        Action::None
    }

    /// Apply operator `op` to a text object: `iw aw`, `i( a(` (also `)` and `b`),
    /// `i" a"`, `i' a'`, `i/ a/`.
    fn text_object(&mut self, op: char, around: bool, obj: char, search: &Search) {
        let cur = self.input.cursor();
        let a = syntax::analyze(self.input.chars());
        let span = match obj {
            'w' => return self.apply(op, self.input.word_object(cur, around), search),
            '(' | ')' | 'b' => a.enclosing_pair(cur),
            '"' | '\'' | '/' => a.quote_span(cur, obj),
            _ => None,
        };
        let range = span.map(|(o, c)| if around { (o, c + 1) } else { (o + 1, c) });
        self.apply(op, range, search);
    }

    /// Apply vim operator `op` (`d` delete, `c` change, `U`/`u` case) with `motion`.
    fn operator(&mut self, op: char, motion: char, search: &Search) {
        let e = &self.input;
        let cur = e.cursor();
        let len = e.len();
        let range = match motion {
            // Like vim, `cw` changes to the end of the word.
            'w' if op == 'c' => Some((cur, e.word_end() + 1)),
            'w' => Some((cur, e.word_forward())),
            'e' => Some((cur, e.word_end() + 1)),
            'b' => Some((e.word_backward(), cur)),
            '$' => Some((cur, len)),
            '0' => Some((0, cur)),
            '^' => Some((e.first_non_blank(), cur)),
            'h' => Some((cur.saturating_sub(1), cur)),
            'l' => Some((cur, cur + 1)),
            m if m == op => Some((0, len)),
            _ => None,
        };
        self.apply(op, range, search);
    }

    fn apply(&mut self, op: char, range: Option<(usize, usize)>, search: &Search) {
        let Some((a, b)) = range else { return };
        let changed = match op {
            'U' | 'u' => self.input.change_case(a, b, op == 'U'),
            _ => self.input.delete_range(a, b),
        };
        if changed {
            self.history.reset();
            self.query_changed(search);
        }
        if op == 'c' {
            self.input.move_to(a.min(b));
            self.mode = EditMode::Insert;
        } else {
            self.input.clamp_normal();
        }
    }
}

/// The filter that highlights `path:` matches. `path:` globs are left out:
/// they match by glob, not by regex, so they get no highlight.
fn path_filter(text: &str, smart_case: bool) -> Option<bs_tui_common::query::Filter> {
    use bs_tui_common::query::{Kind, Node};
    fn globs<'a>(n: &'a Node, out: &mut Vec<&'a str>) {
        match n {
            Node::And(xs) | Node::Or(xs) => xs.iter().for_each(|x| globs(x, out)),
            Node::Not(x) => globs(x, out),
            Node::Field { name, value }
                if name == "path" && value.kind == Kind::Word && value.text.contains(['*', '?', '[', '{']) =>
            {
                out.push(&value.text)
            }
            _ => {}
        }
    }
    let node = bs_tui_common::query::parse(text, &query::FIELDS, smart_case).ok()?;
    let mut found = Vec::new();
    globs(&node, &mut found);
    let mut text = text.to_string();
    for g in found {
        text = text.replace(&format!("path:{g}"), "path:");
    }
    bs_tui_common::query::Filter::new(&text, &query::FIELDS, smart_case)
        .ok()
        .flatten()
}

#[cfg(test)]
mod path_filter_tests {
    use super::path_filter;

    #[test]
    fn a_path_glob_does_not_hide_the_highlight_of_a_plain_path_term() {
        let f = path_filter("path:*.rs path:main", true).unwrap();
        assert_eq!(f.field_highlights("path", "src/main.rs"), vec![4..8]);
        let f = path_filter("path:src/**/*.{rs,toml}", true);
        assert!(f.is_none_or(|f| f.field_highlights("path", "src/a/b.rs").is_empty()));
    }
}

#[cfg(test)]
mod open_menu_tests {
    use super::menu_key;

    #[test]
    fn keys_come_from_the_label_then_the_name() {
        assert_eq!(menu_key("Copy path", "copy", ""), 'c');
        // Taken: the next letter of the label.
        assert_eq!(menu_key("Copy path", "copy", "c"), 'o');
        // The label has nothing free: try the name.
        assert_eq!(menu_key("--", "zed", ""), 'z');
    }

    #[test]
    fn keys_skip_the_moving_and_closing_letters() {
        assert_eq!(menu_key("quick", "q", ""), 'u');
        assert_eq!(menu_key("jk", "jk", ""), 'a');
    }
}
