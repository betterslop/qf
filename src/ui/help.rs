//! Key reference: the help popup and `qf --help` show the same text.

use ratatui::Frame;
use ratatui::text::{Line, Text};

use bs_tui_common::help::Help;

use crate::app::App;

pub const KEYS: &str = "\
Insert mode (fzf and readline keys)
  Enter              open hit, or all marked hits in the quickfix list
  Tab / Shift-Tab    mark hit and go to the next / previous hit
  C-q                all hits to the quickfix list
  C-o                menu of the [open.NAME] commands from the config
                     (a command with a key = \"ctrl-x\" also runs on that key)
  Up/Down  C-k/C-j   move selection (as on the screen)    PgUp/PgDn  page
  Shift-Up/Down      scroll the preview
  C-/                show / hide the preview
  C-a / C-e          start / end of line    C-b / C-f  back / forward one char
  Home / End         start / end of line    Left / Right  back / forward one char
  Backspace / C-h    delete before the cursor    Delete  delete under the cursor
  C-w / C-u          delete word / delete to start        C-l  clear the query
  C-p / C-n          older / newer query    C-r  search the history
  C-Space            complete the word: fields, values, paths, words
  @                  pick a file or folder (fuzzy): @uilist -> path:src/ui/list.rs
                     Esc keeps @ as text, so @Override still searches
  path: ext: type: git: sort:   open a menu of values
  :                  at an empty prompt: command line
  ?                  at an empty prompt: this help
  ( \" ' /            add the closing character (config auto_pairs)
  Esc                normal mode           C-c  quit         F1  help

Open menu (C-o)
  j k  Up/Down       move   (also C-j C-k)      Enter  run the selected command
  a letter           run the command with that letter    Esc q  close

Menus (completion, history, commands)
  Up/Down C-k/C-j    move (as on the screen)    C-n / C-p  next / previous item
  Tab or Enter       accept                     Esc        close the menu

Normal mode (vim keys)
  i a I A            insert
  h l 0 ^ $ w b e    move cursor    Left Right Home End Backspace  also move
  /                  type again: insert at the end of the query
  Esc                clear the query (one step; it never quits)
  x X D C s S        edit
  d{motion} c{motion} dd cc
  ci\" di( da' ciw   change / delete in or around \"\" '' // () or a word
  %                  jump to the matching parenthesis
  gU{motion} gu{motion}  uppercase / lowercase, for example gUiw: or -> OR
  j k                move selection    gg G  top / bottom row of the list
  R                  turn on / off the new search after a file changes (:set watch)
  C-d C-u C-f C-b    half page / page
  C-e / C-y          scroll the preview down / up
  zh / zl            scroll the preview left / right
  :                  command line (see Commands)
  ?  F1              this help         q  quit (also :q, C-c)
  Enter Tab Up/Down  as in insert mode, also C-o C-q C-/ Shift-Up/Down

Command line (: on the status bar row; its menu is above it, so Up is next)
  C-a C-e C-b C-f Home End Left Right   move      Backspace C-h Delete C-w C-u  delete
  Tab                accept a menu item     Enter  run a complete command
  Enter in the menu  accept an argument and run; complete a name that takes one
  C-Space            reopen the completion menu
  Esc                close the line and keep its text; the next : opens with it
  Backspace Enter    in an empty line, close it     F1  help     C-c  quit

Help popup
  j k Down Up  C-d C-u  C-f C-b  PgDn PgUp   scroll     g G  top / bottom
  F1 ? q Esc         close

Mouse                wheel scrolls the list or the preview, click selects

Commands (: in normal mode or at an empty prompt; Tab completes;
          Enter runs a complete command; short forms work: :so m;
          :theme :help :q are the same in every betterslop tool)
  Settings have no Alt keys: use these commands or the config file.
  :sort relevance|path|modified   :context N   :mode query|regex|file
  :case smart|ignore       :changed            :theme dark|light
  :layout auto|horizontal|vertical             :split N    :prompt bottom|top
  :set [no]wrap [no]group [no]preview [no]scrollbar [no]header [no]ivy
  :marks clear|all         :open [NAME]        :help    :q

Query syntax
  foo bar            both on the line (AND)
  foo OR bar   (a OR b) AND c   a b | c  (| binds tighter: a AND (b OR c))
  foo -bar   foo !bar   NOT foo
  \"literal\"  'literal'  'fzf-exact   /regex/ (can have spaces)   ^anchor$
  exacT              a capital letter makes a term case-sensitive
  and / or / not between terms become AND / OR / NOT as you type (config
  auto_uppercase). To search for the word, quote it: \"or\"

Fields (filter files; combine with AND, OR, NOT, -)
  path:src/ui        path matches the word (regex or literal, smart case)
  path:\"a.b\"        path contains the exact text (quoted is never a glob)
  path:*.rs          unquoted with * ? [ {: a glob, path:*.{rs,toml}
  path:/ui.*rs$/     path matches a regex
  A partial value (git:c) matches nothing and shows a hint, not an error
  ext:rs             file extension
  type:rust          ripgrep file type (rg --type-list)
  git:changed        changed in git; also git:modified, git:staged, git:untracked
  sort:path          by path; also sort:modified (newest first). Default: relevance
  A query with only fields lists the matching files, for example: ext:toml git:changed";

/// Draw the help popup if it is open. Clamps the scroll to the text.
pub fn draw(f: &mut Frame, app: &mut App) {
    let Some(scroll) = app.help else { return };
    let text: Text = KEYS.lines().map(Line::raw).collect();
    let max = (text.height() as u16).saturating_sub(f.area().height.saturating_sub(2));
    let scroll = scroll.min(max);
    app.help = Some(scroll);
    // The shared popup (bs-tui-common), so help looks the same in every tool.
    let popup = Help::text("qf keys", text).scroll(scroll);
    f.render_widget(popup, f.area());
}
