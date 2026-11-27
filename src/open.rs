//! Run the editor or an `[open.NAME]` command on hits.

use std::env;
use std::fs;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::thread;

use anyhow::{Context, Result};

use crate::app::Target;

/// A command to run on hits.
pub struct Launch {
    pub cmd: String,
    /// Template for the command's input, one line per target.
    pub stdin: Option<String>,
    pub targets: Vec<Target>,
}

const PLACEHOLDERS: [&str; 4] = ["{file}", "{line}", "{col}", "{text}"];

/// Fill in the placeholders for one target.
fn fill(template: &str, t: &Target) -> String {
    template
        .replace("{file}", &t.path)
        .replace("{line}", &t.line.to_string())
        .replace("{col}", &t.col.to_string())
        .replace("{text}", &t.text)
}

/// The command line for `targets`.
///
/// - Default: `EDITOR +LINE FILE`. Several targets: `EDITOR -q LIST` (a vim and nvim
///   quickfix list).
/// - Template: if the command has `{file}`, `{line}`, `{col}` or `{text}`, `qf` fills
///   them in. For several targets, it repeats the arguments after the program name for
///   each target: `code -g {file}:{line}:{col}` gives `code -g a:1:2 -g b:3:4`.
/// - With `stdin`, the command gets no file arguments.
fn args(launch: &Launch) -> Result<Vec<String>> {
    let cmd = &launch.cmd;
    let mut words = cmd.split_whitespace().map(String::from);
    let program = words.next().context("the command is empty")?;
    let rest: Vec<String> = words.collect();
    let mut args = vec![program];
    if PLACEHOLDERS.iter().any(|p| cmd.contains(p)) {
        for t in &launch.targets {
            args.extend(rest.iter().map(|w| fill(w, t)));
        }
        return Ok(args);
    }
    args.extend(rest);
    if launch.stdin.is_some() {
        return Ok(args);
    }
    match launch.targets.as_slice() {
        [t] => args.extend([format!("+{}", t.line), t.path.to_string()]),
        targets => {
            let file = env::temp_dir().join(format!("qf-{}.qf", std::process::id()));
            let list: String = targets
                .iter()
                .map(|t| format!("{}:{}:{}: {}\n", t.path, t.line, t.col, t.text))
                .collect();
            fs::write(&file, list).context("failed to write quickfix file")?;
            args.extend(["-q".to_string(), file.display().to_string()]);
        }
    }
    Ok(args)
}

/// The command's input: the `stdin` template once per target, one per line.
fn input(template: &str, targets: &[Target]) -> String {
    targets.iter().map(|t| fill(template, t) + "\n").collect()
}

/// Start the command with its input piped in.
fn start(launch: &Launch, args: &[String], quiet: bool) -> Result<(Child, Option<String>)> {
    let mut command = Command::new(&args[0]);
    command.args(&args[1..]);
    let text = launch.stdin.as_deref().map(|t| input(t, &launch.targets));
    // A command in the terminal (not `quiet`) without `stdin` reads the terminal.
    if text.is_some() {
        command.stdin(Stdio::piped());
    } else if quiet {
        command.stdin(Stdio::null());
    }
    if quiet {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let child = command
        .spawn()
        .with_context(|| format!("failed to start {}", args[0]))?;
    Ok((child, text))
}

fn feed(child: &mut Child, text: Option<String>) {
    if let (Some(mut pipe), Some(text)) = (child.stdin.take(), text) {
        let _ = pipe.write_all(text.as_bytes());
    }
}

/// Run the command in the background while `qf` stays open. Its output goes nowhere,
/// so it cannot draw over the screen.
pub fn spawn(launch: &Launch) -> Result<()> {
    let args = args(launch)?;
    let (mut child, text) = start(launch, &args, true)?;
    // Feed and reap the child on a thread, so a slow command does not block the UI.
    thread::spawn(move || {
        feed(&mut child, text);
        let _ = child.wait();
    });
    Ok(())
}

/// Run the command after the TUI is closed. Without `stdin`, replace this process with
/// it, so quitting the editor returns to the shell.
pub fn run(launch: &Launch) -> Result<()> {
    let args = args(launch)?;
    if launch.stdin.is_none() {
        let err = Command::new(&args[0]).args(&args[1..]).exec();
        return Err(err).with_context(|| format!("failed to start {}", launch.cmd));
    }
    wait(launch)
}

/// Run the command in the terminal and wait for it to exit. The caller closes the
/// TUI first and shows it again after.
pub fn wait(launch: &Launch) -> Result<()> {
    let args = args(launch)?;
    let (mut child, text) = start(launch, &args, false)?;
    feed(&mut child, text);
    child.wait().with_context(|| format!("failed to run {}", launch.cmd))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn target(path: &str, line: u64, col: u32) -> Target {
        Target {
            path: Arc::from(path),
            line,
            col,
            text: format!("text {line}"),
        }
    }

    fn launch(cmd: &str, stdin: Option<&str>, targets: &[Target]) -> Launch {
        Launch {
            cmd: cmd.into(),
            stdin: stdin.map(String::from),
            targets: targets.to_vec(),
        }
    }

    fn editor_args(cmd: &str, targets: &[Target]) -> Vec<String> {
        args(&launch(cmd, None, targets)).unwrap()
    }

    #[test]
    fn editor_commands() {
        let one = [target("src/a.rs", 12, 5)];
        assert_eq!(editor_args("nvim", &one), ["nvim", "+12", "src/a.rs"]);
        assert_eq!(editor_args("vim -p", &one), ["vim", "-p", "+12", "src/a.rs"]);
        assert_eq!(
            editor_args("devin -g {file}:{line}:{col}", &one),
            ["devin", "-g", "src/a.rs:12:5"]
        );
        let two = [target("a.rs", 1, 2), target("b.rs", 3, 4)];
        assert_eq!(
            editor_args("code -g {file}:{line}:{col}", &two),
            ["code", "-g", "a.rs:1:2", "-g", "b.rs:3:4"]
        );
        assert_eq!(editor_args("hx {file}:{line}", &two), ["hx", "a.rs:1", "b.rs:3"]);
        assert_eq!(editor_args("echo {text}", &one), ["echo", "text 12"]);
        let qf = editor_args("nvim", &two);
        assert_eq!(qf[..2], ["nvim", "-q"]);
    }

    #[test]
    fn stdin_commands() {
        let two = [target("a.rs", 1, 2), target("b.rs", 3, 4)];
        assert_eq!(
            args(&launch("pbcopy", Some("{file}:{line}"), &two)).unwrap(),
            ["pbcopy"]
        );
        assert_eq!(input("{file}:{line}", &two), "a.rs:1\nb.rs:3\n");
        assert_eq!(input("{text}", &two[..1]), "text 1\n");
    }
}
