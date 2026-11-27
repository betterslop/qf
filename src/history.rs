//! Query history, one query per line in `$XDG_STATE_HOME/qf/history`
//! (default `~/.local/state/qf/history`).

use std::fs;
use std::io;
use std::path::PathBuf;

const MAX_ITEMS: usize = 500;

#[derive(Default)]
pub struct History {
    items: Vec<String>,
    /// Index into `items` while the user steps through history.
    pos: Option<usize>,
    /// The query the user typed before stepping into history.
    draft: String,
    path: Option<PathBuf>,
}

fn history_path() -> Option<PathBuf> {
    Some(bs_tui_common::paths::state_dir("qf")?.join("history"))
}

impl History {
    /// Saved queries, oldest first.
    pub fn items(&self) -> &[String] {
        &self.items
    }

    pub fn load() -> Self {
        let path = history_path();
        let items = path
            .as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect())
            .unwrap_or_default();
        Self {
            items,
            path,
            ..Self::default()
        }
    }

    /// Step to an older query. `current` is kept as the draft when leaving it.
    pub fn prev(&mut self, current: &str) -> Option<String> {
        let last = self.items.len().checked_sub(1)?;
        let i = match self.pos {
            None => {
                self.draft = current.to_string();
                last
            }
            Some(i) => i.saturating_sub(1),
        };
        self.pos = Some(i);
        Some(self.items[i].clone())
    }

    /// Step to a newer query, and back to the draft after the newest.
    pub fn next(&mut self) -> Option<String> {
        let i = self.pos?;
        if i + 1 < self.items.len() {
            self.pos = Some(i + 1);
            Some(self.items[i + 1].clone())
        } else {
            self.pos = None;
            Some(std::mem::take(&mut self.draft))
        }
    }

    /// Leave history navigation (the user edited the query).
    pub fn reset(&mut self) {
        self.pos = None;
    }

    pub fn save(&mut self, query: &str) -> io::Result<()> {
        let query = query.trim();
        let Some(path) = &self.path else {
            return Ok(());
        };
        if query.is_empty() {
            return Ok(());
        }
        self.items.retain(|q| q != query);
        self.items.push(query.to_string());
        let skip = self.items.len().saturating_sub(MAX_ITEMS);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, self.items[skip..].join("\n") + "\n")
    }
}
