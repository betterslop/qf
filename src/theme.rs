//! The embedded syntax themes, and the UI palette built from them.

use std::io::Cursor;

use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;
use syntect::highlighting::{Theme, ThemeSet};

/// Embedded themes (MIT, see assets/themes/LICENSE-monokai-extended).
const DARK: &[u8] = include_bytes!("../assets/themes/Monokai Extended Bright.tmTheme");
const LIGHT: &[u8] = include_bytes!("../assets/themes/Monokai Extended Light.tmTheme");
const FALLBACK_SELECTION: Color = Color::Rgb(58, 58, 74);

/// Pick the theme that matches the terminal background.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    Dark,
    Light,
}

/// The `theme` config value. `Auto` asks the terminal at startup.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    Auto,
    Dark,
    Light,
}

impl ThemeChoice {
    /// The background this setting asks for. `Auto` leaves it to the terminal.
    pub fn background(self) -> Option<bs_tui_common::theme::Background> {
        match self {
            ThemeChoice::Dark => Some(bs_tui_common::theme::Background::Dark),
            ThemeChoice::Light => Some(bs_tui_common::theme::Background::Light),
            ThemeChoice::Auto => None,
        }
    }
}

pub fn load(name: ThemeName) -> Theme {
    let data = match name {
        ThemeName::Dark => DARK,
        ThemeName::Light => LIGHT,
    };
    ThemeSet::load_from_reader(&mut Cursor::new(data)).expect("embedded theme is valid")
}

pub fn rgb(c: syntect::highlighting::Color) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

/// The styles qf adds to the shared roles: the syntax theme for the preview and
/// the selection, and styles built from the shared roles (`bs_tui_common::theme`).
#[derive(Clone)]
pub struct Palette {
    /// Show a `▌` marker on the selected row.
    pub marker: bool,
    /// The hit line in the preview.
    pub current_line: Style,
    /// Matches in the preview.
    pub preview_match: Style,
    pub preview_bg: Style,
    pub gutter: Style,
    pub marked: Style,
    pub insert: Style,
    pub normal: Style,
    /// Query syntax in the prompt.
    pub op: Style,
    pub negate: Style,
    pub quoted: Style,
    pub regex: Style,
    pub paren: Style,
}

impl Palette {
    /// Styles built from the shared roles. They follow the chosen palette, so
    /// build this after `bs_tui_common::theme::switch`. The syntax theme
    /// fields stay empty.
    fn base(marker: bool) -> Self {
        let shared = bs_tui_common::theme::palette();
        let bold = Modifier::BOLD;
        Self {
            marker,
            current_line: Style::new(),
            preview_match: Style::new(),
            preview_bg: Style::new(),
            gutter: shared.dim,
            marked: shared.warn.add_modifier(bold),
            insert: shared.info.add_modifier(bold),
            normal: shared.warn.add_modifier(bold),
            op: shared.info.add_modifier(bold),
            negate: shared.error.add_modifier(bold),
            quoted: shared.ok,
            regex: shared.warn,
            paren: shared.dim.add_modifier(bold),
        }
    }

    /// Colors from the syntax theme for the preview and the selection.
    /// `background`: paint the preview with the theme background and text color.
    pub fn from_theme(theme: &Theme, background: bool, marker: bool) -> Self {
        let s = &theme.settings;
        let base = Self::base(marker);
        let bg = |c: Option<syntect::highlighting::Color>| Style::new().bg(c.map(rgb).unwrap_or(FALLBACK_SELECTION));
        Self {
            current_line: bg(s.line_highlight.or(s.selection)),
            preview_match: Style::new()
                .fg(s.find_highlight_foreground.map(rgb).unwrap_or(Color::Black))
                .bg(s.find_highlight.map(rgb).unwrap_or(Color::Yellow)),
            // Set the text color with the background. Text without a grammar color
            // (plain text, the gutter) must not keep the terminal's text color.
            preview_bg: match (background, s.background, s.foreground) {
                (true, Some(bg), fg) => Style::new().bg(rgb(bg)).fg(fg.map(rgb).unwrap_or(Color::Reset)),
                _ => Style::new(),
            },
            gutter: s
                .gutter_foreground
                .map(|c| Style::new().fg(rgb(c)))
                .unwrap_or(base.gutter),
            ..base
        }
    }

    /// No colors: only bold, underline and reverse video.
    pub fn plain() -> Self {
        Self {
            current_line: Style::new().add_modifier(Modifier::REVERSED),
            preview_match: Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            quoted: Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            regex: Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            ..Self::base(true)
        }
    }
}
