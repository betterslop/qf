//! The results list.
//!
//! Rows have a logical order that starts at the prompt. With the prompt at the top,
//! row 0 is at the top of the list. With the prompt at the bottom (the default, like
//! `sk` and `fzf`), row 0 is at the bottom and rows go up. A file header always sits
//! on the screen above the hits of its file. Context lines sit above and below
//! their hit, once per file.

use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use super::widgets::{Message, Tone, file_style, path_spans_hl, short_error};
use crate::app::App;
use crate::search::{Hit, Status};

#[derive(Clone, Copy)]
enum Row {
    /// File header. The value is the first hit of the file.
    Header(usize),
    Hit(usize),
    /// Context line `k` before (`false`) or after (`true`) hit `i`.
    Context(usize, bool, usize),
}

impl Row {
    fn hit(self) -> usize {
        match self {
            Row::Header(i) | Row::Hit(i) | Row::Context(i, _, _) => i,
        }
    }
}

/// Rows in logical order (from the prompt outward).
fn rows(app: &App) -> Vec<Row> {
    let hits = &app.hits;
    let mut rows = Vec::with_capacity(hits.len() + 64);
    let mut start = 0;
    while start < hits.len() {
        let end = start + hits[start..].iter().take_while(|h| h.path == hits[start].path).count();
        // On the screen the header is above its hits. With rows going up, that is
        // after the hits in logical order.
        if app.group && !app.bottom {
            rows.push(Row::Header(start));
        }
        // Show each context line once per file, and not when it is also a hit.
        let mut shown: HashSet<u64> = hits[start..end].iter().map(|h| h.line).collect();
        for (i, hit) in hits.iter().enumerate().take(end).skip(start) {
            let mut block = Vec::with_capacity(1 + hit.before.len() + hit.after.len());
            for (k, (line, _)) in hit.before.iter().enumerate() {
                if shown.insert(*line) {
                    block.push(Row::Context(i, false, k));
                }
            }
            block.push(Row::Hit(i));
            for (k, (line, _)) in hit.after.iter().enumerate() {
                if shown.insert(*line) {
                    block.push(Row::Context(i, true, k));
                }
            }
            // `block` is in screen order. Rows going up need it reversed.
            if app.bottom {
                block.reverse();
            }
            rows.extend(block);
        }
        if app.group && app.bottom {
            rows.push(Row::Header(start));
        }
        start = end;
    }
    rows
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect, status: &Status) {
    if app.hits.is_empty() {
        app.list_rows.clear();
        draw_empty(f, app, area, status);
        return;
    }
    let h = area.height as usize;
    app.page = h.max(1);
    let rows = rows(app);
    let sel = rows
        .iter()
        .position(|r| matches!(r, Row::Hit(i) if *i == app.selected))
        .unwrap_or(0);

    // `app.scroll` is the first visible logical row. Keep the selection visible, and
    // the header of its file when the header is the next row at either edge.
    if sel < app.scroll {
        app.scroll = sel;
    } else if sel >= app.scroll + h {
        app.scroll = sel + 1 - h;
    }
    if h > 1 && app.scroll == sel && sel > 0 && matches!(rows[sel - 1], Row::Header(_)) {
        app.scroll -= 1;
    }
    if h > 1 && sel + 1 == app.scroll + h && matches!(rows.get(sel + 1), Some(Row::Header(_))) {
        app.scroll += 1;
    }
    app.scroll = app.scroll.min(rows.len().saturating_sub(h));

    let shared = bs_tui_common::theme::palette();
    let show_bar = app.scrollbar && rows.len() > h;
    // The marker column sits left of the text, so it never covers a character.
    let gutter = app.palette.marker as u16;
    let text_area = Rect {
        x: area.x + gutter,
        width: area.width.saturating_sub(show_bar as u16 + gutter),
        ..area
    };
    let visible = &rows[app.scroll..(app.scroll + h).min(rows.len())];
    let num_w = visible
        .iter()
        .filter_map(|&r| row_line(app, r))
        .map(|line| line.to_string().len())
        .max()
        .unwrap_or(1);

    let mut lines: Vec<Line> = visible
        .iter()
        .map(|&row| match row {
            Row::Header(i) => header_line(app, i, text_area.width as usize),
            Row::Hit(i) => hit_line(app, i, num_w, text_area.width as usize),
            Row::Context(i, after, k) => context_line(app, i, after, k, num_w, text_area.width as usize),
        })
        .collect();
    let mut hit_rows: Vec<Option<usize>> = visible.iter().map(|r| Some(r.hit())).collect();
    let sel_y = sel - app.scroll;
    let sel_y = if app.bottom {
        // Rows go up from the bottom edge.
        lines.reverse();
        hit_rows.reverse();
        let pad = h - lines.len();
        lines.splice(0..0, std::iter::repeat_n(Line::default(), pad));
        hit_rows.splice(0..0, std::iter::repeat_n(None, pad));
        h - 1 - sel_y
    } else {
        sel_y
    };
    f.render_widget(Paragraph::new(lines), text_area);
    app.list_rows = hit_rows;

    let row = Rect {
        y: area.y + sel_y as u16,
        height: 1,
        ..text_area
    };
    // The shared `selected` row style and `marker` bar. Matches keep their own style
    // on top of it.
    let matched = shared.matched;
    let buf = f.buffer_mut();
    let kept: Vec<(u16, Style)> = (row.x..row.right())
        .map(|x| (x, buf[(x, row.y)].style()))
        .filter(|(_, st)| is_matched(st, matched))
        .collect();
    buf.set_style(row, shared.selected);
    for (x, st) in kept {
        buf[(x, row.y)].set_style(st);
    }
    if app.palette.marker {
        let marker = shared.selected.patch(shared.marker);
        f.buffer_mut()
            .set_string(area.x, row.y, bs_tui_common::theme::MARKER, marker);
    }

    if show_bar {
        // The scrollbar position counts from the top of the screen.
        let max = rows.len() - h;
        let pos = if app.bottom { max - app.scroll } else { app.scroll };
        let mut state = ScrollbarState::new(max + 1).position(pos).viewport_content_length(h);
        let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .style(shared.dim);
        f.render_stateful_widget(bar, area, &mut state);
    }
}

