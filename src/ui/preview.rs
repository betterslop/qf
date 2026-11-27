//! The preview pane.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use unicode_width::UnicodeWidthChar;

use super::widgets::{file_style, human_size, path_spans_hl};
use crate::app::App;
use crate::preview::{self, Request};
use crate::query;
use crate::search::MAX_HITS;
use bs_tui_common::frame::pane;

/// The preview pane: a bordered pane titled ` path:line `, with the match count,
/// language and size on the right.
pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let p = app.palette.clone();
    let shared = bs_tui_common::theme::palette();
    let block = Block::bordered();
    let body = block.inner(area);
    let Some((path, line)) = app.hits.get(app.selected).map(|h| (h.path.clone(), h.line)) else {
        f.render_widget(pane(" Preview ").style(p.preview_bg), area);
        return;
    };
    let h = body.height as usize;

    // Center the hit, then apply the user's scroll. Keep at least one line on screen.
    let target = line.saturating_sub(1) as usize;
    let base = target as isize - (h / 2) as isize;
    let mut start = (base + app.preview_scroll).max(0);
    if let Some(total) = app.preview.as_ref().filter(|r| r.path == path).map(|r| r.total) {
        start = start.min(total.saturating_sub(1) as isize);
    }
    app.preview_scroll = start - base;
    if h > 0 {
        app.request_preview(Request {
            path: path.clone(),
            start: start as usize,
            count: h,
        });
    }
    let resp = app.preview.as_ref().filter(|r| r.path == path);

    // Title: ` path:line ` on the left; match count, language and size on the right.
    let matches = app.hits.iter().filter(|hit| hit.path == path).count();
    let plus = if app.hits.len() >= MAX_HITS { "+" } else { "" };
    let mut info = format!("{matches}{plus} match{}", if matches == 1 { "" } else { "es" });
    if let Some(r) = resp {
        info.push_str(&format!(" · {} · {}", r.syntax, human_size(r.size)));
    }
    if app.hscroll > 0 {
        info.push_str(&format!(" · col +{}", app.hscroll));
    }
    // Both titles share the top border: the info takes at most half of it.
    let info: String = info.chars().take((area.width / 2).saturating_sub(4) as usize).collect();
    let info_w = if info.is_empty() { 0 } else { info.chars().count() + 2 };
    let line_s = line.to_string();
    let path_max = (area.width as usize).saturating_sub(info_w + line_s.len() + 5).max(8);
    let hl = app.path_ranges(&path);
    let mut title = vec![Span::raw(" ")];
    title.extend(path_spans_hl(&path, path_max, shared.dim, file_style(), &hl));
    title.push(Span::raw(":"));
    title.push(Span::styled(line_s, shared.info));
    title.push(Span::raw(" "));
    let mut block = pane(Line::from(title)).style(p.preview_bg);
    if info_w > 0 {
        block = block.title_top(Line::from(vec![Span::styled(format!(" {info} "), shared.dim)]).right_aligned());
    }
    f.render_widget(block, area);

    // Show the newest response for this file. Its line numbers are correct even if
    // the requested window has not arrived yet.
    let Some(resp) = resp else { return };
    if h == 0 {
        return;
    }
    let show_bar = app.scrollbar && resp.total > h;
    let body = Rect {
        width: body.width.saturating_sub(show_bar as u16),
        ..body
    };
    let num_w = (resp.start + resp.lines.len()).to_string().len();
    let [gutter_area, text_area] =
        Layout::horizontal([Constraint::Length(num_w as u16 + 1), Constraint::Fill(1)]).areas(body);
    let width = text_area.width as usize;

    let mut gutter: Vec<Line> = Vec::with_capacity(h);
    let mut text: Vec<Line> = Vec::with_capacity(h);
    let mut target_rows = None;
    for (i, l) in resp.lines.iter().enumerate() {
        if text.len() >= h {
            break;
        }
        // Match the line as it is in the file, then map onto the drawn (tab-expanded) text.
        let ranges = match (app.active.as_ref(), resp.raw.get(i)) {
            (Some(q), Some(raw)) => query::remap(&q.highlights(raw.as_bytes()), |o| preview::drawn_offset(raw, o)),
            _ => Vec::new(),
        };
        let marked = mark(l, &ranges, p.preview_match);
        let parts = if app.wrap { wrap(marked, width) } else { vec![marked] };
        let first_row = text.len();
        for (k, part) in parts.into_iter().enumerate() {
            let num = if k == 0 {
                format!("{:>num_w$}", resp.start + i + 1)
            } else {
                String::new()
            };
            gutter.push(Line::styled(num, p.gutter));
            text.push(part);
        }
        if resp.start + i == target {
            target_rows = Some((first_row, text.len().min(h)));
        }
    }

    // Paint the hit rows first. The text keeps this background, except where a
    // match sets its own.
    if let Some((from, to)) = target_rows {
        let rows = Rect {
            y: body.y + from as u16,
            height: (to - from) as u16,
            ..body
        };
        f.buffer_mut().set_style(rows, p.current_line);
    }
    f.render_widget(Paragraph::new(gutter), gutter_area);
    let hscroll = if app.wrap { 0 } else { app.hscroll };
    f.render_widget(Paragraph::new(text).scroll((0, hscroll)), text_area);

    if show_bar {
        let max = resp.total - h;
        let mut state = ScrollbarState::new(max + 1)
            .position((start as usize).min(max))
            .viewport_content_length(h);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .style(shared.dim);
        let bar_area = Rect {
            x: body.right(),
            width: 1,
            ..body
        };
        f.render_stateful_widget(bar, bar_area, &mut state);
    }
}

