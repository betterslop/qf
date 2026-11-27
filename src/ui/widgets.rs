//! Reusable pieces: centered messages, path display, and text helpers.

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use unicode_width::UnicodeWidthStr;

/// A centered message with an optional hint line below it.
/// Use it for empty states ("type to search", "no matches") and other notices.
pub struct Message<'a> {
    title: Line<'a>,
    hint: Option<Line<'a>>,
}

impl<'a> Message<'a> {
    pub fn new(title: impl Into<Line<'a>>) -> Self {
        Self {
            title: title.into(),
            hint: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<Line<'a>>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn style(mut self, style: Style) -> Self {
        self.title = self.title.patch_style(style);
        self
    }

    /// Style the title by tone, and the hint dim.
    pub fn tone(self, tone: Tone) -> Self {
        let palette = bs_tui_common::theme::palette();
        let style = match tone {
            Tone::Info => palette.info,
            Tone::Warn => palette.warn,
            Tone::Error => palette.error,
        };
        self.style(style).hint_style(palette.dim)
    }

    pub fn hint_style(mut self, style: Style) -> Self {
        self.hint = self.hint.map(|h| h.patch_style(style));
        self
    }
}

impl Widget for Message<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut lines = vec![self.title.centered()];
        if let Some(hint) = self.hint {
            lines.push(hint.centered());
        }
        let [row] = Layout::vertical([Constraint::Length(lines.len() as u16)])
            .flex(Flex::Center)
            .areas(area);
        Paragraph::new(lines).wrap(Wrap { trim: true }).render(row, buf);
    }
}

/// File names: the `accent` role, bold.
pub fn file_style() -> Style {
    bs_tui_common::theme::palette().accent.add_modifier(Modifier::BOLD)
}

/// Path spans: folder in `dir` style, file name in `file` style. If the path is wider
/// than `max`, the middle of the folder part becomes `…`.
pub fn path_spans<'a>(path: &'a str, max: usize, dir: Style, file: Style) -> Vec<Span<'a>> {
    path_spans_hl(path, max, dir, file, &[])
}

/// [`path_spans`] with the byte ranges `hl` of `path` in the `matched` role. The ranges
/// follow the text that is drawn, so a cut-out middle takes its matches with it.
pub fn path_spans_hl(path: &str, max: usize, dir: Style, file: Style, hl: &[Range<usize>]) -> Vec<Span<'static>> {
    let (folder, name) = match path.rfind('/') {
        Some(i) => path.split_at(i + 1),
        None => ("", path),
    };
    // Each drawn piece: its text, the byte offset it has in `path` (none for `…`), its style.
    let mut pieces: Vec<(String, Option<usize>, Style)> = Vec::new();
    if path.width() <= max || folder.is_empty() {
        pieces.push((folder.to_string(), Some(0), dir));
    } else {
        // Keep the start and the end of the folder part.
        let room = max.saturating_sub(name.width() + 1);
        let head_w = room / 2;
        let tail_w = room - head_w;
        let head: String = take_width(folder.chars(), head_w);
        let tail: String = take_width(folder.chars().rev(), tail_w).chars().rev().collect();
        let tail_at = folder.len() - tail.len();
        pieces.push((head, Some(0), dir));
        pieces.push(("…".to_string(), None, dir));
        pieces.push((tail, Some(tail_at), dir));
    }
    pieces.push((name.to_string(), Some(folder.len()), file));
    let mut spans = Vec::new();
    for (text, at, style) in pieces {
        let ranges = match at {
            Some(at) => hl
                .iter()
                .filter(|r| r.start < at + text.len() && r.end > at)
                .map(|r| r.start.max(at) - at..r.end.min(at + text.len()) - at)
                .collect(),
            None => Vec::new(),
        };
        spans.extend(
            bs_tui_common::highlight::spans(&text, style, ranges)
                .into_iter()
                .map(|s| Span::styled(s.content.to_string(), s.style)),
        );
    }
    spans
}

fn take_width(chars: impl Iterator<Item = char>, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0;
    for c in chars {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw > max {
            break;
        }
        w += cw;
        out.push(c);
    }
    out
}

/// The last useful line of a multi-line error, without an `error:` prefix.
/// Regex errors put the reason on the last line.
pub fn short_error(msg: &str) -> String {
    let line = msg.lines().map(str::trim).rfind(|l| !l.is_empty()).unwrap_or(msg);
    line.strip_prefix("error:").unwrap_or(line).trim().to_string()
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[derive(Clone, Copy)]
pub enum Tone {
    Info,
    Warn,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn paths() {
        let s = Style::new();
        assert_eq!(text(&path_spans("src/main.rs", 40, s, s)), "src/main.rs");
        let long = text(&path_spans("a/very/long/folder/path/name/file.rs", 20, s, s));
        assert!(
            long.ends_with("/file.rs") && long.contains('…') && long.width() <= 20,
            "{long}"
        );
    }

    #[test]
    fn path_matches_follow_the_drawn_text() {
        let m = bs_tui_common::theme::palette().matched;
        let s = Style::new();
        let marked = |spans: &[Span]| -> String {
            spans
                .iter()
                .filter(|x| x.style == m)
                .map(|x| x.content.as_ref())
                .collect()
        };
        let spans = path_spans_hl("src/main.rs", 40, s, s, &[Range { start: 4, end: 8 }]);
        assert_eq!(text(&spans), "src/main.rs");
        assert_eq!(marked(&spans), "main");
        // One match is in the cut-out middle, one in the file name: only the name is marked.
        let path = "aaaa/bbbb/cccc/dddd/eeee/file.rs";
        let spans = path_spans_hl(path, 20, s, s, &[10..14, 25..29]);
        assert!(text(&spans).contains('…') && text(&spans).ends_with("/file.rs"));
        assert_eq!(marked(&spans), "file");
    }

    #[test]
    fn errors_and_sizes() {
        assert_eq!(
            short_error("regex parse error:\n    (?:Hit()\n        ^\nerror: unclosed group"),
            "unclosed group"
        );
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(4300), "4.2 KB");
    }
}
