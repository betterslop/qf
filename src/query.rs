//! Query syntax. `bs_tui_common::query` parses the input (spec: bs-tui-common `QUERY.md`);
//! this module evaluates the parsed tree against each line.
//!
//! - `foo bar`, `foo AND bar`: both terms on the line
//! - `foo OR bar`, `(a OR b) AND c`, `a b | c` (`|` binds tighter than a space)
//! - `foo -bar`, `foo !bar`, `NOT foo`
//! - `"exact phrase"`, `'exact phrase'`: literal text
//! - `/regex/`: regex, may contain spaces
//! - bare words are regex; an invalid regex (`foo(`, `C++`) matches as literal text
//! - each term uses smart case: `exacT` is case-sensitive, `exact` is not
//!
//! Fields filter files. They combine with text terms in the same boolean tree:
//! - `path:src/ui` (a word: regex or literal), `path:"a.b"` (literal), `path:/re/` (regex),
//!   `path:*.rs` (glob: an unquoted value with `*`, `?`, `[` or `{`)
//! - `ext:rs`, `type:rust` (ripgrep file types)
//! - `git:changed`, `git:modified`, `git:staged`, `git:untracked`
//! - `sort:path`, `sort:modified`: a setting, not a filter
//!
//! A partial or unknown field value (`git:c`, `type:ru`) matches nothing and sets `Query::hint`.
//! A query with only fields lists the matching files.
//! In file mode, terms and fields apply to the whole file, not to one line.

use std::path::Path;
use std::sync::Arc;

use bs_tui_common::query::{Kind, Node, Pattern};
use globset::{GlobBuilder, GlobMatcher};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use ignore::types::{Types, TypesBuilder};

use crate::git::GitStatus;

/// Stop collecting highlight ranges for one line after this many.
const MAX_HIGHLIGHTS: usize = 64;
/// Terms and fields are bits in a `u64`.
const MAX_TERMS: usize = 64;
pub const FIELDS: [&str; 5] = ["path", "ext", "type", "git", "sort"];

#[derive(Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Boolean query syntax.
    Query,
    /// The whole input is one regex.
    Regex,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SortBy {
    /// Best files first: whole-word matches, a matching file name, more hits, newer.
    Relevance,
    /// By path, then line.
    Path,
    /// Most recently modified file first.
    Modified,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub mode: Mode,
    /// Regex mode only: spaces become `.*` (ivy-regex-plus style).
    pub ivy: bool,
    /// Per term: case-insensitive unless the term has an uppercase letter.
    /// When false, all terms are case-insensitive.
    pub smart_case: bool,
    /// Terms and fields apply to the whole file.
    pub file_mode: bool,
    /// Only files changed in git (`--changed`): adds `git:changed`.
    pub changed: bool,
    /// Context lines before and after each hit.
    pub context: usize,
}

#[derive(Clone, Copy)]
enum GitField {
    Changed,
    Modified,
    Staged,
    Untracked,
}

/// A file-level condition.
#[derive(Clone)]
enum Field {
    /// A partial or unknown value: matches no file.
    Nothing,
    PathGlob(GlobMatcher),
    PathRegex(RegexMatcher),
    Ext(String),
    Type(Arc<Types>),
    Git(GitField),
}

impl Field {
    fn matches(&self, path: &str, git: Option<&GitStatus>) -> bool {
        match self {
            Field::Nothing => false,
            Field::PathGlob(glob) => glob.is_match(path),
            Field::PathRegex(re) => re.is_match(path.as_bytes()).unwrap_or(false),
            Field::Ext(ext) => Path::new(path)
                .extension()
                .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext)),
            Field::Type(types) => types.matched(path, false).is_whitelist(),
            Field::Git(g) => {
                let Some(status) = git else { return false };
                let f = status.get(path);
                match g {
                    GitField::Changed => f.staged || f.modified || f.untracked,
                    GitField::Modified => f.modified,
                    GitField::Staged => f.staged,
                    GitField::Untracked => f.untracked,
                }
            }
        }
    }
}

#[derive(Clone)]
enum Expr {
    Term(usize),
    Field(usize),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
}

