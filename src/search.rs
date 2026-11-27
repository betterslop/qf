//! In-process search built on the ripgrep crates (`ignore`, `grep-regex`, `grep-searcher`).
//!
//! A background thread walks the tree once at startup and caches the file list.
//! Searches before the list is ready walk the tree. Later searches use the list.
//! A file watcher rebuilds the list when files are created, removed or renamed.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use crossbeam_channel::{Sender, unbounded};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkMatch};
use ignore::overrides::OverrideBuilder;
use ignore::types::TypesBuilder;
use ignore::{WalkBuilder, WalkState};
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Watcher};
use rayon::prelude::*;

use crate::AppEvent;
use crate::git::GitStatus;
use crate::query::{self, Query};

/// Stop a search after this many hits. Keeps memory bounded for short queries like "e".
pub const MAX_HITS: usize = 10_000;
/// Truncate matched lines to this many bytes for the results list.
const MAX_LINE_LEN: usize = 400;
/// Wait for file events to stop for this long before rebuilding the file list.
const REINDEX_DELAY: Duration = Duration::from_millis(300);
/// Files per step of a search over the file list. Files in one step run in
/// parallel. Steps run in path order, and the search checks the hit limit between
/// steps, so the hits kept at the limit always come from the first files by path.
const STEP: usize = 64;
/// Relevance bonus for a hit in a file whose name matches a term.
const NAME_BONUS: u32 = 8;

/// Reuse `git status` output for this long.
const GIT_STATUS_TTL: Duration = Duration::from_secs(2);

pub struct Hit {
    pub path: Arc<str>,
    pub line: u64,
    pub text: String,
    /// Byte ranges of term matches inside `text`, sorted, not overlapping.
    pub hl: Vec<(usize, usize)>,
    /// Context lines (line number, text) before and after the hit.
    pub before: Vec<(u64, String)>,
    pub after: Vec<(u64, String)>,
    /// Last modification time of the file, for `sort:modified`.
    pub modified: Option<SystemTime>,
    /// Relevance of the line: more and whole-word matches, and a matching file name,
    /// score higher. See `line_score`.
    pub score: u32,
    /// 1-based column (in chars) of the first match in the original line.
    pub col: u32,
}

pub enum Event {
    Hits(u64, Vec<Hit>),
    /// The search finished. The number is the count of files searched.
    Done(u64, usize),
}

/// Sorted list of file paths, relative to the current directory.
pub type Files = Arc<Vec<Arc<str>>>;

/// Progress numbers for the UI.
pub struct Status {
    /// Files searched by the current search.
    pub searched: usize,
    /// Files the current search will search, if known.
    pub total: Option<usize>,
    /// Files found by the first index build.
    pub indexed: usize,
    pub index_ready: bool,
}

/// Which files to search: the ripgrep-style command-line flags.
#[derive(Default)]
pub struct WalkConfig {
    /// Folders or files to search. Empty: the current directory.
    pub paths: Vec<PathBuf>,
    pub hidden: bool,
    pub no_ignore: bool,
    /// Globs to include, or to exclude with a `!` prefix.
    pub globs: Vec<String>,
    pub types: Vec<String>,
    pub types_not: Vec<String>,
}

impl WalkConfig {
    /// Build the walker. Fails on a bad glob or an unknown file type.
    fn builder(&self) -> Result<WalkBuilder> {
        let default = [PathBuf::from(".")];
        let paths = if self.paths.is_empty() {
            &default[..]
        } else {
            &self.paths[..]
        };
        let mut builder = WalkBuilder::new(&paths[0]);
        for path in &paths[1..] {
            builder.add(path);
        }
        builder.hidden(!self.hidden);
        // Respect .gitignore files also outside git repositories.
        builder.require_git(false);
        if self.no_ignore {
            builder
                .ignore(false)
                .git_ignore(false)
                .git_global(false)
                .git_exclude(false)
                .parents(false);
        }
        if !self.globs.is_empty() {
            let mut overrides = OverrideBuilder::new(".");
            for glob in &self.globs {
                overrides.add(glob).with_context(|| format!("invalid glob {glob}"))?;
            }
            builder.overrides(overrides.build()?);
        }
        if !self.types.is_empty() || !self.types_not.is_empty() {
            let mut types = TypesBuilder::new();
            types.add_defaults();
            for t in &self.types {
                types.select(t);
            }
            for t in &self.types_not {
                types.negate(t);
            }
            builder.types(types.build().context("invalid file type")?);
        }
        Ok(builder)
    }
}

