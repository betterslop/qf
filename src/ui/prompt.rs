//! The prompt line (with query syntax colors) and the status bar.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::widgets::short_error;
use crate::app::{App, EditMode};
use crate::query::{Mode, SortBy};
use crate::search::{MAX_HITS, Status};
use crate::syntax::{self, Class};
use crate::theme::Palette;
use bs_tui_common::complete::Popup;
use bs_tui_common::frame::{command_bar, status_bar_fit};
use bs_tui_common::notice::Notice;

/// Draw the prompt. Returns the screen position of the word under the cursor, for
/// the completion menu.
pub fn draw_prompt(f: &mut Frame, app: &mut App, area: Rect) -> Position {
    let style = match app.mode {
        EditMode::Insert => app.palette.insert,
        EditMode::Normal => app.palette.normal,
    };
    let marker = Span::styled("> ", style);
    let [marker_area, text_area] =
        Layout::horizontal([Constraint::Length(marker.width() as u16), Constraint::Fill(1)]).areas(area);
    f.render_widget(Paragraph::new(marker), marker_area);

    // Scroll the text so the cursor stays visible. Keep one column for the cursor.
    let chars = app.input.chars();
    let col_of = |i: usize| chars[..i].iter().collect::<String>().width() as u16;
    let col = col_of(app.input.cursor());
    let avail = text_area.width.saturating_sub(1).max(1);
    if col < app.prompt_offset {
        app.prompt_offset = col;
    } else if col >= app.prompt_offset + avail {
        app.prompt_offset = col - avail + 1;
    }
    let spans = match app.opts.mode {
        Mode::Query => query_spans(chars, app.input.cursor(), app.mode, &app.palette),
        Mode::Regex => vec![Span::raw(app.input.text())],
    };
    f.render_widget(
        Paragraph::new(Line::from(spans)).scroll((0, app.prompt_offset)),
        text_area,
    );
    // The open `:` line draws its own block cursor.
    if app.help.is_none() && !app.cmdline.is_open() {
        f.set_cursor_position((text_area.x + col - app.prompt_offset, area.y));
    }
    let word_start = app.completion.as_ref().map_or(app.input.cursor(), |c| c.start);
    let x = (text_area.x + col_of(word_start))
        .saturating_sub(app.prompt_offset)
        .max(text_area.x);
    Position::new(x, area.y)
}

/// Prompt spans with query syntax colors. Unclosed `(` and quotes are red. The
/// parenthesis at the cursor and its partner are in reverse video (like vim's matchparen).
fn query_spans(chars: &[char], cursor: usize, mode: EditMode, p: &Palette) -> Vec<Span<'static>> {
    let a = syntax::analyze(chars);
    let mut styles: Vec<Style> = a
        .classes
        .iter()
        .map(|c| match c {
            Class::Plain => Style::new(),
            Class::Op => p.op,
            Class::Negate => p.negate,
            Class::Quoted => p.quoted,
            Class::Regex => p.regex,
            Class::Paren => p.paren,
        })
        .collect();
    for &(i, _) in &a.errors {
        styles[i] = bs_tui_common::theme::palette()
            .error
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    }
    // Normal mode: the char under the cursor. Insert mode: also the char before it.
    let at = [Some(cursor), (mode == EditMode::Insert).then(|| cursor.wrapping_sub(1))];
    if let Some((i, j)) = at.into_iter().flatten().find_map(|i| a.partner(i).map(|j| (i, j))) {
        styles[i] = styles[i].add_modifier(Modifier::REVERSED);
        styles[j] = styles[j].add_modifier(Modifier::REVERSED);
    }
    // Group runs of the same style.
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let j = (i..chars.len())
            .find(|&j| styles[j] != styles[i])
            .unwrap_or(chars.len());
        spans.push(Span::styled(chars[i..j].iter().collect::<String>(), styles[i]));
        i = j;
    }
    spans
}

/// The query completion menu: next to the word under the cursor, above a bottom
/// prompt or below a top prompt.
pub fn draw_menu(f: &mut Frame, app: &App, anchor: Position) {
    if let Some(c) = &app.completion {
        let popup = Popup::new(c, anchor, app.bottom).style(app.palette.preview_bg);
        f.render_widget(popup, f.area());
    }
}

