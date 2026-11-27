mod app;
mod command;
mod complete;
mod config;
mod editor;
mod git;
mod history;
mod open;
mod preview;
mod query;
mod search;
mod syntax;
mod theme;
mod ui;

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use bs_tui_common::notice::Notice;
use clap::Parser;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded, unbounded};
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};

use app::{Action, App, EditMode};
use config::Config;
use query::{Mode, Options, SortBy};
use search::{Search, WalkConfig};

/// Wait this long after the last keystroke before starting a search.
const DEBOUNCE: Duration = Duration::from_millis(30);

/// Redraw this often while something moves on screen (spinner, progress).
const TICK: Duration = Duration::from_millis(80);

/// The input thread checks for a pause request this often.
const INPUT_CHECK: Duration = Duration::from_millis(50);

/// Interactive ripgrep-style search with a file preview. Enter opens the hit in $EDITOR.
///
/// Query syntax (default): `foo bar` (AND), `foo OR bar`, `(a OR b) AND c`, `foo -bar`,
/// `NOT foo`, `"literal text"`, `/regex/`. Bare words are regex. A capital letter makes
/// a term case-sensitive.
#[derive(Parser)]
#[command(version, after_help = format!("{}\n\n{}", ui::help::KEYS, config::EXAMPLE))]
struct Args {
    /// Regex mode: the whole input is one regex
    #[arg(short, long)]
    regex: bool,
    /// Regex mode with ivy-regex-plus style: spaces become `.*`
    #[arg(short, long)]
    ivy: bool,
    /// Case-insensitive for all terms (default: smart case per term)
    #[arg(short = 'I', long)]
    ignore_case: bool,
    /// Search hidden files and folders
    #[arg(long)]
    hidden: bool,
    /// Do not respect .gitignore, .ignore and other ignore files
    #[arg(long)]
    no_ignore: bool,
    /// Include files that match GLOB, or exclude them with a `!` prefix (repeatable)
    #[arg(short, long = "glob", value_name = "GLOB")]
    globs: Vec<String>,
    /// Only search files of this ripgrep type, for example `rust` (repeatable)
    #[arg(short, long = "type", value_name = "TYPE")]
    types: Vec<String>,
    /// Do not search files of this ripgrep type (repeatable)
    #[arg(short = 'T', long = "type-not", value_name = "TYPE")]
    types_not: Vec<String>,
    /// Only files changed in git (modified, staged or untracked)
    #[arg(long)]
    changed: bool,
    /// Terms apply to the whole file, not to one line
    #[arg(long)]
    file_mode: bool,
    /// Show NUM context lines around each hit
    #[arg(short = 'C', long, value_name = "NUM")]
    context: Option<usize>,
    /// Sort order of the results
    #[arg(long, value_enum)]
    sort: Option<SortBy>,
    /// Initial query (quote it if it has spaces)
    query: Option<String>,
    /// Files or folders to search (default: the current folder)
    paths: Vec<PathBuf>,
}

/// Everything the main loop waits for comes through one channel.
pub enum AppEvent {
    Term(Event),
    Search(search::Event),
    /// A file in the search folders changed.
    Changed,
    Preview(preview::Response),
}

fn main() {
    bs_tui_common::term::main(run_app);
}

fn run_app() -> Result<()> {
    let args = Args::parse();
    let config = Config::load()?;
    // Flags override the config file.
    let opts = Options {
        mode: if args.regex || args.ivy {
            Mode::Regex
        } else {
            config.mode
        },
        ivy: args.ivy || config.ivy,
        smart_case: !args.ignore_case && config.smart_case,
        file_mode: args.file_mode || config.file_mode,
        changed: args.changed,
        context: args.context.unwrap_or(config.context),
    };
    let walk = WalkConfig {
        paths: args.paths,
        hidden: args.hidden,
        no_ignore: args.no_ignore,
        globs: args.globs,
        types: args.types,
        types_not: args.types_not,
    };
    // https://no-color.org: any non-empty value turns colors off.
    let color = env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    // Theme order (bs-tui-common LOOK.md): NO_COLOR, TUI_THEME, the `theme`
    // setting, the terminal background. `auto` asks the terminal, so this must
    // run before the TUI reads input.
    let theme_name = match bs_tui_common::theme::init_with(config.theme.background()) {
        bs_tui_common::theme::Background::Light => theme::ThemeName::Light,
        bs_tui_common::theme::Background::Dark => theme::ThemeName::Dark,
    };
    let palette = if color {
        theme::Palette::from_theme(&theme::load(theme_name), config.theme_background, config.marker)
    } else {
        theme::Palette::plain()
    };
    let (tx, rx) = unbounded();
    let search = Search::new(tx.clone(), &walk)?;
    let preview_tx = preview::spawn(tx.clone(), color, theme_name);
    let query = args.query.unwrap_or_default();
    let sort = args.sort.unwrap_or(config.sort);
    let header = ui::header::HeaderInfo::new(&walk);
    let look = app::Look { palette, color, header };
    let mut app = App::new(&query, opts, sort, &config, look, preview_tx);

    let mut terminal = bs_tui_common::term::init(config.mouse);
    let input = spawn_input(tx);
    let result = run(&mut terminal, &mut app, &search, &rx, &input, config.mouse);
    bs_tui_common::term::restore();

    match result? {
        Some(launch) => open::run(&launch),
        None => Ok(()),
    }
}

