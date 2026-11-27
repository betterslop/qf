//! qf's own `:` commands, as in vim. The command line, unique prefixes and
//! completion are `bs_tui_common::cmdline`; `:theme`, `:help` and `:quit` are
//! its shared commands.

use std::sync::OnceLock;

use bs_tui_common::cmdline::{self, Shared, Spec};

use crate::config::{LayoutMode, PromptSide};
use crate::query::SortBy;

/// qf's own commands. [`specs`] adds the shared ones.
pub const COMMANDS: &[Spec] = &[
    Spec {
        name: "sort",
        args: &["relevance", "path", "modified"],
        help: "sort order",
    },
    Spec {
        name: "context",
        args: &["0", "1", "2", "3", "5"],
        help: "context lines around hits",
    },
    Spec {
        name: "mode",
        args: &["query", "regex", "file"],
        help: "query, regex or file mode",
    },
    Spec {
        name: "case",
        args: &["smart", "ignore"],
        help: "smart case or ignore case",
    },
    Spec {
        name: "changed",
        args: &[],
        help: "only files changed in git (toggle)",
    },
    Spec {
        name: "layout",
        args: &["auto", "horizontal", "vertical"],
        help: "preview position",
    },
    Spec {
        name: "split",
        args: &["30", "40", "50", "60", "70"],
        help: "percent of the screen for results",
    },
    Spec {
        name: "prompt",
        args: &["bottom", "top"],
        help: "prompt position",
    },
    Spec {
        name: "set",
        args: &[
            "wrap",
            "nowrap",
            "group",
            "nogroup",
            "preview",
            "nopreview",
            "scrollbar",
            "noscrollbar",
            "watch",
            "nowatch",
            "header",
            "noheader",
            "ivy",
            "noivy",
        ],
        help: "turn an option on or off",
    },
    Spec {
        name: "marks",
        args: &["clear", "all"],
        help: "clear or mark all hits",
    },
    Spec {
        name: "open",
        args: &[],
        help: "open the hit, or run an [open.NAME] command",
    },
];

