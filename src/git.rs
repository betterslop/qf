//! Git working-tree status, from `git status --porcelain`. Paths are relative to the
//! current directory, the same as the paths in the results.

use std::collections::HashMap;
use std::process::Command;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Flags {
    /// Changes in the index.
    pub staged: bool,
    /// Changes in the working tree to a tracked file.
    pub modified: bool,
    pub untracked: bool,
}

#[derive(Default)]
pub struct GitStatus {
    files: HashMap<String, Flags>,
}

impl GitStatus {
    pub fn get(&self, path: &str) -> Flags {
        self.files.get(path).copied().unwrap_or_default()
    }

    /// Run git in the current directory. `None` outside a git repository.
    pub fn load() -> Option<Self> {
        let prefix = git(&["rev-parse", "--show-prefix"])?;
        let prefix = String::from_utf8_lossy(&prefix).trim().to_string();
        let out = git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
        Some(Self::parse(&out, &prefix))
    }

    /// Parse `git status --porcelain=v1 -z` output. Git prints paths relative to the
    /// repository root. `prefix` is the current directory inside the repository.
    fn parse(out: &[u8], prefix: &str) -> Self {
        let mut files = HashMap::new();
        let mut entries = out.split(|&b| b == 0).filter(|e| e.len() > 3);
        while let Some(entry) = entries.next() {
            let (x, y) = (entry[0], entry[1]);
            let path = String::from_utf8_lossy(&entry[3..]);
            // A rename or copy has the old path in the next entry.
            if x == b'R' || x == b'C' {
                entries.next();
            }
            let Some(rel) = path.strip_prefix(prefix) else {
                continue;
            };
            let flags = Flags {
                staged: x != b' ' && x != b'?',
                modified: y != b' ' && y != b'?',
                untracked: x == b'?',
            };
            files.insert(rel.to_string(), flags);
        }
        Self { files }
    }
}

/// The current branch, or the short commit hash on a detached HEAD.
/// `None` outside a repository or before the first commit.
pub fn branch() -> Option<String> {
    let text = |out: Vec<u8>| String::from_utf8_lossy(&out).trim().to_string();
    let name = text(git(&["rev-parse", "--abbrev-ref", "HEAD"])?);
    if name != "HEAD" {
        return Some(name);
    }
    Some(text(git(&["rev-parse", "--short", "HEAD"])?))
}

fn git(args: &[&str]) -> Option<Vec<u8>> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then_some(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_porcelain() {
        let out = b"M  sub/a.rs\0 M sub/b.rs\0?? sub/new.txt\0R  sub/c.rs\0sub/old.rs\0 M other/x.rs\0";
        let s = GitStatus::parse(out, "sub/");
        assert_eq!(
            s.get("a.rs"),
            Flags {
                staged: true,
                modified: false,
                untracked: false
            }
        );
        assert_eq!(
            s.get("b.rs"),
            Flags {
                staged: false,
                modified: true,
                untracked: false
            }
        );
        assert!(s.get("new.txt").untracked);
        assert!(s.get("c.rs").staged);
        assert_eq!(s.get("old.rs"), Flags::default());
        assert_eq!(s.get("x.rs"), Flags::default());
    }
}