/// True for a cell style that has everything the `matched` role sets.
fn is_matched(st: &Style, matched: Style) -> bool {
    matched.fg.is_none_or(|c| st.fg == Some(c))
        && matched.bg.is_none_or(|c| st.bg == Some(c))
        && st.add_modifier.contains(matched.add_modifier)
        && (matched.fg.is_some() || matched.bg.is_some() || !matched.add_modifier.is_empty())
}

/// The spinner in `warn`, then the text.
fn busy(text: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(bs_tui_common::icons::spinner(), bs_tui_common::theme::palette().warn),
        Span::raw(format!(" {text}")),
    ])
}

const HINT: &str = "@ files · : commands · C-Space complete · ? help";

fn draw_empty(f: &mut Frame, app: &App, area: Rect, status: &Status) {
    // Start screen: the logo while the query is empty.
    if app.input.is_empty() && app.error.is_none() && !app.searching {
        let shared = bs_tui_common::theme::palette();
        let line = if status.index_ready {
            Line::styled("Type to search", shared.info)
        } else {
            busy(format!("Indexing… {} files", status.indexed)).patch_style(shared.info)
        };
        let hint = Line::styled(HINT, shared.dim);
        if super::splash::draw(f, area, line, hint) {
            return;
        }
    }
    let (msg, tone) = if let Some(err) = &app.error {
        (
            Message::new(format!("✗ {}", short_error(err))).hint("Fix the query to search again"),
            Tone::Error,
        )
    } else if app.searching {
        (Message::new(busy("Searching…".into())), Tone::Info)
    } else if !status.index_ready && app.input.is_empty() {
        (
            Message::new(busy(format!("Indexing… {} files", status.indexed))),
            Tone::Info,
        )
    } else if app.input.is_empty() {
        (Message::new("Type to search").hint(HINT), Tone::Info)
    } else {
        (
            Message::new("No matches").hint(":case ignore or :mode regex can help"),
            Tone::Warn,
        )
    };
    f.render_widget(msg.tone(tone), area);
}

/// The file line number of a hit or context row.
fn row_line(app: &App, row: Row) -> Option<u64> {
    match row {
        Row::Header(_) => None,
        Row::Hit(i) => Some(app.hits[i].line),
        Row::Context(i, after, k) => {
            let hit = &app.hits[i];
            Some(if after { hit.after[k].0 } else { hit.before[k].0 })
        }
    }
}

/// A dim context line: `     12- text`, like `rg -C`.
fn context_line(app: &App, i: usize, after: bool, k: usize, num_w: usize, width: usize) -> Line<'_> {
    let dim = bs_tui_common::theme::palette().dim;
    let hit = &app.hits[i];
    let (line, text) = if after { &hit.after[k] } else { &hit.before[k] };
    let mut spans = vec![Span::raw("  ")];
    if app.group {
        spans.push(Span::styled(format!(" {line:>num_w$}"), dim));
    } else {
        spans.extend(path_spans_hl(
            &hit.path,
            width / 2,
            dim,
            dim,
            &app.path_ranges(&hit.path),
        ));
        spans.push(Span::styled(format!("-{line}"), dim));
    }
    spans.push(Span::styled(format!("- {text}"), dim));
    Line::from(spans)
}

fn header_line(app: &App, first: usize, width: usize) -> Line<'_> {
    let shared = bs_tui_common::theme::palette();
    let path = &app.hits[first].path;
    let count = app.hits[first..].iter().take_while(|h| h.path == *path).count();
    let count = format!(" ({count})");
    let hl = app.path_ranges(path);
    let mut spans = path_spans_hl(
        path,
        width.saturating_sub(count.len() + 1),
        shared.dim,
        file_style(),
        &hl,
    );
    spans.insert(0, Span::raw(" "));
    spans.push(Span::styled(count, shared.dim));
    Line::from(spans)
}

/// `▌+ src/main.rs:12: text` (flat) or `▌+   12: text` (grouped, numbers right-aligned).
fn hit_line(app: &App, i: usize, num_w: usize, width: usize) -> Line<'_> {
    let p = &app.palette;
    let shared = bs_tui_common::theme::palette();
    let hit: &Hit = &app.hits[i];
    let mark = if app.is_marked(hit) {
        Span::styled("+", p.marked)
    } else {
        Span::raw(" ")
    };
    let mut spans = vec![Span::raw(" "), mark];
    if app.group {
        spans.push(Span::styled(format!(" {:>num_w$}", hit.line), shared.info));
    } else {
        let hl = app.path_ranges(&hit.path);
        spans.extend(path_spans_hl(&hit.path, width / 2, shared.dim, file_style(), &hl));
        spans.push(Span::raw(":"));
        spans.push(Span::styled(hit.line.to_string(), shared.info));
    }
    spans.push(Span::raw(": "));
    let mut at = 0;
    for &(s, e) in &hit.hl {
        spans.push(Span::raw(&hit.text[at..s]));
        spans.push(Span::styled(&hit.text[s..e], shared.matched));
        at = e;
    }
    spans.push(Span::raw(&hit.text[at..]));
    Line::from(spans)
}