pub struct Search {
    walker: Arc<WalkBuilder>,
    generation: Arc<AtomicU64>,
    index: Arc<RwLock<Option<Files>>>,
    indexed: Arc<AtomicUsize>,
    /// File counter and file total of the current search.
    progress: RwLock<(Arc<AtomicUsize>, Option<usize>)>,
    git: Mutex<Option<(Instant, Option<Arc<GitStatus>>)>>,
    tx: Sender<AppEvent>,
}

impl Search {
    pub fn new(tx: Sender<AppEvent>, config: &WalkConfig) -> Result<Self> {
        let walker = Arc::new(config.builder()?);
        let index = Arc::new(RwLock::new(None));
        let indexed = Arc::new(AtomicUsize::new(0));
        let (slot, counter, w) = (Arc::clone(&index), Arc::clone(&indexed), Arc::clone(&walker));
        let roots = if config.paths.is_empty() {
            vec![PathBuf::from(".")]
        } else {
            config.paths.clone()
        };
        let hidden = config.hidden;
        let app_tx = tx.clone();
        thread::spawn(move || keep_index(&w, &roots, hidden, &slot, &counter, &app_tx));
        Ok(Self {
            walker,
            generation: Arc::new(AtomicU64::new(0)),
            index,
            indexed,
            progress: RwLock::new((Arc::new(AtomicUsize::new(0)), None)),
            git: Mutex::new(None),
            tx,
        })
    }

    /// The cached file list, if it is ready.
    pub fn files(&self) -> Option<Files> {
        self.index.read().ok().and_then(|i| i.clone())
    }

    pub fn status(&self) -> Status {
        let (searched, total) = match self.progress.read() {
            Ok(p) => (p.0.load(Ordering::Relaxed), p.1),
            Err(_) => (0, None),
        };
        Status {
            searched,
            total,
            indexed: self.indexed.load(Ordering::Relaxed),
            index_ready: self.index.read().is_ok_and(|i| i.is_some()),
        }
    }

    /// Stop the running search, if any, and return a new id that no search uses.
    pub fn cancel(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Git status, from a cache that is at most `GIT_STATUS_TTL` old.
    fn git_status(&self) -> Option<Arc<GitStatus>> {
        let mut cache = self.git.lock().ok()?;
        if let Some((at, status)) = cache.as_ref()
            && at.elapsed() < GIT_STATUS_TTL
        {
            return status.clone();
        }
        let status = GitStatus::load().map(Arc::new);
        *cache = Some((Instant::now(), status.clone()));
        status
    }

    /// Start a new search and return its id. Any older search stops at its next check.
    /// `None` (empty query) sends `Done` at once, which clears the results.
    /// `only` limits the search to these files (narrowing).
    pub fn start(&self, query: Option<Query>, context: usize, only: Option<Files>) -> u64 {
        let id = self.cancel();
        let Some(query) = query else {
            let _ = self.tx.send(AppEvent::Search(Event::Done(id, 0)));
            return id;
        };
        let git = if query.needs_git() { self.git_status() } else { None };
        let job = Job {
            id,
            query,
            context,
            git,
            generation: Arc::clone(&self.generation),
            count: AtomicUsize::new(0),
            files: Arc::new(AtomicUsize::new(0)),
            tx: self.tx.clone(),
        };
        let files = only.or_else(|| self.index.read().ok().and_then(|i| i.clone()));
        if let Ok(mut p) = self.progress.write() {
            *p = (Arc::clone(&job.files), files.as_ref().map(|f| f.len()));
        }
        let walker = Arc::clone(&self.walker);
        thread::spawn(move || {
            match files {
                Some(files) => job.run_index(&files),
                None => job.run_walk(&walker),
            }
            let files = job.files.load(Ordering::Relaxed);
            let _ = job.tx.send(AppEvent::Search(Event::Done(id, files)));
        });
        id
    }
}

fn display_path(path: &Path) -> Arc<str> {
    path.strip_prefix("./").unwrap_or(path).to_string_lossy().into()
}

/// Build the file list, then rebuild it after files are created, removed or renamed.
/// After any change, including an edit of file content, send `AppEvent::Changed`.
fn keep_index(
    walker: &WalkBuilder,
    roots: &[PathBuf],
    hidden: bool,
    slot: &RwLock<Option<Files>>,
    counter: &AtomicUsize,
    app_tx: &Sender<AppEvent>,
) {
    let (tx, rx) = unbounded();
    // Event paths are absolute and canonical. Compare against the canonical root.
    let base = std::fs::canonicalize(".").ok();
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && let Some(list) = classify(&ev, base.as_deref(), hidden)
        {
            let _ = tx.send(list);
        }
    });
    // Without a watcher, the list stays as it was at startup.
    let _watcher = watcher.map(|mut w| {
        for root in roots {
            let _ = w.watch(root, RecursiveMode::Recursive);
        }
        w
    });

    let set = |files| {
        if let Ok(mut slot) = slot.write() {
            *slot = Some(Arc::new(files));
        }
    };
    set(build_index(walker, counter));
    while let Ok(mut list) = rx.recv() {
        while let Ok(l) = rx.recv_timeout(REINDEX_DELAY) {
            list |= l;
        }
        if list {
            set(build_index(walker, counter));
        }
        let _ = app_tx.send(AppEvent::Changed);
    }
}

