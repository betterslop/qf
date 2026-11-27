//! One pass over the prompt text for the editor and the UI: a class per char (for
//! colors), matched parentheses, quoted spans, and problems such as `missing )`.
//! The rules follow the parser (`bs-tui-common/QUERY.md`): `(` opens a group only at the
//! start of a term (`foo(` is text), and `'` and `/` start a quote or regex only at the
//! start of a term. Their closing `'` or `/` must end the term.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Plain,
    /// `AND`, `OR`, `|`
    Op,
    /// `NOT`, `-`, `!`
    Negate,
    /// `"…"` or `'…'`
    Quoted,
    /// `/…/`
    Regex,
    Paren,
}

pub struct Analysis {
    pub classes: Vec<Class>,
    /// Matched parentheses: (open, close) char indices.
    pub pairs: Vec<(usize, usize)>,
    /// Closed quotes and regexes: (open, close) char indices and the delimiter.
    pub quotes: Vec<(usize, usize, char)>,
    /// Problems: char index and message.
    pub errors: Vec<(usize, &'static str)>,
    /// Words: (start, end) char indices, end exclusive.
    words: Vec<(usize, usize)>,
}

fn at_term_start(chars: &[char], i: usize) -> bool {
    i == 0
        || matches!(chars[i - 1], '(' | ':')
        || chars[i - 1].is_whitespace()
        || (matches!(chars[i - 1], '-' | '!') && at_term_start(chars, i - 1))
}

/// The closing `'` or `/` for the one at `open`: the first one that ends the term.
fn closing(chars: &[char], open: usize, in_group: bool) -> Option<usize> {
    let ends = |j: usize| j >= chars.len() || chars[j].is_whitespace() || (chars[j] == ')' && in_group);
    let mut j = open + 1;
    while j < chars.len() {
        if chars[j] == '\\' {
            j += 2;
        } else if chars[j] == chars[open] && ends(j + 1) {
            return Some(j);
        } else {
            j += 1;
        }
    }
    None
}

pub fn analyze(chars: &[char]) -> Analysis {
    let n = chars.len();
    let mut a = Analysis {
        classes: vec![Class::Plain; n],
        pairs: Vec::new(),
        quotes: Vec::new(),
        errors: Vec::new(),
        words: Vec::new(),
    };
    let mut stack = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        let start = at_term_start(chars, i);
        if c.is_whitespace() {
            i += 1;
        } else if c == '"' || ((c == '\'' || c == '/') && start && closing(chars, i, !stack.is_empty()).is_some()) {
            let open = i;
            let close = if c == '"' {
                i += 1;
                while i < n && chars[i] != c {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
                (i < n).then_some(i)
            } else {
                closing(chars, i, !stack.is_empty())
            };
            let class = if c == '/' { Class::Regex } else { Class::Quoted };
            match close {
                Some(close) => {
                    a.quotes.push((open, close, c));
                    i = close + 1;
                }
                None => a.errors.push((open, "unclosed \"")),
            }
            a.classes[open..i.min(n)].fill(class);
        } else if c == '\'' && start {
            // fzf exact match: `'foo` with no closing quote is literal text.
            let from = i;
            while i < n && !chars[i].is_whitespace() && !(chars[i] == ')' && !stack.is_empty()) {
                i += 1;
            }
            a.classes[from..i].fill(Class::Quoted);
            a.words.push((from, i));
        } else if c == '(' && start {
            stack.push(i);
            a.classes[i] = Class::Paren;
            i += 1;
        } else if c == ')' {
            match stack.pop() {
                Some(open) => a.pairs.push((open, i)),
                None => a.errors.push((i, "unmatched )")),
            }
            a.classes[i] = Class::Paren;
            i += 1;
        } else if (c == '-' || c == '!')
            && start
            && chars
                .get(i + 1)
                .is_some_and(|&n| !n.is_whitespace() && n != c && n != ')')
        {
            a.classes[i] = Class::Negate;
            i += 1;
        } else {
            // A word ends at a space, or at `)` that closes an open group.
            let from = i;
            while i < n && !chars[i].is_whitespace() && !(chars[i] == ')' && !stack.is_empty()) {
                if chars[i] == '"' {
                    break;
                }
                i += 1;
            }
            let word: String = chars[from..i].iter().collect();
            let class = match word.as_str() {
                "AND" | "OR" | "|" => Class::Op,
                "NOT" => Class::Negate,
                _ => Class::Plain,
            };
            a.classes[from..i].fill(class);
            a.words.push((from, i));
        }
    }
    for open in stack {
        a.errors.push((open, "missing )"));
    }
    a.errors.sort_unstable();
    a.pairs.sort_unstable();
    a
}

impl Analysis {
    /// The matching parenthesis of the one at `i`.
    pub fn partner(&self, i: usize) -> Option<usize> {
        self.pairs.iter().find_map(|&(o, c)| {
            if o == i {
                Some(c)
            } else if c == i {
                Some(o)
            } else {
                None
            }
        })
    }

