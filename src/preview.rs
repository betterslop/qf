//! File preview with syntax highlighting, on a worker thread.
//!
//! The UI sends a `Request` for a line window. The worker drops stale requests,
//! highlights only the lines it needs, and caches the result per file, so moving
//! between hits in one file does not parse it again.

use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::SystemTime;

use crossbeam_channel::{Sender, unbounded};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::highlighting::{FontStyle, HighlightIterator, HighlightState, Highlighter, Theme};
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

use crate::AppEvent;
use crate::theme;

const CACHE_SIZE: usize = 16;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Stop highlighting after this line. Later lines render as plain text.
const MAX_HL_LINES: usize = 20_000;
/// Lines longer than this (minified files) stop highlighting for the rest of the file.
const MAX_HL_LINE_LEN: usize = 2_000;
/// syntect's bundled grammars plus `assets/syntaxes`, compiled by `build.rs`.
const SYNTAXES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/syntaxes.packdump"));

#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    pub path: Arc<str>,
    pub start: usize,
    pub count: usize,
}

pub struct Response {
    pub path: Arc<str>,
    /// First line of `lines`, 0-based.
    pub start: usize,
    pub lines: Vec<Line<'static>>,
    /// The same lines as in the file (tabs not expanded). Queries match these.
    pub raw: Vec<String>,
    /// Number of lines in the file.
    pub total: usize,
    /// Grammar name, for example "Rust" or "Plain Text".
    pub syntax: String,
    /// File size in bytes.
    pub size: u64,
}

/// Messages to the preview worker.
pub enum Msg {
    Window(Request),
    /// Switch the theme. Cached highlighting is dropped.
    Theme(theme::ThemeName),
}

/// Start the preview worker. Responses arrive as `AppEvent::Preview`.
/// `color`: false turns off syntax colors (`NO_COLOR`).
pub fn spawn(events: Sender<AppEvent>, color: bool, theme: theme::ThemeName) -> Sender<Msg> {
    let (tx, rx) = unbounded::<Msg>();
    thread::spawn(move || {
        let mut previewer = Previewer::new(color, theme);
        while let Ok(first) = rx.recv() {
            // Apply every theme change. Only the newest window request matters.
            let mut req = None;
            for msg in std::iter::once(first).chain(rx.try_iter()) {
                match msg {
                    Msg::Theme(name) => {
                        previewer.theme = theme::load(name);
                        previewer.docs.clear();
                    }
                    Msg::Window(r) => req = Some(r),
                }
            }
            if let Some(req) = req
                && events.send(AppEvent::Preview(previewer.window(req))).is_err()
            {
                break;
            }
        }
    });
    tx
}

fn load_syntaxes() -> SyntaxSet {
    syntect::dumps::from_binary(SYNTAXES)
}

struct Previewer {
    syntaxes: SyntaxSet,
    theme: Theme,
    color: bool,
    docs: VecDeque<Doc>,
}

impl Previewer {
    fn new(color: bool, name: theme::ThemeName) -> Self {
        Self {
            syntaxes: load_syntaxes(),
            theme: theme::load(name),
            color,
            docs: VecDeque::new(),
        }
    }

    fn window(&mut self, req: Request) -> Response {
        // Drop the cached copy if the file changed on disk.
        let modified = modified(&req.path);
        if let Some(i) = self
            .docs
            .iter()
            .position(|d| d.path == *req.path && d.modified != modified)
        {
            self.docs.remove(i);
        }
        match self.docs.iter().position(|d| d.path == *req.path) {
            Some(0) => {}
            Some(i) => {
                let doc = self.docs.remove(i).expect("index is in range");
                self.docs.push_front(doc);
            }
            None => {
                self.docs
                    .push_front(Doc::load(&req.path, &self.syntaxes, &self.theme, self.color));
                self.docs.truncate(CACHE_SIZE);
            }
        }
        let doc = &mut self.docs[0];
        let total = doc.lines.len();
        let start = req.start.min(total);
        let end = (start + req.count).min(total);
        doc.highlight_to(end, &self.syntaxes, &self.theme);
        Response {
            path: req.path,
            start,
            lines: doc.styled[start..end].to_vec(),
            raw: doc.lines[start..end].to_vec(),
            total,
            syntax: doc.syntax.clone(),
            size: doc.size,
        }
    }
}

fn modified(path: &str) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

struct Doc {
    path: String,
    modified: Option<SystemTime>,
    size: u64,
    syntax: String,
    lines: Vec<String>,
    styled: Vec<Line<'static>>,
    parse: Option<ParseState>,
    highlight: HighlightState,
}