/// `Some(true)` for events that add or remove files, `Some(false)` for events that
/// change file content, `None` for the rest. Without `--hidden`, events in hidden
/// directories (`.git`) do not count.
fn classify(ev: &notify::Event, base: Option<&Path>, hidden: bool) -> Option<bool> {
    let list = matches!(
        ev.kind,
        EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
    );
    let content = matches!(ev.kind, EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any));
    let visible = hidden
        || ev.paths.iter().any(|p| {
            let rel: PathBuf = base.and_then(|r| p.strip_prefix(r).ok()).unwrap_or(p).to_path_buf();
            !rel.components()
                .any(|c| matches!(c, Component::Normal(n) if n.to_string_lossy().starts_with('.')))
        });
    ((list || content) && visible).then_some(list)
}

/// Walk the tree and return the sorted file list. `counter` counts the files found.
fn build_index(walker: &WalkBuilder, counter: &AtomicUsize) -> Vec<Arc<str>> {
    counter.store(0, Ordering::Relaxed);
    let (tx, rx) = unbounded();
    walker.build_parallel().run(|| {
        let tx = tx.clone();
        Box::new(move |entry| {
            if let Ok(entry) = entry
                && entry.file_type().is_some_and(|t| t.is_file())
            {
                counter.fetch_add(1, Ordering::Relaxed);
                let _ = tx.send(display_path(entry.path()));
            }
            WalkState::Continue
        })
    });
    drop(tx);
    let mut files: Vec<_> = rx.into_iter().collect();
    files.sort_unstable();
    files
}

struct Job {
    id: u64,
    query: Query,
    context: usize,
    git: Option<Arc<GitStatus>>,
    generation: Arc<AtomicU64>,
    /// Hits found.
    count: AtomicUsize,
    /// Files searched.
    files: Arc<AtomicUsize>,
    tx: Sender<AppEvent>,
}

impl Job {
    /// A newer search started.
    fn cancelled(&self) -> bool {
        self.generation.load(Ordering::Relaxed) != self.id
    }

    /// The hit limit is reached.
    fn full(&self) -> bool {
        self.count.load(Ordering::Relaxed) >= MAX_HITS
    }

    fn searcher(&self) -> Searcher {
        SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .line_number(true)
            .before_context(self.context)
            .after_context(self.context)
            .build()
    }

    fn run_index(&self, files: &[Arc<str>]) {
        for step in files.chunks(STEP) {
            if self.cancelled() || self.full() {
                break;
            }
            let done = step.par_iter().try_for_each_init(
                || self.searcher(),
                |searcher, path| {
                    if self.file(searcher, Path::new(&**path), Arc::clone(path)) {
                        Ok(())
                    } else {
                        Err(())
                    }
                },
            );
            if done.is_err() {
                break;
            }
        }
    }