/// The open `:` command line, in place of the status bar, with its menu above it.
pub fn draw_cmdline(f: &mut Frame, app: &App, area: Rect) {
    let (line, col) = command_bar(&app.cmdline);
    f.render_widget(Paragraph::new(line).style(bs_tui_common::theme::palette().bar), area);
    if let Some(c) = app.cmdline.completion() {
        let anchor = Position::new(area.x + col, area.y);
        f.render_widget(Popup::new(c, anchor, true).style(app.palette.preview_bg), f.area());
    }
}

/// The notice for the status line: the last command's result, else the search
/// error, the query hint or the syntax problem.
fn status_notice(app: &App) -> Option<Notice> {
    let notice = if let Some(n) = &app.message {
        n.clone()
    } else if let Some(err) = &app.error {
        Notice::error(short_error(err))
    } else if let Some(hint) = app.active.as_ref().and_then(|q| q.hint.as_deref()) {
        Notice::info(short_error(hint))
    } else if app.opts.mode == Mode::Query {
        let chars = app.input.chars();
        Notice::warn(syntax::analyze(chars).problem(chars)?)
    } else {
        return None;
    };
    Some(notice)
}

/// The status bar, the last row of the screen: the notice, qf's state, then the hints.
pub fn draw_status(f: &mut Frame, app: &App, area: Rect, status: &Status) {
    let pal = bs_tui_common::theme::palette();
    let notice = status_notice(app);
    // qf's state parts go between the notice and the help hint (LOOK.md).
    let mut state: Vec<Vec<Span<'static>>> = Vec::new();
    let n = app.hits.len();
    let mut count = Vec::new();
    if app.searching {
        count.push(Span::styled(format!("{} ", bs_tui_common::icons::spinner()), pal.warn));
    }
    // The start screen (empty query, no hits) needs no hit count.
    if n > 0 || !app.input.is_empty() {
        count.push(Span::raw(format!("{n}{} hits", if n >= MAX_HITS { "+" } else { "" })));
    }
    state.push(count);
    if app.searching {
        let total = status.total.map(|t| format!("/{t}")).unwrap_or_default();
        state.push(vec![Span::raw(format!("searched {}{total} files", status.searched))]);
    } else if let Some(s) = &app.stats {
        let text = format!(
            "{}/{} files · {} ms",
            s.files_matched,
            s.files_searched,
            s.elapsed.as_millis()
        );
        state.push(vec![Span::raw(text)]);
    }
    if !app.marks.is_empty() {
        state.push(vec![Span::styled(
            format!("{} marked", app.marks.len()),
            app.palette.marked,
        )]);
    }
    let regex = app.opts.mode == Mode::Regex;
    let flags = [
        (app.mode == EditMode::Normal, "normal"),
        (regex, "regex"),
        (regex && app.opts.ivy, "ivy"),
        (!app.opts.smart_case, "ignore-case"),
        (app.opts.file_mode, "file"),
        (app.opts.changed, "changed"),
        (app.sort_order() == SortBy::Path, "sort:path"),
        (app.sort_order() == SortBy::Modified, "sort:modified"),
    ];
    for (_, name) in flags.iter().filter(|(on, _)| *on) {
        state.push(vec![Span::raw(format!("[{name}]"))]);
    }
    if app.opts.context > 0 {
        state.push(vec![Span::raw(format!("[context {}]", app.opts.context))]);
    }
    let help = if app.mode == EditMode::Insert {
        "F1 help"
    } else {
        "? help"
    };
    let line = status_bar_fit(area.width, notice.as_ref(), &[], state, &[], help);
    f.render_widget(Paragraph::new(line).style(pal.bar), area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_colors() {
        let p = Palette::plain();
        let chars: Vec<char> = "(foo OR \"a b\") -bar /x y/".chars().collect();
        let spans = query_spans(&chars, 99, EditMode::Normal, &p);
        let parts: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            parts,
            ["(", "foo ", "OR", " ", "\"a b\"", ")", " ", "-", "bar ", "/x y/"]
        );
        assert_eq!(spans[2].style, p.op);
        assert_eq!(spans[7].style, p.negate);

        // Matchparen: cursor on `(` marks both parentheses.
        let spans = query_spans(&chars, 0, EditMode::Normal, &p);
        assert!(spans[0].style.add_modifier.contains(Modifier::REVERSED));
    }
}
