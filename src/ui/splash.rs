//! Start screen: the logo in the empty results pane, like a Neovim dashboard.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

/// "qf" (Quick Find) in the ANSI Shadow figlet font.
const LOGO: [&str; 6] = [
    " ██████╗ ███████╗",
    "██╔═══██╗██╔════╝",
    "██║   ██║█████╗  ",
    "██║▄▄ ██║██╔══╝  ",
    "╚██████╔╝██║     ",
    " ╚══▀▀═╝ ╚═╝     ",
];

/// Draw the logo with `status` and `hint` below it. Returns false if `area` is too
/// small; the caller then shows a plain message.
pub fn draw(f: &mut Frame, area: Rect, status: Line, hint: Line) -> bool {
    let logo_w = LOGO[0].width() as u16;
    let tagline = format!("quick find · v{}", env!("CARGO_PKG_VERSION"));
    let height = LOGO.len() as u16 + 5;
    if area.height < height || area.width < logo_w.max(hint.width() as u16) {
        return false;
    }

    let mut lines: Vec<Line> = LOGO
        .iter()
        .map(|text| Line::styled(*text, super::widgets::file_style()).centered())
        .collect();
    lines.push(Line::default());
    lines.push(Line::styled(tagline, bs_tui_common::theme::palette().dim).centered());
    lines.push(Line::default());
    lines.push(status.centered());
    lines.push(hint.centered());

    let [rect] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    f.render_widget(Paragraph::new(lines), rect);
    true
}