#[derive(Clone)]
pub struct Query {
    /// Finds candidate lines fast. In line mode, every matching line matches this.
    /// In file mode, every line that matches any term matches this.
    pub prefilter: RegexMatcher,
    terms: Vec<RegexMatcher>,
    /// Terms that are not under a NOT. Used to highlight the match.
    positive: Vec<usize>,
    positive_mask: u64,
    fields: Vec<Field>,
    expr: Expr,
    pub file_mode: bool,
    /// From `sort:` in the query.
    pub sort: Option<SortBy>,
    /// Why a field value matches nothing (for the status line). Not an error.
    pub hint: Option<String>,
}

impl Query {
    /// The query has text terms. Without them, it lists the files that match the fields.
    pub fn has_text(&self) -> bool {
        !self.terms.is_empty()
    }

    pub fn needs_git(&self) -> bool {
        self.fields.iter().any(|f| matches!(f, Field::Git(_)))
    }

    /// Bit `i` is set if field `i` matches the file.
    pub fn file_fields(&self, path: &str, git: Option<&GitStatus>) -> u64 {
        self.fields
            .iter()
            .enumerate()
            .fold(0, |bits, (i, f)| bits | (u64::from(f.matches(path, git)) << i))
    }

    /// False if no content can make the file match. Check this before reading the file.
    pub fn possible(&self, fields: u64) -> bool {
        self.maybe(&self.expr, fields) != Some(false)
    }

    /// Line mode: the line matches the query, in a file with these fields.
    pub fn is_match(&self, line: &[u8], fields: u64) -> bool {
        self.eval(&self.expr, &|i| self.terms[i].is_match(line).unwrap_or(false), fields)
    }

    /// File mode: bit `i` is set if term `i` matches the line.
    pub fn line_terms(&self, line: &[u8]) -> u64 {
        self.terms.iter().enumerate().fold(0, |bits, (i, t)| {
            bits | (u64::from(t.is_match(line).unwrap_or(false)) << i)
        })
    }

    /// File mode: a line with these terms is a hit (it has a positive term).
    pub fn is_positive(&self, terms: u64) -> bool {
        terms & self.positive_mask != 0
    }

    /// File mode: the file matches, given the terms found in it.
    pub fn file_match(&self, present: u64, fields: u64) -> bool {
        self.eval(&self.expr, &|i| present >> i & 1 == 1, fields)
    }

    /// Byte ranges of all positive term matches in `line`, sorted and merged.
    pub fn highlights(&self, line: &[u8]) -> Vec<(usize, usize)> {
        let mut found = Vec::new();
        for &i in &self.positive {
            let _ = self.terms[i].find_iter(line, |m| {
                if m.start() < m.end() {
                    found.push((m.start(), m.end()));
                }
                found.len() < MAX_HIGHLIGHTS
            });
        }
        found.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(found.len());
        for (s, e) in found {
            match merged.last_mut() {
                Some(last) if s <= last.1 => last.1 = last.1.max(e),
                _ => merged.push((s, e)),
            }
        }
        merged
    }

    fn eval(&self, expr: &Expr, term: &dyn Fn(usize) -> bool, fields: u64) -> bool {
        match expr {
            Expr::Term(i) => term(*i),
            Expr::Field(i) => fields >> i & 1 == 1,
            Expr::And(xs) => xs.iter().all(|x| self.eval(x, term, fields)),
            Expr::Or(xs) => xs.iter().any(|x| self.eval(x, term, fields)),
            Expr::Not(x) => !self.eval(x, term, fields),
        }
    }

    /// Three-valued evaluation with unknown terms: `None` means it depends on the content.
    fn maybe(&self, expr: &Expr, fields: u64) -> Option<bool> {
        match expr {
            Expr::Term(_) => None,
            Expr::Field(i) => Some(fields >> i & 1 == 1),
            Expr::And(xs) => {
                let mut unknown = false;
                for x in xs {
                    match self.maybe(x, fields) {
                        Some(false) => return Some(false),
                        None => unknown = true,
                        Some(true) => {}
                    }
                }
                if unknown { None } else { Some(true) }
            }
            Expr::Or(xs) => {
                let mut unknown = false;
                for x in xs {
                    match self.maybe(x, fields) {
                        Some(true) => return Some(true),
                        None => unknown = true,
                        Some(false) => {}
                    }
                }
                if unknown { None } else { Some(false) }
            }
            Expr::Not(x) => self.maybe(x, fields).map(|b| !b),
        }
    }
}

