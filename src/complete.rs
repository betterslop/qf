//! Completion candidates for the word under the cursor in the prompt.
//!
//! `@` at the start of a term opens a fuzzy file and folder picker (like Claude Code).
//! A chosen path replaces `@…` with `path:…`. Without a choice, `@foo` stays text.
//!
//! An exact match stays in a menu, as the first item, so Tab or Enter can accept it.
//!
//! Field names, their fixed values and `AND` `OR` `NOT` come from
//! `bs_tui_common::filter::field_candidates`. qf adds paths, words and history.
//!
//! Every source is in memory: fixed lists, the cached file list, words from the
//! current hits, and the history. A lookup takes about a millisecond for 100k files.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use bs_tui_common::complete::{Candidate, Completion, MAX_ITEMS, rank};
use bs_tui_common::filter::field_candidates;
use ignore::types::TypesBuilder;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Do not collect more words than this from the hits.
const MAX_WORDS: usize = 5_000;

const FIELDS: [&str; 5] = ["path", "ext", "type", "git", "sort"];
const GIT: [&str; 4] = ["changed", "modified", "staged", "untracked"];
const SORT: [&str; 3] = ["relevance", "path", "modified"];

/// File extensions with their file counts, most common first.
pub type ExtCounts = Vec<(String, usize)>;

/// Data the candidates come from.
pub struct Sources<'a> {
    pub files: &'a [Arc<str>],
    /// Folders of the files (`src/`, `src/ui/`), sorted.
    pub dirs: &'a [Arc<str>],
    /// File extensions with their file counts, most common first.
    pub exts: &'a [(String, usize)],
    /// Words from the current hits, most common first.
    pub words: &'a [String],
    /// Earlier queries, oldest first.
    pub history: &'a [String],
}

/// The word that ends at the cursor: back to a space or `(`. A leading `-` or `!`
/// is not part of it.
fn token(chars: &[char], cursor: usize) -> (usize, String) {
    let mut start = cursor;
    while start > 0 && !chars[start - 1].is_whitespace() && chars[start - 1] != '(' {
        start -= 1;
    }
    if start < cursor && (chars[start] == '-' || chars[start] == '!') {
        start += 1;
    }
    (start, chars[start..cursor].iter().collect())
}

/// Candidates for the word that ends at `cursor`. `None` if there are none.
pub fn complete(chars: &[char], cursor: usize, src: &Sources) -> Option<Completion> {
    if chars.is_empty() {
        let items: Vec<Candidate> = src
            .history
            .iter()
            .rev()
            .take(MAX_ITEMS)
            .map(|h| Candidate::new(h.clone(), "history"))
            .collect();
        return (!items.is_empty()).then_some(Completion {
            start: 0,
            end: 0,
            items,
            selected: 0,
        });
    }
    let (start, word) = token(chars, cursor);
    // `path:` uses the same fuzzy picker as `@`.
    if let Some(needle) = word.strip_prefix('@').or_else(|| word.strip_prefix("path:")) {
        let items = fuzzy_paths(src, needle);
        return (!items.is_empty()).then_some(Completion {
            start,
            end: cursor,
            items,
            selected: 0,
        });
    }
    let values = |field: &str| -> Vec<String> {
        match field {
            "git" => GIT.iter().map(|s| s.to_string()).collect(),
            "sort" => SORT.iter().map(|s| s.to_string()).collect(),
            "type" => type_names().to_vec(),
            "ext" => src.exts.iter().map(|(e, _)| e.clone()).collect(),
            _ => Vec::new(),
        }
    };
    let mut items = field_candidates(chars, cursor, &FIELDS, &values).map_or_else(Vec::new, |c| c.items);
    if !word.is_empty() && !word.contains(':') {
        let words = rank(&word, src.words.iter().cloned());
        items.extend(words.into_iter().map(|text| Candidate::new(text, "word")));
    }
    let items: Vec<Candidate> = items.into_iter().take(MAX_ITEMS).collect();
    (!items.is_empty()).then_some(Completion {
        start,
        end: cursor,
        items,
        selected: 0,
    })
}

/// The menu shows the path. Accepting inserts `path:<path>`. Folders end with `/`.
fn path_candidate(p: &str) -> Candidate {
    Candidate {
        text: path_field(p),
        label: p.to_string(),
        kind: "",
        hl: Vec::new(),
    }
}

fn path_field(p: &str) -> String {
    // A bare value is a regex or glob. Quote it to keep the path literal.
    if p.contains(|c: char| c.is_whitespace() || "\\+()|^${}*?[]".contains(c)) {
        format!("path:\"{p}\"")
    } else {
        format!("path:{p}")
    }
}

/// Fuzzy matching with `nucleo-matcher` (the Helix editor's matcher, with fzf's
/// scoring). It finds the best placement of the typed letters, not the first one.
struct Fuzzy {
    pattern: Pattern,
    matcher: Matcher,
    buf: Vec<char>,
}

