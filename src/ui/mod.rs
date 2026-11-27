//! Screen layout: the top bar, the prompt line, two panes (results and preview) with
//! borders, and the status bar as the last row. The preview sits at the side
//! (horizontal layout) or above/below the results (vertical layout). Auto layout picks
//! vertical in narrow terminals.

pub mod header;
pub mod help;
mod list;
mod preview;
mod prompt;
mod splash;
pub mod widgets;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::App;
use crate::config::LayoutMode;
use crate::search::{MAX_HITS, Status};
use bs_tui_common::frame::{pane, pane_title};

pub fn draw(f: &mut Frame, app: &mut App, status: &Status) {
    let [mut area, status_area] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());
    if app.show_header && area.height > 3 {
        let [top, rest] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        header::draw(f, app, top, status);
        area = rest;
    }
    // The prompt line sits outside the panes, next to the results.
    let [panes, prompt] = if app.bottom {
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area)
    } else {
        let [p, l] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        [l, p]
    };
    let (results, preview) = split(app, panes);
    let block = pane(hits_title(app));
    let list = block.inner(results);
    f.render_widget(block, results);
    app.list_area = list;
    app.preview_area = preview;

    let anchor = prompt::draw_prompt(f, app, prompt);
    if !app.cmdline.is_open() {
        prompt::draw_status(f, app, status_area, status);
    }
    list::draw(f, app, list, status);
    if let Some(area) = preview {
        preview::draw(f, app, area);
    }
    prompt::draw_menu(f, app, anchor);
    // The `:` line takes the status bar row. Its menu goes over the panes.
    if app.cmdline.is_open() {
        prompt::draw_cmdline(f, app, status_area);
    }
    if let Some(menu) = &app.open_menu {
        let items = menu.menu_items();
        f.render_widget(bs_tui_common::menu::Menu::new("Open", &items, menu.selected), f.area());
    }
    help::draw(f, app);
}

/// The results pane title: ` Hits 12 `. Every hit shows, so there is no `shown/total`.
fn hits_title(app: &App) -> String {
    let n = app.hits.len();
    if n == 0 {
        return pane_title("Hits", 0, 0, false);
    }
    format!(" Hits {n}{} ", if n >= MAX_HITS { "+" } else { "" })
}