impl Doc {
    fn load(path: &str, syntaxes: &SyntaxSet, theme: &Theme, color: bool) -> Self {
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let (lines, readable) = match fs::metadata(path) {
            Ok(m) if m.len() > MAX_FILE_BYTES => (vec!["[file too large to preview]".into()], false),
            Ok(_) => match fs::read(path) {
                Ok(bytes) => (
                    String::from_utf8_lossy(&bytes).lines().map(String::from).collect(),
                    true,
                ),
                Err(e) => (vec![format!("[{e}]")], false),
            },
            Err(e) => (vec![format!("[{e}]")], false),
        };

        let p = Path::new(path);
        let syntax = readable
            .then(|| {
                p.extension()
                    .or_else(|| p.file_name())
                    .and_then(|s| s.to_str())
                    .and_then(|s| syntaxes.find_syntax_by_extension(s))
                    .or_else(|| lines.first().and_then(|l| syntaxes.find_syntax_by_first_line(l)))
            })
            .flatten();
        let parse = syntax.filter(|_| color).map(ParseState::new);

        Self {
            path: path.to_string(),
            modified: modified(path),
            size,
            syntax: syntax.map_or_else(|| "Plain Text".to_string(), |s| s.name.clone()),
            lines,
            styled: Vec::new(),
            parse,
            highlight: HighlightState::new(&Highlighter::new(theme), ScopeStack::new()),
        }
    }

    fn highlight_to(&mut self, end: usize, syntaxes: &SyntaxSet, theme: &Theme) {
        if self.styled.len() >= end {
            return;
        }
        let highlighter = Highlighter::new(theme);
        while self.styled.len() < end {
            let i = self.styled.len();
            let text = &self.lines[i];
            if i >= MAX_HL_LINES || text.len() > MAX_HL_LINE_LEN {
                self.parse = None;
            }
            let line = match self.parse.as_mut() {
                Some(ps) => {
                    let src = format!("{text}\n");
                    match ps.parse_line(&src, syntaxes) {
                        Ok(ops) => Line::from(
                            HighlightIterator::new(&mut self.highlight, &ops, &src, &highlighter)
                                .filter_map(|(style, s)| {
                                    let s = s.trim_end_matches('\n');
                                    (!s.is_empty()).then(|| Span::styled(s.replace('\t', TAB), to_style(style)))
                                })
                                .collect::<Vec<_>>(),
                        ),
                        Err(_) => {
                            self.parse = None;
                            plain(text)
                        }
                    }
                }
                None => plain(text),
            };
            self.styled.push(line);
        }
    }
}

const TAB: &str = "    ";

fn plain(text: &str) -> Line<'static> {
    Line::raw(text.replace('\t', TAB))
}

/// Byte offset in the drawn line (tabs expanded) for byte offset `at` in the file line `raw`.
pub fn drawn_offset(raw: &str, at: usize) -> usize {
    at + (TAB.len() - 1) * raw[..at].matches('\t').count()
}

fn to_style(s: syntect::highlighting::Style) -> Style {
    let mut style = Style::new().fg(Color::Rgb(s.foreground.r, s.foreground.g, s.foreground.b));
    if s.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    if s.font_style.contains(FontStyle::ITALIC) {
        style = style.add_modifier(Modifier::ITALIC);
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawn_offsets_expand_tabs() {
        assert_eq!(drawn_offset("a\tb", 0), 0);
        assert_eq!(drawn_offset("a\tb", 1), 1);
        assert_eq!(drawn_offset("a\tb", 2), 5);
        assert_eq!(drawn_offset("a\tb", 3), 6);
        assert_eq!(plain("a\tb").spans[0].content, "a    b");
    }

    /// Each extra grammar loads, matches its file name, and parses a line.
    #[test]
    fn extra_grammars() {
        let set = load_syntaxes();
        for (file, line) in [
            ("a.ts", "const x: number = 1;"),
            ("a.tsx", "const el = <div className=\"a\">{x}</div>;"),
            ("Cargo.toml", "[package]\nname = \"qf\""),
            ("Dockerfile", "RUN apt-get update && apt-get install -y curl"),
            ("a.swift", "let x: Int = 1"),
            ("a.kt", "fun main() { println(\"hi\") }"),
            ("a.nix", "{ pkgs ? import <nixpkgs> {} }: pkgs.hello"),
            ("main.tf", "resource \"aws_s3_bucket\" \"b\" { bucket = \"x\" }"),
            ("a.zig", "pub fn main() void {}"),
            ("a.rs", "fn main() {}"),
        ] {
            let p = Path::new(file);
            let key = p
                .extension()
                .or_else(|| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap();
            let syntax = set
                .find_syntax_by_extension(key)
                .unwrap_or_else(|| panic!("no grammar for {file}"));
            let mut state = ParseState::new(syntax);
            for l in line.lines() {
                let ops = state
                    .parse_line(&format!("{l}\n"), &set)
                    .unwrap_or_else(|e| panic!("{file}: {e}"));
                assert!(!ops.is_empty(), "{file}: no scopes");
            }
        }
    }
}