impl Fuzzy {
    /// Words in `needle` must all match (as in fzf). Smart case.
    fn new(needle: &str, config: Config) -> Self {
        Self {
            pattern: Pattern::new(needle, CaseMatching::Smart, Normalization::Smart, AtomKind::Fuzzy),
            matcher: Matcher::new(config),
            buf: Vec::new(),
        }
    }

    fn score(&mut self, hay: &str) -> Option<u32> {
        self.pattern.score(Utf32Str::new(hay, &mut self.buf), &mut self.matcher)
    }

    /// Matched char positions, sorted, for highlighting.
    fn indices(&mut self, hay: &str) -> Vec<u32> {
        let mut out = Vec::new();
        self.pattern
            .indices(Utf32Str::new(hay, &mut self.buf), &mut self.matcher, &mut out);
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Keep the best `MAX_ITEMS` of `scored` (highest score first, then shortest, then
/// by name), and sort only those.
fn best(mut scored: Vec<(u32, &str)>) -> Vec<(u32, &str)> {
    let order =
        |a: &(u32, &str), b: &(u32, &str)| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then_with(|| a.1.cmp(b.1));
    if scored.len() > MAX_ITEMS {
        scored.select_nth_unstable_by(MAX_ITEMS, order);
        scored.truncate(MAX_ITEMS);
    }
    scored.sort_by(order);
    scored
}

/// `@` picker: files and folders that fuzzy-match `needle`, best first. An exact
/// path comes first. With an empty needle: the top-level folders and files.
fn fuzzy_paths(src: &Sources, needle: &str) -> Vec<Candidate> {
    let entries = src.dirs.iter().chain(src.files.iter());
    if needle.is_empty() {
        let top = entries.filter(|p| !p.trim_end_matches('/').contains('/'));
        return top.take(MAX_ITEMS).map(|p| path_candidate(p)).collect();
    }
    let mut fuzzy = Fuzzy::new(needle, Config::DEFAULT.match_paths());
    let scored = entries
        .filter_map(|p| {
            let exact = p.eq_ignore_ascii_case(needle);
            fuzzy.score(p).map(|s| (if exact { u32::MAX } else { s }, &**p))
        })
        .collect();
    best(scored)
        .into_iter()
        .map(|(_, p)| Candidate {
            hl: fuzzy.indices(p),
            ..path_candidate(p)
        })
        .collect()
}

/// Every folder of the files, with a trailing `/`, sorted.
pub fn dir_list(files: &[Arc<str>]) -> Vec<Arc<str>> {
    let mut dirs = BTreeSet::new();
    for f in files {
        for (i, _) in f.match_indices('/') {
            dirs.insert(&f[..=i]);
        }
    }
    dirs.into_iter().map(Arc::from).collect()
}

/// ripgrep's file type names.
fn type_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut types = TypesBuilder::new();
        types.add_defaults();
        types.definitions().iter().map(|d| d.name().to_string()).collect()
    })
}

/// `C-r` history search: entries that fuzzy-match `query` (each word, as in fzf),
/// best first; all entries, newest first, for an empty query. A chosen entry
/// replaces the whole query.
pub fn history(history: &[String], query: &str) -> Option<Completion> {
    let items: Vec<Candidate> = if query.trim().is_empty() {
        history
            .iter()
            .rev()
            .take(MAX_ITEMS)
            .map(|h| Candidate::new(h.clone(), ""))
            .collect()
    } else {
        let mut fuzzy = Fuzzy::new(query, Config::DEFAULT);
        // Newest first among equal scores: `best` keeps input order for ties only by
        // name, so score newer entries one point higher.
        let scored = history
            .iter()
            .enumerate()
            .filter_map(|(i, h)| fuzzy.score(h).map(|s| (s * 1024 + i as u32, h.as_str())))
            .collect();
        best(scored)
            .into_iter()
            .map(|(_, h)| Candidate {
                hl: fuzzy.indices(h),
                ..Candidate::new(h.to_string(), "")
            })
            .collect()
    };
    let end = query.chars().count();
    (!items.is_empty()).then_some(Completion {
        start: 0,
        end,
        items,
        selected: 0,
    })
}

/// File extensions in the file list, most common first.
pub fn ext_counts(files: &[Arc<str>]) -> ExtCounts {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for f in files {
        if let Some(ext) = Path::new(&**f).extension() {
            *counts.entry(ext.to_string_lossy().to_lowercase()).or_default() += 1;
        }
    }
    let mut exts: Vec<_> = counts.into_iter().collect();
    exts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    exts
}