    fn run_walk(&self, walker: &WalkBuilder) {
        walker.build_parallel().run(|| {
            let mut searcher = self.searcher();
            Box::new(move |entry| {
                // Before the file list is ready, the walk order decides which hits
                // are kept at the limit.
                if self.cancelled() || self.full() {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
                if self.file(&mut searcher, entry.path(), display_path(entry.path())) {
                    WalkState::Continue
                } else {
                    WalkState::Quit
                }
            })
        });
    }

    /// Search one file and send its hits. Returns false when the search must stop.
    fn file(&self, searcher: &mut Searcher, path: &Path, display: Arc<str>) -> bool {
        if self.cancelled() {
            return false;
        }
        // Fields depend only on the path. Skip the file without reading it if they
        // already decide that it cannot match.
        let fields = self.query.file_fields(&display, self.git.as_deref());
        if !self.query.possible(fields) {
            return true;
        }
        self.files.fetch_add(1, Ordering::Relaxed);

        let hits = if !self.query.has_text() {
            // Only fields: list the file, with its first line.
            vec![file_hit(path, &display)]
        } else {
            let name = display.rsplit('/').next().unwrap_or_default();
            let name_bonus = if self.query.highlights(name.as_bytes()).is_empty() {
                0
            } else {
                NAME_BONUS
            };
            let mut sink = HitSink::new(self, Arc::clone(&display), fields, name_bonus);
            let _ = searcher.search_path(&self.query.prefilter, path, &mut sink);
            if !self.query.file_mode {
                sink.hits
            } else if !self.query.file_match(sink.present, fields) {
                Vec::new()
            } else if sink.hits.is_empty() {
                // The file matches with no positive term (`NOT foo`): list the file.
                vec![file_hit(path, &display)]
            } else {
                sink.hits
            }
        };
        if hits.is_empty() {
            return !self.cancelled();
        }
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        let hits: Vec<Hit> = hits.into_iter().map(|h| Hit { modified, ..h }).collect();
        self.count.fetch_add(hits.len(), Ordering::Relaxed);
        if self.tx.send(AppEvent::Search(Event::Hits(self.id, hits))).is_err() {
            return false;
        }
        !self.cancelled()
    }
}

/// A hit at line 1 for a file that matches without a matching line.
fn file_hit(path: &Path, display: &Arc<str>) -> Hit {
    let first = File::open(path)
        .ok()
        .and_then(|f| BufReader::new(f).lines().next())
        .and_then(|l| l.ok())
        .unwrap_or_default();
    Hit {
        path: Arc::clone(display),
        line: 1,
        text: clean(&first),
        hl: Vec::new(),
        before: Vec::new(),
        after: Vec::new(),
        modified: None,
        score: 0,
        col: 1,
    }
}

/// Collects the hits of one file, with context lines.
///
/// The searcher reports prefilter matches and the context around them. A prefilter
/// match that fails the query is a context line here. Every line within `context`
/// lines of a real hit reaches the sink, because real hits are prefilter matches.
struct HitSink<'a> {
    job: &'a Job,
    path: Arc<str>,
    fields: u64,
    hits: Vec<Hit>,
    /// Recent non-hit lines, for the context before the next hit.
    recent: VecDeque<(u64, String)>,
    /// File mode: the terms found in the file so far.
    present: u64,
    /// Added to the score of every hit in this file.
    name_bonus: u32,
}

impl<'a> HitSink<'a> {
    fn new(job: &'a Job, path: Arc<str>, fields: u64, name_bonus: u32) -> Self {
        Self {
            job,
            path,
            fields,
            hits: Vec::new(),
            recent: VecDeque::new(),
            present: 0,
            name_bonus,
        }
    }

    fn context_line(&mut self, line: u64, text: &str) {
        let n = self.job.context as u64;
        if n == 0 {
            return;
        }
        if let Some(last) = self.hits.last_mut()
            && line <= last.line + n
        {
            last.after.push((line, clean(text)));
        }
        self.recent.push_back((line, clean(text)));
        if self.recent.len() > self.job.context {
            self.recent.pop_front();
        }
    }
}

fn line_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\n', '\r'])
        .to_string()
}

impl Sink for HitSink<'_> {
    type Error = io::Error;

    fn matched(&mut self, _: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, io::Error> {
        let line = mat.line_number().unwrap_or(0);
        let text = line_text(mat.bytes());
        let query = &self.job.query;
        let is_hit = if query.file_mode {
            let terms = query.line_terms(text.as_bytes());
            self.present |= terms;
            query.is_positive(terms)
        } else {
            query.is_match(text.as_bytes(), self.fields)
        };
        if is_hit {
            let n = self.job.context as u64;
            let before = self
                .recent
                .drain(..)
                .filter(|(l, _)| l + n >= line && *l < line)
                .collect();
            // Match on the line as it is on disk, then map the ranges onto the drawn text.
            let raw = query.highlights(text.as_bytes());
            let (drawn, map) = clean_offsets(&text);
            let hl = query::remap(&raw, |o| map[o]);
            let score = line_score(&drawn, &hl) + self.name_bonus;
            let col = raw.first().map_or(1, |&(s, _)| (text[..s].chars().count() + 1) as u32);
            let text = drawn;
            self.hits.push(Hit {
                path: Arc::clone(&self.path),
                line,
                text,
                hl,
                before,
                after: Vec::new(),
                modified: None,
                score,
                col,
            });
        } else {
            self.context_line(line, &text);
        }
        // One file never gives more than the limit.
        Ok(!self.job.cancelled() && self.hits.len() < MAX_HITS)
    }

    fn context(&mut self, _: &Searcher, ctx: &SinkContext<'_>) -> Result<bool, io::Error> {
        self.context_line(ctx.line_number().unwrap_or(0), &line_text(ctx.bytes()));
        Ok(true)
    }

    fn context_break(&mut self, _: &Searcher) -> Result<bool, io::Error> {
        self.recent.clear();
        Ok(true)
    }
}