/// Map byte ranges through an offset map. Ranges that become empty are dropped.
pub fn remap(ranges: &[(usize, usize)], map: impl Fn(usize) -> usize) -> Vec<(usize, usize)> {
    ranges
        .iter()
        .map(|&(s, e)| (map(s), map(e)))
        .filter(|&(s, e)| s < e)
        .collect()
}

/// Compile the input. Returns `Ok(None)` for an empty input.
pub fn compile(input: &str, opts: Options) -> Result<Option<Query>, String> {
    let input = input.trim();
    if input.is_empty() && !opts.changed {
        return Ok(None);
    }

    let mut b = Builder::new(opts.smart_case);
    let mut expr = match opts.mode {
        _ if input.is_empty() => Expr::And(Vec::new()),
        Mode::Regex => b.regex(&regex_mode_pattern(input, opts.ivy), false)?,
        Mode::Query => b.node(&bs_tui_common::query::parse(input, &FIELDS, opts.smart_case)?, false)?,
    };

    if opts.changed {
        let changed = b.push_field(Field::Git(GitField::Changed));
        expr = Expr::And(vec![expr, changed]);
    }
    if b.fields.len() > MAX_TERMS || (opts.file_mode && b.terms.len() > MAX_TERMS) {
        return Err(format!("too many terms (at most {MAX_TERMS})"));
    }

    let ids = if opts.file_mode {
        // File mode must see every line with any term, also terms under NOT.
        Some((0..b.terms.len()).collect())
    } else {
        candidates(&expr)
    };
    let prefilter = match ids {
        Some(ids) => ids
            .iter()
            .map(|&i| format!("(?:{})", b.patterns[i]))
            .collect::<Vec<_>>()
            .join("|"),
        // The query can match a line with no positive term (`NOT foo`): scan every line.
        None => String::new(),
    };
    let prefilter = RegexMatcherBuilder::new()
        .case_insensitive(true)
        .line_terminator(Some(b'\n'))
        .build(&prefilter)
        .map_err(|e| e.to_string())?;

    let positive_mask = b
        .positive
        .iter()
        .fold(0u64, |m, &i| m | 1u64.checked_shl(i as u32).unwrap_or(0));
    Ok(Some(Query {
        prefilter,
        terms: b.terms,
        positive: b.positive,
        positive_mask,
        fields: b.fields,
        expr,
        file_mode: opts.file_mode,
        sort: b.sort,
        hint: b.hint,
    }))
}

fn regex_mode_pattern(input: &str, ivy: bool) -> String {
    if ivy {
        input.split_whitespace().collect::<Vec<_>>().join(".*")
    } else {
        input.to_string()
    }
}

/// Terms of which at least one must match for `expr` to match. `None` means no such set exists.
fn candidates(expr: &Expr) -> Option<Vec<usize>> {
    match expr {
        Expr::Term(i) => Some(vec![*i]),
        Expr::And(xs) => xs.iter().find_map(candidates),
        Expr::Or(xs) => xs
            .iter()
            .map(candidates)
            .collect::<Option<Vec<_>>>()
            .map(|v| v.concat()),
        // Fields do not limit which lines can match.
        Expr::Not(_) | Expr::Field(_) => None,
    }
}

type BuildResult = Result<Expr, String>;

struct Builder {
    smart_case: bool,
    patterns: Vec<String>,
    terms: Vec<RegexMatcher>,
    positive: Vec<usize>,
    fields: Vec<Field>,
    sort: Option<SortBy>,
    hint: Option<String>,
}

impl Builder {
    fn new(smart_case: bool) -> Self {
        Self {
            smart_case,
            patterns: Vec::new(),
            terms: Vec::new(),
            positive: Vec::new(),
            fields: Vec::new(),
            sort: None,
            hint: None,
        }
    }

    fn push_field(&mut self, field: Field) -> Expr {
        self.fields.push(field);
        Expr::Field(self.fields.len() - 1)
    }