    /// The innermost parenthesis pair around `i` (or with `i` on a parenthesis).
    pub fn enclosing_pair(&self, i: usize) -> Option<(usize, usize)> {
        self.pairs
            .iter()
            .filter(|&&(o, c)| o <= i && i <= c)
            .min_by_key(|&&(o, c)| c - o)
            .copied()
    }

    /// The quote (`"`, `'` or `/`) span around `i`, else the next one after `i`, like vim.
    pub fn quote_span(&self, i: usize, delim: char) -> Option<(usize, usize)> {
        let spans = self.quotes.iter().filter(|q| q.2 == delim);
        spans
            .clone()
            .find(|q| q.0 <= i && i <= q.1)
            .or_else(|| spans.clone().find(|q| q.0 > i))
            .map(|q| (q.0, q.1))
    }

    /// The first problem to show in the status line: a syntax error, or a hint for a
    /// lowercase operator between terms.
    pub fn problem(&self, chars: &[char]) -> Option<String> {
        if let Some((_, msg)) = self.errors.first() {
            return Some((*msg).to_string());
        }
        let words: Vec<String> = self.words.iter().map(|&(s, e)| chars[s..e].iter().collect()).collect();
        let last = words.len().checked_sub(1)?;
        words.iter().enumerate().find_map(|(k, w)| {
            let between = k < last && (k > 0 || w == "not");
            matches!(w.as_str(), "and" | "or" | "not")
                .then_some(())
                .filter(|_| between)
                .map(|_| format!("\"{w}\" is a search term; use {}", w.to_uppercase()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &str) -> (Vec<char>, Analysis) {
        let chars: Vec<char> = s.chars().collect();
        let a = analyze(&chars);
        (chars, a)
    }

    #[test]
    fn pairs_and_errors() {
        let (_, a) = run("(foo OR (bar)) \"x y\" /re/");
        assert_eq!(a.pairs, vec![(0, 13), (8, 12)]);
        assert_eq!(a.quotes, vec![(15, 19, '"'), (21, 24, '/')]);
        assert!(a.errors.is_empty());
        assert_eq!(a.partner(13), Some(0));
        assert_eq!(a.enclosing_pair(10), Some((8, 12)));

        let (c, a) = run("(foo \"bar");
        assert_eq!(a.problem(&c).as_deref(), Some("missing )"));
        let (c, a) = run("foo \"bar");
        assert_eq!(a.problem(&c).as_deref(), Some("unclosed \""));
        let (c, a) = run("foo)");
        assert!(a.problem(&c).is_none(), "`foo)` is one word: {:?}", a.errors);
        let (c, a) = run("foo() don't");
        assert!(a.problem(&c).is_none(), "{:?}", a.errors);
    }

    #[test]
    fn operator_hints() {
        let (c, a) = run("foo or bar");
        assert_eq!(a.problem(&c).as_deref(), Some("\"or\" is a search term; use OR"));
        let (c, a) = run("not foo");
        assert!(a.problem(&c).is_some());
        let (c, a) = run("foo and");
        assert!(a.problem(&c).is_none(), "a last word can be a search term");
        let (_, a) = run("foo OR NOT bar");
        assert_eq!(a.classes[4], Class::Op);
        assert_eq!(a.classes[7], Class::Negate);
    }
}