/// Relevance of one line: 1 per match, plus 1 if the match starts a word and 2 more
/// if it is a whole word. `search` in `fn search(` scores 4; in `research` it scores 1.
fn line_score(text: &str, hl: &[(usize, usize)]) -> u32 {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let score: u32 = hl
        .iter()
        .map(|&(s, e)| {
            let starts = !word(text[..s].chars().next_back());
            let ends = !word(text[e..].chars().next());
            1 + u32::from(starts) + 2 * u32::from(starts && ends)
        })
        .sum();
    score.min(20)
}

/// Trim leading whitespace, replace tabs and control characters with spaces,
/// and truncate long lines.
fn clean(text: &str) -> String {
    clean_with(text, None)
}

/// `clean`, and the cleaned byte offset of every byte offset of `text` (`text.len() + 1`
/// entries), to map match ranges from the file line onto the drawn text.
fn clean_offsets(text: &str) -> (String, Vec<usize>) {
    let mut map = Vec::new();
    let out = clean_with(text, Some(&mut map));
    (out, map)
}

fn clean_with(text: &str, mut map: Option<&mut Vec<usize>>) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_LINE_LEN));
    // Leading whitespace is dropped: its offsets map to the start.
    let mut leading = true;
    for c in text.chars() {
        leading &= c.is_whitespace();
        if !leading && out.len() >= MAX_LINE_LEN {
            break;
        }
        if let Some(m) = map.as_deref_mut() {
            m.extend(std::iter::repeat_n(out.len(), c.len_utf8()));
        }
        if !leading {
            out.push(if c.is_control() { ' ' } else { c });
        }
    }
    if let Some(m) = map {
        // Truncated or end of line: the rest maps to the end.
        m.resize(text.len() + 1, out.len());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores() {
        assert_eq!(line_score("fn search(", &[(3, 9)]), 4);
        assert_eq!(line_score("research", &[(2, 8)]), 1);
        assert_eq!(line_score("searcher", &[(0, 6)]), 2);
        assert_eq!(line_score("x", &[]), 0);
    }

    /// Hit text and ranges for one raw line, as `HitSink` computes them.
    fn drawn(query: &str, raw: &str) -> (String, Vec<(usize, usize)>) {
        let opts = crate::query::Options {
            mode: crate::query::Mode::Query,
            ivy: false,
            smart_case: true,
            file_mode: false,
            changed: false,
            context: 0,
        };
        let q = crate::query::compile(query, opts).unwrap().unwrap();
        assert!(q.is_match(raw.as_bytes(), 0), "{query} must match the raw line");
        let (text, map) = clean_offsets(raw);
        let hl = query::remap(&q.highlights(raw.as_bytes()), |o| map[o]);
        (text, hl)
    }

    #[test]
    fn highlights_map_onto_drawn_text() {
        let (t, hl) = drawn("/^\\s+fn/", "    fn main");
        assert_eq!((t.as_str(), hl), ("fn main", vec![(0, 2)]));
        let (t, hl) = drawn("/foo\\tbar/", "foo\tbar");
        assert_eq!((t.as_str(), hl), ("foo bar", vec![(0, 7)]));
        let (t, hl) = drawn("bar", "\t\ta\u{1}bar");
        assert_eq!((t.as_str(), &hl[..]), ("a bar", &[(2, 5)][..]));
        // A match only in the dropped indent has nothing to draw.
        let (_, hl) = drawn("/^\\s+/", "   x");
        assert!(hl.is_empty());
        // Multi-byte text before the match, and a C1 control (2 bytes) drawn as one space.
        let (t, hl) = drawn("x", "\u{e9}\u{85}x");
        assert_eq!((t.as_str(), hl), ("\u{e9} x", vec![(3, 4)]));
    }

    #[test]
    fn clean_offsets_cover_the_line() {
        let (t, map) = clean_offsets("  ab");
        assert_eq!((t.as_str(), map), ("ab", vec![0, 0, 0, 1, 2]));
        assert_eq!(clean("  ab\tc"), "ab c");
        let long = "x".repeat(MAX_LINE_LEN + 10);
        let (t, map) = clean_offsets(&long);
        assert_eq!(t.len(), MAX_LINE_LEN);
        assert_eq!(map.len(), long.len() + 1);
        assert_eq!(*map.last().unwrap(), MAX_LINE_LEN);
    }
}
