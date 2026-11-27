//! Top bar, as in lazygit, btop and k9s: the folder, git branch and file count on the
//! left, the search scope from the command-line flags on the right.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use super::widgets::path_spans;
use crate::app::App;
use crate::git;
use crate::search::{Status, WalkConfig};

/// What the top bar shows. Read once at startup.
pub struct HeaderInfo {
    /// The current folder, with `~` for the home folder.
    pub folder: String,
    pub branch: Option<String>,
    /// Flags that change which files are searched, for example `hidden` or `-g !*.lock`.
    pub scope: Vec<String>,
}

impl HeaderInfo {
    pub fn new(walk: &WalkConfig) -> Self {
        let folder = std::env::current_dir().map(|p| folder_of(&p)).unwrap_or_default();
        let mut scope: Vec<String> = walk.paths.iter().map(|p| p.display().to_string()).collect();
        if walk.hidden {
            scope.push("hidden".into());
        }
        if walk.no_ignore {
            scope.push("no-ignore".into());
        }
        scope.extend(walk.globs.iter().map(|g| format!("-g {g}")));
        scope.extend(walk.types.iter().map(|t| format!("-t {t}")));
        scope.extend(walk.types_not.iter().map(|t| format!("-T {t}")));
        Self {
            folder,
            branch: git::branch(),
            scope,
        }
    }
}

/// The folder with the home folder written as `~`.
fn folder_of(cwd: &std::path::Path) -> String {
    bs_tui_common::paths::short_home(cwd)
}

pub fn draw(f: &mut Frame, app: &App, area: Rect, status: &Status) {
    let h = &app.header;
    let files = if status.index_ready {
        format!("{} files", thousands(status.indexed))
    } else {
        format!("indexing… {} files", thousands(status.indexed))
    };

    // Two spaces before the scope keep it apart from the left side.
    let right = h.scope.join(" · ");
    let right_w = if right.is_empty() {
        0
    } else {
        right.chars().count() as u16 + 3
    };
    let [left_area, right_area] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_w)]).areas(area);

    // The folder gets the room the other parts leave. Its last part stays whole.
    // " qf " + " " before the folder, " · branch", " · N files" (frame::title_bar).
    let dot_w = 3;
    let fixed = 4 + 1 + h.branch.as_ref().map_or(0, |b| dot_w + b.chars().count()) + dot_w + files.chars().count();
    let room = (left_area.width as usize).saturating_sub(fixed).max(8);
    let shared = bs_tui_common::theme::palette();
    let folder: String = path_spans(&h.folder, room, Style::new(), Style::new())
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    let mut parts = vec![Span::raw(folder)];
    if let Some(branch) = &h.branch {
        parts.push(Span::raw(branch.clone()));
    }
    parts.push(Span::raw(files));
    let line = bs_tui_common::frame::title_bar("qf", parts);
    f.render_widget(Paragraph::new(line).style(shared.bar), left_area);
    f.render_widget(
        Paragraph::new(Span::raw(if right.is_empty() {
            String::new()
        } else {
            format!("{right} ")
        }))
        .right_aligned()
        .style(shared.bar),
        right_area,
    );
}

/// `2155` -> `2,155`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_prefix_must_end_at_a_folder_boundary() {
        static HOME: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _lock = HOME.lock().unwrap();
        let old = std::env::var_os("HOME");
        // SAFETY: no other test in this module reads HOME.
        unsafe { std::env::set_var("HOME", "/Users/skid") };
        let short = |p: &str| folder_of(std::path::Path::new(p));
        assert_eq!(short("/Users/skidmore"), "/Users/skidmore");
        assert_eq!(short("/Users/skid/src/qf"), "~/src/qf");
        assert_eq!(short("/Users/skid"), "~");
        match old {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }
    }

    #[test]
    fn numbers() {
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(2155), "2,155");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}