/// Apply `style` to the byte ranges `ranges` of the line text. Splits spans as needed.
fn mark(line: &Line<'static>, ranges: &[(usize, usize)], style: Style) -> Line<'static> {
    if ranges.is_empty() {
        return line.clone();
    }
    let mut out = Vec::with_capacity(line.spans.len() + ranges.len() * 2);
    let mut pos = 0;
    for span in &line.spans {
        let text = span.content.as_ref();
        let end = pos + text.len();
        let mut cuts: Vec<usize> = ranges
            .iter()
            .flat_map(|&(s, e)| [s, e])
            .filter(|&p| p > pos && p < end)
            .collect();
        cuts.push(end);
        cuts.sort_unstable();
        cuts.dedup();
        let mut at = pos;
        for cut in cuts {
            let inside = ranges.iter().any(|&(s, e)| s <= at && at < e);
            let seg_style = if inside { span.style.patch(style) } else { span.style };
            out.push(Span::styled(text[at - pos..cut - pos].to_string(), seg_style));
            at = cut;
        }
        pos = end;
    }
    Line::from(out)
}

/// Split a line into rows of at most `width` columns.
fn wrap(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line];
    }
    let mut rows = Vec::new();
    let mut row: Vec<Span<'static>> = Vec::new();
    let mut col = 0;
    for span in line.spans {
        let mut buf = String::new();
        for c in span.content.chars() {
            let w = c.width().unwrap_or(0);
            if col + w > width && col > 0 {
                if !buf.is_empty() {
                    row.push(Span::styled(std::mem::take(&mut buf), span.style));
                }
                rows.push(Line::from(std::mem::take(&mut row)));
                col = 0;
            }
            buf.push(c);
            col += w;
        }
        if !buf.is_empty() {
            row.push(Span::styled(buf, span.style));
        }
    }
    rows.push(Line::from(row));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn wrap_rows() {
        let line = Line::from(vec![Span::raw("abcd"), Span::raw("efghij")]);
        assert_eq!(texts(&wrap(line, 4)), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn mark_splits_spans() {
        let hl = Style::new().bg(Color::Yellow);
        let line = Line::from(vec![Span::raw("foo"), Span::raw("bar")]);
        let marked = mark(&line, &[(2, 4)], hl);
        let parts: Vec<(&str, bool)> = marked
            .spans
            .iter()
            .map(|s| (s.content.as_ref(), s.style.bg == hl.bg))
            .collect();
        assert_eq!(parts, [("fo", false), ("o", true), ("b", true), ("ar", false)]);
    }
}