/// Identifiers (3+ chars) in the hit lines, most common first.
pub fn hit_words<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for line in lines {
        for w in line.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if w.chars().count() >= 3 && !w.chars().all(|c| c.is_ascii_digit()) {
                *counts.entry(w).or_default() += 1;
            }
        }
    }
    let mut words: Vec<_> = counts.into_iter().collect();
    words.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    words.into_iter().take(MAX_WORDS).map(|(w, _)| w.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(c: Option<Completion>) -> Vec<String> {
        c.map(|c| c.items.into_iter().map(|i| i.text).collect())
            .unwrap_or_default()
    }

    fn run(input: &str, src: &Sources) -> Vec<String> {
        let chars: Vec<char> = input.chars().collect();
        texts(complete(&chars, chars.len(), src))
    }

    #[test]
    fn candidates() {
        let files: Vec<Arc<str>> = ["Cargo.toml", "src/main.rs", "src/ui/list.rs", "src/ui/mod.rs"]
            .iter()
            .map(|s| Arc::from(*s))
            .collect();
        let exts = ext_counts(&files);
        let words = hit_words(["Search search SearchStatus", "Searcher"].into_iter());
        let history = vec!["old query".to_string()];
        let dirs = dir_list(&files);
        let src = Sources {
            files: &files,
            dirs: &dirs,
            exts: &exts,
            words: &words,
            history: &history,
        };

        assert_eq!(run("foo pa", &src), ["path:"]);
        assert_eq!(run("foo o", &src), ["OR"]);
        assert_eq!(run("git:st", &src), ["git:staged"]);
        assert_eq!(run("-git:un", &src), ["git:untracked"]);
        assert_eq!(run("ext:", &src), ["ext:rs", "ext:toml"]);
        assert_eq!(run("path:src/", &src)[..2], ["path:src/", "path:src/ui/"]);
        assert_eq!(run("git:staged", &src), ["git:staged"]);
        assert_eq!(run("foo OR", &src)[0], "OR");
        assert_eq!(run("path:uilist", &src), ["path:src/ui/list.rs"]);
        assert_eq!(run("(path:src/ui/l", &src), ["path:src/ui/list.rs"]);
        assert!(run("type:rus", &src).contains(&"type:rust".to_string()));
        assert_eq!(run("Searc", &src)[0], "Search");
        assert_eq!(run("", &src), ["old query"]);
        let c = complete(&"a (pa".chars().collect::<Vec<_>>(), 5, &src).unwrap();
        assert_eq!((c.start, c.end), (3, 5));

        // @ picker.
        assert_eq!(dirs.iter().map(|d| d.as_ref()).collect::<Vec<_>>(), ["src/", "src/ui/"]);
        assert_eq!(run("@uilist", &src)[0], "path:src/ui/list.rs");
        assert_eq!(run("@ui", &src)[0], "path:src/ui/");
        assert_eq!(run("foo -@main", &src), ["path:src/main.rs"]);
        assert_eq!(run("@", &src), ["path:src/", "path:Cargo.toml"]);
        assert!(run("@zzz", &src).is_empty());
    }

    #[test]
    fn history_search() {
        let h: Vec<String> = ["fn main", "TODO path:src", "struct App"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let texts = |q: &str| history(&h, q).map(|c| c.items.into_iter().map(|i| i.text).collect::<Vec<_>>());
        assert_eq!(texts("").unwrap(), ["struct App", "TODO path:src", "fn main"]);
        assert_eq!(texts("tdsrc").unwrap(), ["TODO path:src"]);
        assert!(texts("zzz").is_none());
    }

    /// The best placement wins: `fs` next to each other in a file name beats `f`
    /// and `s` far apart. A greedy (first match) matcher ranked these the other way.
    #[test]
    fn best_alignment() {
        let files: Vec<Arc<str>> = ["lib/fancy/settings.rs", "src/fooutils/fs.rs"]
            .iter()
            .map(|s| Arc::from(*s))
            .collect();
        let src = Sources {
            files: &files,
            dirs: &[],
            exts: &[],
            words: &[],
            history: &[],
        };
        let c = complete(&"@fs".chars().collect::<Vec<_>>(), 3, &src).unwrap();
        assert_eq!(c.items[0].label, "src/fooutils/fs.rs");
        assert_eq!(c.items[0].hl, [13, 14], "the highlight is on the file name");
    }

    /// Timing check: `cargo test --release -- --ignored --nocapture fuzzy_speed`.
    #[test]
    #[ignore]
    fn fuzzy_speed() {
        let files: Vec<Arc<str>> = (0..100_000)
            .map(|i| Arc::from(format!("pkg{}/src/module{}/file_{i}.rs", i % 50, i % 400).as_str()))
            .collect();
        let dirs = dir_list(&files);
        let src = Sources {
            files: &files,
            dirs: &dirs,
            exts: &[],
            words: &[],
            history: &[],
        };
        for needle in ["@m", "@mod12f", "@pkg7src", "@file_9999"] {
            let start = std::time::Instant::now();
            let n = run(needle, &src).len();
            println!("{needle:12} {n:3} items  {:?}", start.elapsed());
        }
    }
}