/// qf's commands, then the shared ones.
pub fn specs() -> &'static [Spec] {
    static SPECS: OnceLock<Vec<Spec>> = OnceLock::new();
    SPECS.get_or_init(|| cmdline::with_shared(COMMANDS))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QueryMode {
    Query,
    Regex,
    File,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Command {
    Sort(SortBy),
    Context(usize),
    Mode(QueryMode),
    SmartCase(bool),
    Changed,
    Layout(LayoutMode),
    Split(u16),
    Prompt(PromptSide),
    /// Option name and value, from `:set name` / `:set noname`.
    Set(&'static str, bool),
    Marks {
        all: bool,
    },
    /// Open in the editor, or run the `[open.NAME]` command with this name.
    Open(Option<String>),
    /// `:theme`, `:help` or `:quit`.
    Shared(Shared),
}

pub fn parse(line: &str) -> Result<Command, String> {
    let mut words = line.split_whitespace();
    let Some(name) = words.next() else {
        return Err("no command".into());
    };
    let spec = cmdline::find(specs(), name)?;
    let arg = words.next();
    if let Some(shared) = cmdline::parse_shared(&format!("{} {}", spec.name, arg.unwrap_or_default())) {
        return shared.map(Command::Shared);
    }
    let usage = || cmdline::usage(spec);
    let arg_or_usage = || arg.ok_or_else(usage);
    Ok(match spec.name {
        "sort" => Command::Sort(match arg_or_usage()? {
            "relevance" => SortBy::Relevance,
            "path" => SortBy::Path,
            "modified" | "recent" => SortBy::Modified,
            _ => return Err(usage()),
        }),
        "context" => Command::Context(arg_or_usage()?.parse().map_err(|_| usage())?),
        "mode" => Command::Mode(match arg_or_usage()? {
            "query" => QueryMode::Query,
            "regex" => QueryMode::Regex,
            "file" => QueryMode::File,
            _ => return Err(usage()),
        }),
        "case" => Command::SmartCase(match arg_or_usage()? {
            "smart" => true,
            "ignore" => false,
            _ => return Err(usage()),
        }),
        "changed" => Command::Changed,
        "layout" => Command::Layout(match arg_or_usage()? {
            "auto" => LayoutMode::Auto,
            "horizontal" => LayoutMode::Horizontal,
            "vertical" => LayoutMode::Vertical,
            _ => return Err(usage()),
        }),
        "split" => {
            let n: u16 = arg_or_usage()?.parse().map_err(|_| usage())?;
            Command::Split(n.clamp(10, 90))
        }
        "prompt" => Command::Prompt(match arg_or_usage()? {
            "bottom" => PromptSide::Bottom,
            "top" => PromptSide::Top,
            _ => return Err(usage()),
        }),
        "set" => {
            let opt = arg_or_usage()?;
            let (name, on) = match opt.strip_prefix("no") {
                Some(rest) => (rest, false),
                None => (opt, true),
            };
            let name = ["wrap", "group", "preview", "scrollbar", "watch", "header", "ivy"]
                .into_iter()
                .find(|o| *o == name)
                .ok_or_else(|| format!("unknown option {opt}"))?;
            Command::Set(name, on)
        }
        "marks" => Command::Marks {
            all: match arg_or_usage()? {
                "all" => true,
                "clear" => false,
                _ => return Err(usage()),
            },
        },
        "open" => Command::Open(arg.map(String::from)),
        _ => unreachable!("specs lists the names"),
    })
}

/// The full name of the command in `line`, if the first word names one.
pub fn name(line: &str) -> Option<&'static str> {
    cmdline::find(specs(), line.split_whitespace().next()?)
        .ok()
        .map(|s| s.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bs_tui_common::complete::Candidate;
    use bs_tui_common::theme::Background;

    #[test]
    fn parsing() {
        assert_eq!(parse("sort modified"), Ok(Command::Sort(SortBy::Modified)));
        assert_eq!(parse("so path"), Ok(Command::Sort(SortBy::Path)));
        assert_eq!(parse("q"), Ok(Command::Shared(Shared::Quit)));
        assert_eq!(parse("he"), Ok(Command::Shared(Shared::Help)));
        assert_eq!(parse("th light"), Ok(Command::Shared(Shared::Theme(Background::Light))));
        assert!(parse("theme").unwrap_err().starts_with("usage"));
        assert_eq!(parse("set nowrap"), Ok(Command::Set("wrap", false)));
        assert_eq!(parse("context 3"), Ok(Command::Context(3)));
        assert_eq!(parse("split 95"), Ok(Command::Split(90)));
        assert_eq!(parse("open"), Ok(Command::Open(None)));
        assert_eq!(parse("op copy"), Ok(Command::Open(Some("copy".into()))));
        assert_eq!(name("op copy"), Some("open"));
        assert!(parse("c").unwrap_err().contains("ambiguous"));
        assert!(parse("sort sideways").unwrap_err().starts_with("usage"));
        assert!(parse("bogus").is_err());
    }

    fn open_item(name: &str, label: &str) -> Candidate {
        let mut c = Candidate::new(name.to_string(), "");
        c.label = label.to_string();
        c
    }

    fn texts(input: &str) -> Vec<String> {
        let chars: Vec<char> = input.chars().collect();
        let open = [open_item("code", "code"), open_item("copy", "Copy hit")];
        let args = |name: &str| (name == "open").then(|| open.to_vec());
        cmdline::complete(&chars, chars.len(), specs(), &args)
            .map(|c| c.items.into_iter().map(|i| i.text).collect())
            .unwrap_or_default()
    }

    #[test]
    fn completion() {
        assert_eq!(texts("so"), ["sort"]);
        assert_eq!(texts("set"), ["set"]);
        assert_eq!(texts("sort path"), ["path"]);
        assert_eq!(texts("c"), ["context", "case", "changed"]);
        assert_eq!(texts("sort "), ["relevance", "path", "modified"]);
        assert_eq!(texts("so m"), ["modified"]);
        assert_eq!(
            texts("set no"),
            [
                "nowrap",
                "nogroup",
                "nopreview",
                "noscrollbar",
                "nowatch",
                "noheader",
                "noivy"
            ]
        );
        assert_eq!(texts("").len(), COMMANDS.len() + cmdline::SHARED.len());
        assert_eq!(texts("t"), ["theme"]);
        assert_eq!(texts("open "), ["code", "copy"]);
        assert_eq!(texts("open cop"), ["copy"]);
    }
}