/// Read terminal events on a thread, so the main loop blocks on one channel and never polls.
///
/// A send on the returned channel pauses the thread, and the next send resumes it. The
/// channel has no buffer, so the first send returns only when the thread has stopped
/// reading: a command in the terminal then gets every key.
fn spawn_input(tx: Sender<AppEvent>) -> Sender<()> {
    let (pause_tx, pause_rx) = bounded::<()>(0);
    thread::spawn(move || {
        loop {
            if pause_rx.try_recv().is_ok() && pause_rx.recv().is_err() {
                break;
            }
            match event::poll(INPUT_CHECK) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => break,
            }
            let Ok(ev) = event::read() else { break };
            if tx.send(AppEvent::Term(ev)).is_err() {
                break;
            }
        }
    });
    pause_tx
}

/// Close the TUI, run the command in the terminal, then show the TUI again.
fn run_in_terminal(
    terminal: &mut DefaultTerminal,
    input: &Sender<()>,
    mouse: bool,
    launch: &open::Launch,
) -> Result<()> {
    let _ = input.send(());
    bs_tui_common::term::restore();
    let result = open::wait(launch);
    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    bs_tui_common::term::set_mouse(mouse);
    terminal.clear()?;
    let _ = input.send(());
    result
}

fn run(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    search: &Search,
    rx: &Receiver<AppEvent>,
    input: &Sender<()>,
    mouse: bool,
) -> Result<Option<open::Launch>> {
    // `--changed` lists the changed files without a query.
    if !app.input.is_empty() || app.opts.changed {
        app.restart(search);
    }
    let mut shape = None;
    loop {
        if shape != Some(app.mode) {
            let style = match app.mode {
                EditMode::Insert => SetCursorStyle::SteadyBar,
                EditMode::Normal => SetCursorStyle::SteadyBlock,
            };
            execute!(terminal.backend_mut(), style)?;
            terminal.backend_mut().flush()?;
            shape = Some(app.mode);
        }
        let status = search.status();
        terminal.draw(|f| ui::draw(f, app, &status))?;

        // Wait for an event, the end of the debounce, or the next tick while the
        // spinner runs.
        let ticking = app.searching || (app.input.is_empty() && !status.index_ready);
        let deadline = [
            app.search_due.map(|t| t + DEBOUNCE),
            ticking.then(|| Instant::now() + TICK),
        ]
        .into_iter()
        .flatten()
        .min();
        let first = match deadline {
            Some(t) => match rx.recv_deadline(t) {
                Ok(ev) => Some(ev),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return Ok(None),
            },
            None => Some(rx.recv()?),
        };
        // Handle all queued events before the next draw.
        for ev in first.into_iter().chain(std::iter::from_fn(|| rx.try_recv().ok())) {
            match handle(app, search, ev) {
                Action::None => {}
                Action::Quit => return Ok(None),
                Action::Open(launch) => return Ok(Some(launch)),
                Action::Run(launch) => {
                    if let Err(e) = run_in_terminal(terminal, input, mouse, &launch) {
                        app.message = Some(Notice::error(format!("{e:#}")));
                    }
                    // The editor reset the cursor shape.
                    shape = None;
                    break;
                }
            }
        }
        if app.search_due.is_some_and(|t| t.elapsed() >= DEBOUNCE) {
            app.restart(search);
        }
    }
}

/// Handle one event. Returns the key's action, or `Action::None`.
fn handle(app: &mut App, search: &Search, ev: AppEvent) -> Action {
    match ev {
        AppEvent::Search(ev) => app.on_search(ev),
        AppEvent::Changed => app.on_changed(search),
        AppEvent::Preview(resp) => app.preview = Some(resp),
        AppEvent::Term(Event::Key(key)) if key.kind == KeyEventKind::Press => return app.on_key(key, search),
        AppEvent::Term(Event::Mouse(ev)) => app.on_mouse(ev),
        AppEvent::Term(_) => {}
    }
    Action::None
}