    /// A value that matches nothing yet. Keep the first reason as the hint.
    fn nothing(&mut self, hint: String) -> Field {
        self.hint.get_or_insert(hint);
        Field::Nothing
    }

    fn field(&mut self, name: &str, pattern: &Pattern) -> BuildResult {
        let value = pattern.text.as_str();
        // An empty value matches all.
        if value.is_empty() {
            return Ok(Expr::And(Vec::new()));
        }
        let field = match name {
            // Only an unquoted word can be a glob. A quoted value is literal.
            "path" if pattern.kind == Kind::Word && value.contains(['*', '?', '[', '{']) => {
                match GlobBuilder::new(value)
                    .literal_separator(false)
                    .case_insensitive(pattern.ignore_case)
                    .build()
                {
                    Ok(glob) => Field::PathGlob(glob.compile_matcher()),
                    Err(e) => self.nothing(format!("path:{value}: {}", e.kind())),
                }
            }
            "path" => {
                let build = |src: &str| {
                    RegexMatcherBuilder::new()
                        .case_insensitive(pattern.ignore_case)
                        .build(src)
                };
                match build(&pattern.regex()) {
                    Ok(re) => Field::PathRegex(re),
                    Err(e) if pattern.kind == Kind::Regex => return Err(e.to_string()),
                    Err(_) => Field::PathRegex(build(&regex_syntax::escape(value)).map_err(|e| e.to_string())?),
                }
            }
            "ext" => Field::Ext(value.trim_start_matches('.').to_string()),
            "type" => {
                let mut types = TypesBuilder::new();
                types.add_defaults().select(value);
                match types.build() {
                    Ok(types) => Field::Type(Arc::new(types)),
                    Err(_) => self.nothing(format!("type:{value}: unknown type (rg --type-list)")),
                }
            }
            "git" => match value {
                "changed" => Field::Git(GitField::Changed),
                "modified" => Field::Git(GitField::Modified),
                "staged" => Field::Git(GitField::Staged),
                "untracked" => Field::Git(GitField::Untracked),
                _ => self.nothing(format!("git:{value}: use changed, modified, staged or untracked")),
            },
            "sort" => {
                self.sort = match value {
                    "relevance" => Some(SortBy::Relevance),
                    "path" => Some(SortBy::Path),
                    "modified" | "recent" => Some(SortBy::Modified),
                    _ => {
                        self.nothing(format!("sort:{value}: use relevance, path or modified"));
                        return Ok(Expr::Or(Vec::new()));
                    }
                };
                // A setting, not a filter: always true.
                return Ok(Expr::And(Vec::new()));
            }
            _ => unreachable!("FIELDS lists the field names"),
        };
        Ok(self.push_field(field))
    }

    /// `ignore_case` comes from the parsed pattern. `None`: grep_regex smart case.
    fn matcher(&self, pattern: &str, ignore_case: Option<bool>) -> Result<RegexMatcher, String> {
        let mut b = RegexMatcherBuilder::new();
        match ignore_case {
            Some(i) => b.case_insensitive(i),
            None => b.case_smart(self.smart_case).case_insensitive(!self.smart_case),
        };
        b.build(pattern).map_err(|e| e.to_string())
    }

    fn push(&mut self, pattern: String, matcher: RegexMatcher, negated: bool) -> Expr {
        let i = self.terms.len();
        self.patterns.push(pattern);
        self.terms.push(matcher);
        if !negated {
            self.positive.push(i);
        }
        Expr::Term(i)
    }

    /// Regex mode: the whole input, grep_regex smart case.
    fn regex(&mut self, pattern: &str, negated: bool) -> Result<Expr, String> {
        let m = self.matcher(pattern, None)?;
        Ok(self.push(pattern.to_string(), m, negated))
    }

    /// A parsed term: the shared crate decides regex or literal and the case.
    /// A word that grep_regex rejects is literal text.
    fn term(&mut self, p: &Pattern, negated: bool) -> Result<Expr, String> {
        let ic = Some(p.ignore_case);
        let src = p.regex();
        match self.matcher(&src, ic) {
            Ok(m) => Ok(self.push(src, m, negated)),
            Err(_) if p.kind == Kind::Word => {
                let src = regex_syntax::escape(&p.text);
                let m = self.matcher(&src, ic)?;
                Ok(self.push(src, m, negated))
            }
            Err(e) => Err(e),
        }
    }

