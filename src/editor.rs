//! One-line text editor for the prompt, with vim-style motions.
//! The cursor is a char index in `0..=len`.

#[derive(Default)]
pub struct LineEditor {
    chars: Vec<char>,
    cursor: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Blank,
    Word,
    Punct,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Blank
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

impl LineEditor {
    /// Text with the cursor at the end.
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let cursor = chars.len();
        Self { chars, cursor }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn chars(&self) -> &[char] {
        &self.chars
    }

    pub fn char_at(&self, i: usize) -> Option<char> {
        self.chars.get(i).copied()
    }

    /// Replace `a..b` with `text`. The cursor goes to the end of the new text.
    pub fn replace_range(&mut self, a: usize, b: usize, text: &str) {
        let b = b.min(self.chars.len());
        let new: Vec<char> = text.chars().collect();
        self.cursor = a + new.len();
        self.chars.splice(a..b, new);
    }

    /// Uppercase or lowercase `a..b`. Returns true if text changed.
    pub fn change_case(&mut self, a: usize, b: usize, upper: bool) -> bool {
        let (a, b) = (a.min(b), a.max(b).min(self.chars.len()));
        let old: String = self.chars[a..b].iter().collect();
        let new = if upper { old.to_uppercase() } else { old.to_lowercase() };
        if new == old {
            return false;
        }
        self.replace_range(a, b, &new);
        self.cursor = a;
        true
    }

    /// vim `iw` (`around`: `aw`): the word, punctuation run or space run at `i`.
    /// `aw` adds the spaces after it, or before it if there are none after.
    pub fn word_object(&self, i: usize, around: bool) -> Option<(usize, usize)> {
        let n = self.chars.len();
        if i >= n {
            return None;
        }
        let cls = class(self.chars[i]);
        let (mut a, mut b) = (i, i + 1);
        while a > 0 && class(self.chars[a - 1]) == cls {
            a -= 1;
        }
        while b < n && class(self.chars[b]) == cls {
            b += 1;
        }
        if around && cls != Class::Blank {
            let after = b;
            while b < n && class(self.chars[b]) == Class::Blank {
                b += 1;
            }
            if b == after {
                while a > 0 && class(self.chars[a - 1]) == Class::Blank {
                    a -= 1;
                }
            }
        }
        Some((a, b))
    }

    pub fn move_to(&mut self, pos: usize) {
        self.cursor = pos.min(self.chars.len());
    }

    /// Normal mode keeps the cursor on a char, not after the last one.
    pub fn clamp_normal(&mut self) {
        if self.cursor >= self.chars.len() {
            self.cursor = self.chars.len().saturating_sub(1);
        }
    }

    pub fn insert(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) -> bool {
        let start = self.cursor.saturating_sub(1);
        self.delete_range(start, self.cursor)
    }

    /// Delete `a..b` (either order) and put the cursor at the start. Returns true if text changed.
    pub fn delete_range(&mut self, a: usize, b: usize) -> bool {
        let (a, b) = (a.min(b), a.max(b).min(self.chars.len()));
        if a >= b {
            return false;
        }
        self.chars.drain(a..b);
        self.cursor = a;
        true
    }

    pub fn first_non_blank(&self) -> usize {
        self.chars.iter().position(|c| !c.is_whitespace()).unwrap_or(0)
    }

    /// vim `w`: start of the next word.
    pub fn word_forward(&self) -> usize {
        let n = self.chars.len();
        let mut i = self.cursor;
        if i >= n {
            return n;
        }
        let cls = class(self.chars[i]);
        if cls != Class::Blank {
            while i < n && class(self.chars[i]) == cls {
                i += 1;
            }
        }
        while i < n && class(self.chars[i]) == Class::Blank {
            i += 1;
        }
        i
    }

    /// vim `b`: start of this or the previous word.
    pub fn word_backward(&self) -> usize {
        let mut i = self.cursor.min(self.chars.len());
        while i > 0 && class(self.chars[i - 1]) == Class::Blank {
            i -= 1;
        }
        if i == 0 {
            return 0;
        }
        let cls = class(self.chars[i - 1]);
        while i > 0 && class(self.chars[i - 1]) == cls {
            i -= 1;
        }
        i
    }

    /// Readline `Ctrl-W`: start of the blank-separated word before the cursor.
    pub fn blank_word_start(&self) -> usize {
        let mut i = self.cursor.min(self.chars.len());
        while i > 0 && self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    /// vim `e`: last char of this or the next word.
    pub fn word_end(&self) -> usize {
        let n = self.chars.len();
        if n == 0 {
            return 0;
        }
        let mut i = self.cursor + 1;
        while i < n && class(self.chars[i]) == Class::Blank {
            i += 1;
        }
        if i >= n {
            return n - 1;
        }
        let cls = class(self.chars[i]);
        while i + 1 < n && class(self.chars[i + 1]) == cls {
            i += 1;
        }
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, cursor: usize) -> LineEditor {
        let mut e = LineEditor::new(text);
        e.move_to(cursor);
        e
    }

    #[test]
    fn motions() {
        let e = at("foo.bar  baz", 0);
        assert_eq!(e.word_forward(), 3);
        assert_eq!(e.word_end(), 2);
        assert_eq!(at("foo.bar  baz", 4).word_forward(), 9);
        assert_eq!(at("foo.bar  baz", 9).word_backward(), 4);
        assert_eq!(at("foo.bar  baz", 12).word_backward(), 9);
        assert_eq!(at("foo.bar  baz", 6).word_end(), 11);
        assert_eq!(at("  foo", 4).first_non_blank(), 2);
    }

    #[test]
    fn editing() {
        let mut e = at("hello world", 11);
        let to = e.word_backward();
        assert!(e.delete_range(to, e.cursor()));
        assert_eq!(e.text(), "hello ");
        e.insert('x');
        assert_eq!(e.text(), "hello x");
        assert!(e.backspace());
        e.clamp_normal();
        assert_eq!(e.cursor(), 5);
    }
}