/// The results pane and the preview pane. The results stay next to the prompt.
fn split(app: &App, area: Rect) -> (Rect, Option<Rect>) {
    if !app.show_preview {
        return (area, None);
    }
    let vertical = match app.layout {
        LayoutMode::Horizontal => false,
        LayoutMode::Vertical => true,
        LayoutMode::Auto => area.width < app.narrow_width,
    };
    let results = Constraint::Percentage(app.split);
    let preview = Constraint::Fill(1);
    if !vertical {
        let [r, p] = Layout::horizontal([results, preview]).areas(area);
        return (r, Some(p));
    }
    // Keep the results next to the prompt: preview above a bottom prompt, below a top prompt.
    if app.bottom {
        let [p, r] = Layout::vertical([preview, results]).areas(area);
        (r, Some(p))
    } else {
        let [r, p] = Layout::vertical([results, preview]).areas(area);
        (r, Some(p))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use bs_tui_common::notice::Notice;
    use bs_tui_common::theme::{self, Background};
    use crossbeam_channel::unbounded;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::style::Color;

    use super::*;
    use crate::app::{Action, Look};
    use crate::config::Config;
    use crate::query::{Mode, Options, SortBy};
    use crate::search::{Search, WalkConfig};
    use crate::theme::Palette;
    use bs_tui_common::complete::{Candidate, Completion};

    /// The shared palette is global: tests that read or change it take this lock.
    static PALETTE: Mutex<()> = Mutex::new(());

    fn app() -> (App, Search) {
        let (tx, _rx) = unbounded();
        let walk = WalkConfig {
            paths: vec!["src/ui".into()],
            ..WalkConfig::default()
        };
        let search = Search::new(tx, &walk).unwrap();
        let (ptx, _prx) = unbounded();
        let opts = Options {
            mode: Mode::Query,
            ivy: false,
            smart_case: true,
            file_mode: false,
            changed: false,
            context: 0,
        };
        let look = Look {
            palette: Palette::plain(),
            color: true,
            header: header::HeaderInfo {
                folder: "qf".into(),
                branch: None,
                scope: Vec::new(),
            },
        };
        let mut app = App::new("", opts, SortBy::Relevance, &Config::default(), look, ptx);
        app.bottom = false;
        (app, search)
    }

    fn draw(app: &mut App) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(140, 12)).unwrap();
        let status = Status {
            searched: 0,
            total: None,
            indexed: 0,
            index_ready: true,
        };
        terminal.draw(|f| super::draw(f, app, &status)).unwrap();
        terminal
    }

    use crate::search::Status;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn row(terminal: &Terminal<TestBackend>, y: u16) -> String {
        let buf = terminal.backend().buffer();
        (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
    }

    #[test]
    fn ctrl_c_quits_from_the_command_line() {
        let (mut app, search) = app();
        app.on_key(key(KeyCode::Char(':'), KeyModifiers::NONE), &search);
        assert!(app.cmdline.is_open());
        let action = app.on_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL), &search);
        assert!(matches!(action, Action::Quit));
    }

    #[test]
    fn ctrl_l_clears_the_query_in_normal_mode() {
        let (mut app, search) = app();
        type_text(&mut app, &search, "abc");
        press(&mut app, &search, KeyCode::Esc, KeyModifiers::NONE);
        press(&mut app, &search, KeyCode::Char('l'), KeyModifiers::CONTROL);
        assert!(app.input.text().is_empty() && app.mode == crate::app::EditMode::Insert);
    }

    #[test]
    fn theme_command_switches_the_shared_palette() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, search) = app();
        for c in ":theme light".chars() {
            app.on_key(key(KeyCode::Char(c), KeyModifiers::NONE), &search);
        }
        app.on_key(key(KeyCode::Enter, KeyModifiers::NONE), &search);
        let light = theme::palette().accent.fg;
        theme::switch(Background::Dark);
        assert_eq!(light, theme::LIGHT.accent.fg);
        assert_ne!(light, theme::DARK.accent.fg);
    }

    #[test]
    fn status_bar_is_the_last_row_in_both_prompt_positions_and_layouts() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let bar = theme::DARK.bar.bg.unwrap();
        for bottom in [false, true] {
            for layout in [LayoutMode::Horizontal, LayoutMode::Vertical] {
                let (mut app, _search) = app();
                app.bottom = bottom;
                app.layout = layout;
                app.message = Some(Notice::info("ran copy"));
                let terminal = draw(&mut app);
                let last = row(&terminal, 11);
                assert!(last.starts_with(" ran copy · "), "{last}");
                assert!(last.trim_end().ends_with("F1 help"), "{last}");
                let buf = terminal.backend().buffer();
                assert_eq!(buf[(1, 11)].fg, theme::DARK.info.fg.unwrap());
                assert!((0..140).all(|x| buf[(x, 11)].bg == bar));
                // No other row is the status bar, and the prompt keeps its own line.
                assert!((0..11).all(|y| buf[(0, y)].bg != bar || y == 0));
                let prompt = (1..11).find(|&y| row(&terminal, y).starts_with("> ")).expect("prompt");
                assert_eq!(prompt == 1, !bottom, "{bottom}");
            }
        }
    }

    #[test]
    fn help_hint_stays_whole_with_a_hit_count_at_30_and_44_columns() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        for width in [30u16, 44] {
            for normal in [false, true] {
                let (mut app, _search) = app();
                if normal {
                    app.mode = crate::app::EditMode::Normal;
                }
                for c in "fn".chars() {
                    app.input.insert(c);
                }
                app.hits = (0..379)
                    .map(|i| crate::search::Hit {
                        path: "a.rs".into(),
                        line: i,
                        text: String::new(),
                        hl: Vec::new(),
                        before: Vec::new(),
                        after: Vec::new(),
                        modified: None,
                        score: 0,
                        col: 1,
                    })
                    .collect();
                app.stats = Some(crate::app::Stats {
                    files_matched: 30,
                    files_searched: 53,
                    elapsed: std::time::Duration::from_millis(2),
                });
                let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
                let status = Status {
                    searched: 0,
                    total: None,
                    indexed: 0,
                    index_ready: true,
                };
                terminal.draw(|f| super::draw(f, &mut app, &status)).unwrap();
                let last = row(&terminal, 11);
                let help = if normal { "? help" } else { "F1 help" };
                assert!(last.trim_end().ends_with(help), "{width} {normal}: {last}");
                assert!(last.contains("379 hits"), "{width} {normal}: {last}");
            }
        }
    }

    #[test]
    fn insert_mode_readline_keys_match_the_shared_line_editor() {
        let (mut app, search) = app();
        let mut shared = bs_tui_common::input::LineEditor::new("");
        let ctrl = KeyModifiers::CONTROL;
        let mut keys: Vec<KeyEvent> = "foo.bar  baz qux"
            .chars()
            .map(|c| key(KeyCode::Char(c), KeyModifiers::NONE))
            .collect();
        for k in [
            key(KeyCode::Char('w'), ctrl),
            key(KeyCode::Char('w'), ctrl),
            key(KeyCode::Char('b'), ctrl),
            key(KeyCode::Left, KeyModifiers::NONE),
            key(KeyCode::Delete, KeyModifiers::NONE),
            key(KeyCode::Char('f'), ctrl),
            key(KeyCode::Right, KeyModifiers::NONE),
            key(KeyCode::Backspace, KeyModifiers::NONE),
            key(KeyCode::Char('h'), ctrl),
            key(KeyCode::Char('a'), ctrl),
            key(KeyCode::Delete, KeyModifiers::NONE),
            key(KeyCode::End, KeyModifiers::NONE),
            key(KeyCode::Char('e'), ctrl),
            key(KeyCode::Home, KeyModifiers::NONE),
            key(KeyCode::Char('e'), ctrl),
            key(KeyCode::Left, KeyModifiers::NONE),
            key(KeyCode::Char('u'), ctrl),
        ] {
            keys.push(k);
        }
        for k in keys {
            app.on_key(k, &search);
            shared.key(&k);
            assert_eq!(app.input.text(), shared.text(), "{k:?}");
            assert_eq!(app.input.cursor(), shared.cursor(), "{k:?}");
        }
    }

    #[test]
    fn narrow_status_bar_ends_with_the_whole_help_hint() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, _search) = app();
        app.mode = crate::app::EditMode::Normal;
        let mut terminal = Terminal::new(TestBackend::new(30, 12)).unwrap();
        let status = Status {
            searched: 0,
            total: None,
            indexed: 0,
            index_ready: true,
        };
        terminal.draw(|f| super::draw(f, &mut app, &status)).unwrap();
        let last = row(&terminal, 11);
        assert!(last.trim_end().ends_with("? help"), "{last}");
    }

    #[test]
    fn status_bar_shows_normal_mode_help_hint_and_cuts_a_long_notice() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, _search) = app();
        app.mode = crate::app::EditMode::Normal;
        app.message = Some(Notice::error("x".repeat(80)));
        let terminal = draw(&mut app);
        let last = row(&terminal, 11);
        assert!(last.starts_with(&format!(" {}… · ", "x".repeat(59))), "{last}");
        assert!(
            last.contains("[normal]") && last.trim_end().ends_with("? help"),
            "{last}"
        );
        assert_eq!(terminal.backend().buffer()[(1, 11)].fg, Color::Red);
    }

    #[test]
    fn completion_menu_is_a_rounded_accent_popup_with_a_marker() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, _search) = app();
        app.completion = Some(Completion {
            start: 0,
            end: 0,
            items: ["alpha", "beta"]
                .map(|t| Candidate {
                    text: t.into(),
                    label: t.into(),
                    kind: "word",
                    hl: Vec::new(),
                })
                .to_vec(),
            selected: 1,
        });
        let terminal = draw(&mut app);
        let buf = terminal.backend().buffer();
        let (x, y) = (0..12)
            .find_map(|y| (0..140).find(|&x| buf[(x, y)].symbol() == "╭").map(|x| (x, y)))
            .expect("rounded corner");
        assert_eq!(buf[(x, y)].fg, theme::DARK.accent.fg.unwrap());
        // The selected (second) row has the marker in the `marker` role.
        assert_eq!(buf[(x + 1, y + 2)].symbol(), "▌");
        assert_eq!(buf[(x + 1, y + 2)].fg, theme::DARK.marker.fg.unwrap());
        assert_eq!(buf[(x + 1, y + 1)].symbol(), " ");
    }

    #[test]
    fn help_popup_has_a_rounded_accent_border() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, _search) = app();
        app.help = Some(0);
        let terminal = draw(&mut app);
        let buf = terminal.backend().buffer();
        let (x, y) = (0..12)
            .find_map(|y| (0..140).find(|&x| buf[(x, y)].symbol() == "╭").map(|x| (x, y)))
            .expect("rounded corner");
        assert_eq!(buf[(x, y)].fg, theme::DARK.accent.fg.unwrap());
    }

    #[test]
    fn preview_border_uses_the_border_role() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, _search) = app();
        app.layout = LayoutMode::Horizontal;
        app.split = 50;
        let terminal = draw(&mut app);
        let buf = terminal.backend().buffer();
        let x = (0..140).find(|&x| buf[(x, 6)].symbol() == "│").expect("border column");
        assert_eq!(buf[(x, 6)].fg, theme::DARK.border.fg.unwrap());
    }

    fn hit(path: &str, line: u64, text: &str, hl: Vec<(usize, usize)>) -> crate::search::Hit {
        crate::search::Hit {
            path: path.into(),
            line,
            text: text.into(),
            hl,
            before: Vec::new(),
            after: Vec::new(),
            modified: None,
            score: 0,
            col: 1,
        }
    }

    fn draw_sized(app: &mut App, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        let status = Status {
            searched: 0,
            total: None,
            indexed: 0,
            index_ready: true,
        };
        terminal.draw(|f| super::draw(f, app, &status)).unwrap();
        terminal
    }

    fn press(app: &mut App, search: &Search, code: KeyCode, m: KeyModifiers) -> Action {
        app.on_key(key(code, m), search)
    }

    fn type_text(app: &mut App, search: &Search, text: &str) {
        for c in text.chars() {
            press(app, search, KeyCode::Char(c), KeyModifiers::NONE);
        }
    }

    #[test]
    fn results_and_preview_are_bordered_panes_in_every_layout() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let border = theme::DARK.border.fg.unwrap();
        for bottom in [false, true] {
            for layout in [LayoutMode::Horizontal, LayoutMode::Vertical, LayoutMode::Auto] {
                for width in [140, 60] {
                    let (mut app, search) = app();
                    app.bottom = bottom;
                    app.layout = layout;
                    app.hits = vec![hit("src/main.rs", 3, "fn main()", vec![])];
                    let terminal = draw_sized(&mut app, width, 20);
                    let rows: Vec<String> = (0..20).map(|y| row(&terminal, y)).collect();
                    let title = rows
                        .iter()
                        .position(|r| r.contains("┌ Hits 1 ┐") || r.contains("┌ Hits 1 ─"))
                        .expect("results title");
                    let ptitle = rows
                        .iter()
                        .position(|r| r.contains("┌ src/main.rs:3 "))
                        .expect("preview title");
                    let buf = terminal.backend().buffer();
                    let x = rows[title].find('┌').map(|b| rows[title][..b].chars().count()).unwrap() as u16;
                    assert_eq!(buf[(x, title as u16)].fg, border);
                    // The prompt and the status bar stay outside the panes.
                    let prompt = rows.iter().position(|r| r.starts_with("> ")).unwrap();
                    assert!(
                        !rows[prompt].contains('│') && !rows[19].contains('│'),
                        "{bottom} {width}"
                    );
                    assert!(rows[19].trim_end().ends_with("F1 help"));
                    // The page size and the mouse rows follow the inner area.
                    assert_eq!(app.page, app.list_area.height as usize);
                    assert!(app.list_area.y as usize > title && app.list_area.x > 0);
                    let ra = app.preview_area.unwrap();
                    assert!(ra.y as usize <= ptitle && ptitle < (ra.y + ra.height) as usize);
                    app.selected = 0;
                    app.on_mouse(ratatui::crossterm::event::MouseEvent {
                        kind: ratatui::crossterm::event::MouseEventKind::Down(
                            ratatui::crossterm::event::MouseButton::Left,
                        ),
                        column: app.list_area.x,
                        row: app.list_area.y + app.list_area.height - 1,
                        modifiers: KeyModifiers::NONE,
                    });
                    drop(search);
                }
            }
        }
    }

    #[test]
    fn mouse_click_selects_the_row_under_the_pointer() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, _search) = app();
        app.hits = vec![
            hit("a.rs", 1, "one", vec![]),
            hit("a.rs", 2, "two", vec![]),
            hit("a.rs", 3, "three", vec![]),
        ];
        app.group = false;
        let terminal = draw_sized(&mut app, 140, 20);
        let y = (0..20).find(|&y| row(&terminal, y).contains("a.rs:3: three")).unwrap();
        app.on_mouse(ratatui::crossterm::event::MouseEvent {
            kind: ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left),
            column: 10,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn hits_title_counts_hits() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, _search) = app();
        let terminal = draw_sized(&mut app, 140, 20);
        assert!((0..20).any(|y| row(&terminal, y).contains("┌ Hits ─")));
        app.hits = vec![hit("a.rs", 1, "x", vec![]), hit("a.rs", 2, "x", vec![])];
        let terminal = draw_sized(&mut app, 140, 20);
        assert!((0..20).any(|y| row(&terminal, y).contains("┌ Hits 2 ─")));
    }

    #[test]
    fn selected_row_keeps_the_matched_style() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, _search) = app();
        app.group = false;
        app.hits = vec![hit("a.rs", 1, "fn main()", vec![(3, 7)])];
        let terminal = draw_sized(&mut app, 140, 20);
        let y = (0..20)
            .find(|&y| row(&terminal, y).contains("a.rs:1: fn main()"))
            .unwrap();
        let buf = terminal.backend().buffer();
        let matched: String = (0..140)
            .filter(|&x| buf[(x, y)].bg == theme::DARK.matched.bg.unwrap())
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        assert_eq!(matched, "main");
        // The rest of the row has the selected background.
        let x = row(&terminal, y).find("a.rs").unwrap() as u16;
        assert_eq!(buf[(x, y)].bg, theme::DARK.selected.bg.unwrap());
    }

    #[test]
    fn spinner_is_in_warn() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, _search) = app();
        app.input = crate::editor::LineEditor::new("x");
        app.searching = true;
        let terminal = draw_sized(&mut app, 140, 20);
        let buf = terminal.backend().buffer();
        let (x, y) = (2..19)
            .flat_map(|y| (0..70).map(move |x| (x, y)))
            .find(|&(x, y)| {
                buf[(x, y)]
                    .symbol()
                    .chars()
                    .next()
                    .is_some_and(|c| ('\u{2800}'..='\u{28ff}').contains(&c))
            })
            .expect("spinner in the list");
        assert_eq!(buf[(x, y)].fg, theme::DARK.warn.fg.unwrap());
        assert!(row(&terminal, y).contains("Searching…"));
    }

    #[test]
    fn splash_logo_uses_palette_roles_only() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, _search) = app();
        let terminal = draw_sized(&mut app, 140, 30);
        let buf = terminal.backend().buffer();
        let logo: Vec<Color> = (0..30)
            .flat_map(|y| (0..140).map(move |x| (x, y)))
            .filter(|&(x, y)| buf[(x, y)].symbol() == "█")
            .map(|(x, y)| buf[(x, y)].fg)
            .collect();
        assert!(!logo.is_empty());
        assert!(logo.iter().all(|c| *c == theme::DARK.accent.fg.unwrap()));
    }

    #[test]
    fn path_field_matches_are_highlighted_in_the_drawn_path_and_preview_title() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, search) = app();
        app.group = false;
        app.input = crate::editor::LineEditor::new("fn path:main");
        app.restart(&search);
        app.hits = vec![hit("src/main.rs", 3, "fn x()", vec![(0, 2)])];
        let terminal = draw_sized(&mut app, 140, 20);
        let buf = terminal.backend().buffer();
        let bg = theme::DARK.matched.bg.unwrap();
        let marked = |y: u16| -> String {
            (0..140)
                .filter(|&x| buf[(x, y)].bg == bg)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        };
        let list = (0..20)
            .find(|&y| row(&terminal, y).contains("src/main.rs:3: fn x()"))
            .unwrap();
        assert_eq!(marked(list), "mainfn");
        let title = (0..20)
            .find(|&y| row(&terminal, y).contains("┌ src/main.rs:3"))
            .unwrap();
        assert_eq!(marked(title), "main");
    }

    #[test]
    fn normal_mode_esc_clears_the_query_and_slash_types_again() {
        let (mut app, search) = app();
        app.input = crate::editor::LineEditor::new("abc");
        app.mode = crate::app::EditMode::Normal;
        let a = press(&mut app, &search, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!matches!(a, Action::Quit));
        assert!(app.input.is_empty());
        let a = press(&mut app, &search, KeyCode::Esc, KeyModifiers::NONE);
        assert!(!matches!(a, Action::Quit));
        app.input = crate::editor::LineEditor::new("abc");
        app.input.move_to(0);
        press(&mut app, &search, KeyCode::Char('/'), KeyModifiers::NONE);
        assert!(app.mode == crate::app::EditMode::Insert);
        assert_eq!(app.input.cursor(), 3);
        assert_eq!(app.input.text(), "abc");
    }

    #[test]
    fn command_line_has_readline_keys_and_keeps_its_text_on_esc() {
        let (mut app, search) = app();
        let ctrl = KeyModifiers::CONTROL;
        press(&mut app, &search, KeyCode::Char(':'), KeyModifiers::NONE);
        type_text(&mut app, &search, "foo bar");
        let text = |app: &App| app.cmdline.editor().unwrap().text();
        press(&mut app, &search, KeyCode::Char('w'), ctrl);
        assert_eq!(text(&app), "foo ");
        press(&mut app, &search, KeyCode::Char('a'), ctrl);
        type_text(&mut app, &search, "x");
        assert_eq!(text(&app), "xfoo ");
        press(&mut app, &search, KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(text(&app), "xoo ");
        press(&mut app, &search, KeyCode::End, KeyModifiers::NONE);
        press(&mut app, &search, KeyCode::Char('h'), ctrl);
        assert_eq!(text(&app), "xoo");
        press(&mut app, &search, KeyCode::Char('b'), ctrl);
        press(&mut app, &search, KeyCode::Char('u'), ctrl);
        assert_eq!(text(&app), "o");
        // Esc closes the line and keeps the text.
        // (The first Esc closes the completion menu, if it is open.)
        press(&mut app, &search, KeyCode::Esc, KeyModifiers::NONE);
        if app.cmdline.is_open() {
            press(&mut app, &search, KeyCode::Esc, KeyModifiers::NONE);
        }
        assert!(!app.cmdline.is_open());
        press(&mut app, &search, KeyCode::Char(':'), KeyModifiers::NONE);
        assert_eq!(text(&app), "o");
        // Enter runs a command, and the next line starts empty.
        press(&mut app, &search, KeyCode::Char('u'), ctrl);
        type_text(&mut app, &search, "case smart");
        press(&mut app, &search, KeyCode::Enter, KeyModifiers::NONE);
        assert!(!app.cmdline.is_open());
        press(&mut app, &search, KeyCode::Char(':'), KeyModifiers::NONE);
        assert_eq!(text(&app), "");
    }

    #[test]
    fn f1_opens_help_from_the_command_line() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        let (mut app, search) = app();
        app.mode = crate::app::EditMode::Normal;
        press(&mut app, &search, KeyCode::Char(':'), KeyModifiers::NONE);
        press(&mut app, &search, KeyCode::F(1), KeyModifiers::NONE);
        assert!(app.help.is_some() && app.cmdline.is_open());
        press(&mut app, &search, KeyCode::F(1), KeyModifiers::NONE);
        assert!(app.help.is_none() && app.cmdline.is_open());
        let terminal = draw_sized(&mut app, 140, 20);
        assert!(row(&terminal, 19).starts_with(" :█"), "{}", row(&terminal, 19));
    }

    #[test]
    fn command_line_draws_on_the_status_bar_row_with_its_menu_above() {
        let _lock = PALETTE.lock().unwrap();
        theme::switch(Background::Dark);
        for bottom in [false, true] {
            let (mut app, search) = app();
            app.bottom = bottom;
            press(&mut app, &search, KeyCode::Char(':'), KeyModifiers::NONE);
            type_text(&mut app, &search, "theme ");
            let terminal = draw_sized(&mut app, 140, 20);
            assert!(row(&terminal, 19).starts_with(" :theme █"), "{}", row(&terminal, 19));
            assert!(terminal.backend().buffer()[(1, 19)].bg == theme::DARK.bar.bg.unwrap());
            // The menu sits above the bar, at the word, with the first item next to it.
            assert!(row(&terminal, 18).contains("╰──────╯"), "{}", row(&terminal, 18));
            assert!(row(&terminal, 17).contains("│▌dark │"), "{}", row(&terminal, 17));
            assert!(row(&terminal, 16).contains("│ light│"), "{}", row(&terminal, 16));
            let at_word: String = row(&terminal, 17).chars().skip(7).take(7).collect();
            assert_eq!(at_word, "│▌dark ");
            // The query prompt stays on its own row.
            assert!((0..19).any(|y| row(&terminal, y).starts_with("> ")));
        }
    }

    #[test]
    fn command_line_enter_rules() {
        let (mut app, search) = app();
        // A complete command runs as typed, also with the menu open.
        press(&mut app, &search, KeyCode::Char(':'), KeyModifiers::NONE);
        type_text(&mut app, &search, "split 3");
        assert!(app.cmdline.completion().is_some());
        press(&mut app, &search, KeyCode::Enter, KeyModifiers::NONE);
        assert!(!app.cmdline.is_open());
        assert_eq!(app.split, 10);
        // A name with arguments: Enter completes it and shows the arguments.
        press(&mut app, &search, KeyCode::Char(':'), KeyModifiers::NONE);
        type_text(&mut app, &search, "pro");
        press(&mut app, &search, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.cmdline.editor().unwrap().text(), "prompt ");
        // Up is the next item: the menu is above the status bar.
        press(&mut app, &search, KeyCode::Up, KeyModifiers::NONE);
        press(&mut app, &search, KeyCode::Enter, KeyModifiers::NONE);
        assert!(!app.cmdline.is_open() && !app.bottom);
        // An unknown command shows an error.
        press(&mut app, &search, KeyCode::Char(':'), KeyModifiers::NONE);
        type_text(&mut app, &search, "xyz");
        press(&mut app, &search, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.message.as_ref().unwrap().text, "unknown command :xyz");
    }
}