    fn node(&mut self, node: &Node, negated: bool) -> BuildResult {
        Ok(match node {
            Node::All => Expr::And(Vec::new()),
            Node::And(xs) => Expr::And(xs.iter().map(|x| self.node(x, negated)).collect::<Result<_, _>>()?),
            Node::Or(xs) => Expr::Or(xs.iter().map(|x| self.node(x, negated)).collect::<Result<_, _>>()?),
            Node::Not(x) => Expr::Not(Box::new(self.node(x, !negated)?)),
            Node::Field { name, value } => return self.field(name, value),
            Node::Text(p) => self.term(p, negated)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTS: Options = Options {
        mode: Mode::Query,
        ivy: false,
        smart_case: true,
        file_mode: false,
        changed: false,
        context: 0,
    };

    fn matches(query: &str, line: &str) -> bool {
        let q = compile(query, OPTS).unwrap().unwrap();
        q.prefilter.is_match(line.as_bytes()).unwrap() && q.is_match(line.as_bytes(), 0)
    }

    /// Does the query accept `path`, with `line` as the only line of the file?
    fn file(query: &str, path: &str, line: &str) -> bool {
        let q = compile(query, OPTS).unwrap().unwrap();
        let fields = q.file_fields(path, None);
        q.possible(fields) && (!q.has_text() || q.is_match(line.as_bytes(), fields))
    }

    #[test]
    fn fields() {
        assert!(file("foo path:src/ui", "src/ui/list.rs", "foo"));
        assert!(!file("foo path:src/ui", "src/main.rs", "foo"));
        assert!(file("foo -path:test", "src/a.rs", "foo"));
        assert!(!file("foo -path:test", "tests/a.rs", "foo"));
        assert!(file("path:*.rs", "src/ui/list.rs", ""));
        assert!(!file("path:*.rs", "Cargo.toml", ""));
        assert!(file("path:*.{rs,toml}", "Cargo.toml", ""));
        assert!(file("path:/ui\\/.*rs$/", "src/ui/list.rs", ""));
        assert!(file("ext:rs foo", "a.rs", "foo"));
        assert!(file("ext:.RS", "a.rs", ""));
        assert!(file("type:rust", "a.rs", ""));
        assert!(!file("type:rust", "a.py", ""));
        assert!(file("foo OR path:docs", "docs/x.md", "anything"));
        assert!(!file("foo OR path:docs", "src/x.md", "anything"));
        assert!(!file("git:changed", "a.rs", ""));
        let q = compile("foo sort:modified", OPTS).unwrap().unwrap();
        assert!(q.sort == Some(SortBy::Modified) && q.is_match(b"foo", 0));
        // Unknown field names stay text.
        assert!(matches("http:x", "see http:x"));
    }

    #[test]
    fn file_mode() {
        let q = compile(
            "foo bar -baz",
            Options {
                file_mode: true,
                ..OPTS
            },
        )
        .unwrap()
        .unwrap();
        let (foo, bar) = (q.line_terms(b"a foo"), q.line_terms(b"bar b"));
        assert!(q.is_positive(foo) && q.file_match(foo | bar, 0));
        assert!(!q.file_match(foo, 0));
        assert!(!q.file_match(foo | bar | q.line_terms(b"baz"), 0));
        // The prefilter also finds lines with negated terms.
        assert!(q.prefilter.is_match(b"baz").unwrap());
    }

    #[test]
    fn boolean() {
        assert!(matches("foo bar", "bar then foo"));
        assert!(!matches("foo bar", "foo only"));
        assert!(matches("foo OR bar", "bar only"));
        assert!(matches("(foo OR bar) AND baz", "bar baz"));
        assert!(!matches("(foo OR bar) AND baz", "foo bar"));
        assert!(matches("foo -bar", "foo"));
        assert!(!matches("foo -bar", "foo bar"));
        assert!(matches("NOT foo", "anything"));
        assert!(!matches("NOT foo", "a foo"));
        assert!(!matches("foo AND NOT bar", "foo bar"));
    }

    #[test]
    fn case_and_literals() {
        assert!(matches("exact", "EXACT"));
        assert!(matches("exacT", "exacT"));
        assert!(!matches("exacT", "exact"));
        assert!(matches("\"a.b\"", "a.b"));
        assert!(!matches("\"a.b\"", "axb"));
        assert!(matches("a.b", "axb"));
        assert!(matches("/fn\\s+main/", "fn  main"));
        assert!(matches("foo(", "call foo(x)"));
        assert!(matches("C++", "learn C++"));
    }

    #[test]
    fn highlights_merge() {
        let q = compile("foo OR oba OR bar -baz", OPTS).unwrap().unwrap();
        assert_eq!(q.highlights(b"foobar foo"), vec![(0, 6), (7, 10)]);
    }

    #[test]
    fn partial_input() {
        assert!(matches("foo()", "x = foo()"));
        assert!(matches("(foo OR", "a foo"));
        assert!(matches("a b | c", "a c"));
        assert!(!matches("a b | c", "b c"));
        assert!(matches("foo !bar", "foo"));
        assert!(matches("a::b", "use a::b;"));
        assert!(matches("x:y", "x:y"));
    }

    #[test]
    fn path_follows_word_rules() {
        // A word is a regex (smart case), `"..."` is literal, `/re/` is a regex.
        assert!(file("path:src/ui", "SRC/UI/a.rs", ""));
        assert!(!file("path:Src", "src/a.rs", ""));
        assert!(file("path:Src", "Src/a.rs", ""));
        assert!(file("path:^src", "src/a.rs", ""));
        assert!(!file("path:^src", "a/src/a.rs", ""));
        assert!(file("path:a.rs", "axrs", ""));
        assert!(!file("path:\"a.rs\"", "axrs", ""));
        assert!(file("path:\"a.rs\"", "a.rs", ""));
        // Quoted values are not globs.
        assert!(!file("path:\"*.rs\"", "a.rs", ""));
        assert!(file("path:\"*.rs\"", "x*.rs", ""));
        assert!(file("path:/Ui/", "src/Ui/a", ""));
        assert!(!file("path:/Ui/", "src/ui/a", ""));
        // An invalid word is literal; an invalid /regex/ is the only error.
        assert!(file("path:foo(", "a/foo(b", ""));
        assert!(compile("path:/[/", OPTS).is_err());
        // A bad glob while typing is not an error.
        assert!(!file("path:[a", "a", ""));
    }

    #[test]
    fn unknown_field_values_match_nothing_with_a_hint() {
        for q in ["git:c", "type:ru", "sort:p", "path:[a"] {
            let query = compile(q, OPTS).expect(q).unwrap();
            assert!(query.hint.is_some(), "{q}");
            assert!(!query.possible(query.file_fields("a.rs", None)), "{q}");
        }
        assert!(compile("git:changed", OPTS).unwrap().unwrap().hint.is_none());
        assert!(compile("sort:path", OPTS).unwrap().unwrap().hint.is_none());
        // Empty value matches all.
        assert!(file("git: foo", "a.rs", "foo"));
        assert!(file("type: ext:rs", "a.rs", ""));
    }

    #[test]
    fn smart_case_agrees_with_the_parser() {
        for (q, line, want) in [
            ("Foo", "foo", false),
            ("Foo", "Foo", true),
            ("foo", "FOO", true),
            ("\"Foo\"", "foo", false),
            ("\"foo\"", "FOO", true),
            ("/Foo/", "foo", false),
            ("/foo/", "FOO", true),
            ("\\Sx", "AX", true),
        ] {
            assert_eq!(matches(q, line), want, "{q} on {line}");
        }
        let off = Options {
            smart_case: false,
            ..OPTS
        };
        let q = compile("Foo", off).unwrap().unwrap();
        assert!(q.is_match(b"foo", 0));
    }

    #[test]
    fn remap_drops_empty_ranges() {
        assert_eq!(remap(&[(0, 2), (3, 5)], |o| o.saturating_sub(3)), vec![(0, 2)]);
    }
}
