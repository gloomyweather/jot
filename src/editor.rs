//! A small multi-line text editor with soft wrapping, undo and list continuation.

use unicode_width::UnicodeWidthChar;

pub const TAB_WIDTH: usize = 4;
const UNDO_LIMIT: usize = 300;

#[derive(Clone)]
struct Snapshot {
    lines: Vec<String>,
    row: usize,
    col: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
    Other,
}

/// A visual row: part `start..end` (in chars) of logical line `line`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub line: usize,
    pub start: usize,
    pub end: usize,
}

pub struct Editor {
    lines: Vec<String>,
    row: usize,
    /// Cursor position in chars within the line.
    col: usize,
    /// Visual column to come back to when moving up and down.
    goal_x: Option<usize>,
    pub scroll: usize,
    /// Wrap width and visible height, set by the renderer.
    pub width: usize,
    pub height: usize,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_kind: EditKind,
    /// Edited since the last save.
    pub changed: bool,
}

pub fn char_width(c: char) -> usize {
    if c == '\t' {
        TAB_WIDTH
    } else {
        c.width().unwrap_or(0)
    }
}

/// Split a line into visual rows of at most `width` columns, breaking after spaces
/// where possible. A space that would overflow stays at the end of its row.
pub fn wrap(line: &str, width: usize) -> Vec<(usize, usize)> {
    let width = width.max(1);
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return vec![(0, 0)];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let (mut end, mut w, mut last_space) = (start, 0, None);
        while end < chars.len() {
            let cw = char_width(chars[end]);
            if w + cw > width {
                break;
            }
            w += cw;
            if chars[end] == ' ' {
                last_space = Some(end);
            }
            end += 1;
        }
        if end < chars.len() {
            // An overflowing space stays on this row; a char wider than the row gets one of its own.
            if chars[end] == ' ' || end == start {
                end += 1;
            } else if let Some(sp) = last_space {
                end = sp + 1;
            }
        }
        out.push((start, end));
        start = end;
    }
    out
}

/// For a list line, the prefix to continue it with on the next line and the
/// length of its own prefix: `- `, `* `, `- [ ] `, `> `, `1. `.
fn list_prefix(line: &str) -> Option<(String, usize)> {
    let body = line.trim_start();
    let indent = &line[..line.len() - body.len()];
    let ilen = indent.chars().count();
    for (marker, next) in [
        ("- [ ] ", "- [ ] "),
        ("- [x] ", "- [ ] "),
        ("- [X] ", "- [ ] "),
        ("- ", "- "),
        ("* ", "* "),
        ("+ ", "+ "),
        ("> ", "> "),
    ] {
        if body.starts_with(marker) {
            return Some((format!("{indent}{next}"), ilen + marker.chars().count()));
        }
    }
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && digits < 6 && body[digits..].starts_with(". ") {
        let n: u32 = body[..digits].parse().ok()?;
        return Some((format!("{indent}{}. ", n + 1), ilen + digits + 2));
    }
    None
}

#[derive(PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Punct,
}

fn class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

impl Editor {
    /// A new editor with the cursor at the end of the text.
    pub fn new(text: &str) -> Self {
        let lines: Vec<String> = text.replace("\r\n", "\n").split('\n').map(String::from).collect();
        let row = lines.len() - 1;
        let col = lines[row].chars().count();
        Self {
            lines,
            row,
            col,
            goal_x: None,
            scroll: 0,
            width: 80,
            height: 20,
            undo: Vec::new(),
            redo: Vec::new(),
            last_kind: EditKind::Other,
            changed: false,
        }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn is_blank(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }

    pub fn words(&self) -> usize {
        self.lines.iter().map(|l| l.split_whitespace().count()).sum()
    }

    fn len(&self, row: usize) -> usize {
        self.lines[row].chars().count()
    }

    fn byte(&self, row: usize, col: usize) -> usize {
        self.lines[row]
            .char_indices()
            .nth(col)
            .map_or(self.lines[row].len(), |(i, _)| i)
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            lines: self.lines.clone(),
            row: self.row,
            col: self.col,
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.lines = s.lines;
        self.row = s.row;
        self.col = s.col;
        self.goal_x = None;
        self.last_kind = EditKind::Other;
        self.changed = true;
    }

    /// Record an undo point unless this edit continues the previous one.
    fn checkpoint(&mut self, kind: EditKind) {
        if kind != self.last_kind || kind == EditKind::Other {
            self.undo.push(self.snapshot());
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.last_kind = kind;
        self.goal_x = None;
        self.changed = true;
    }

    pub fn undo(&mut self) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        self.redo.push(self.snapshot());
        self.restore(s);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        self.undo.push(self.snapshot());
        self.restore(s);
        true
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' {
            return self.newline();
        }
        self.checkpoint(EditKind::Insert);
        let b = self.byte(self.row, self.col);
        self.lines[self.row].insert(b, c);
        self.col += 1;
        // Undo a word at a time.
        if c.is_whitespace() {
            self.last_kind = EditKind::Other;
        }
    }

    /// Insert pasted text as a single undo step.
    pub fn insert_str(&mut self, s: &str) {
        self.checkpoint(EditKind::Other);
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        for (i, part) in s.split('\n').enumerate() {
            if i > 0 {
                let b = self.byte(self.row, self.col);
                let rest = self.lines[self.row].split_off(b);
                self.row += 1;
                self.lines.insert(self.row, rest);
                self.col = 0;
            }
            let b = self.byte(self.row, self.col);
            self.lines[self.row].insert_str(b, part);
            self.col += part.chars().count();
        }
    }

    /// Split the line; inside a list, start the next item. Enter on an empty
    /// item ends the list instead.
    pub fn newline(&mut self) {
        self.checkpoint(EditKind::Other);
        let prefix = list_prefix(&self.lines[self.row]).filter(|(_, own)| self.col >= *own);
        if let Some((_, own)) = &prefix {
            if *own == self.len(self.row) {
                // Only the marker on this line: drop it.
                self.lines[self.row].clear();
                self.col = 0;
                return;
            }
        }
        let b = self.byte(self.row, self.col);
        let rest = self.lines[self.row].split_off(b);
        let prefix = prefix.map(|(p, _)| p).unwrap_or_default();
        self.col = prefix.chars().count();
        self.row += 1;
        self.lines.insert(self.row, prefix + &rest);
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.checkpoint(EditKind::Delete);
            let b = self.byte(self.row, self.col - 1);
            self.lines[self.row].remove(b);
            self.col -= 1;
        } else if self.row > 0 {
            self.checkpoint(EditKind::Other);
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.len(self.row);
            self.lines[self.row].push_str(&line);
        }
    }

    pub fn delete(&mut self) {
        if self.col < self.len(self.row) {
            self.checkpoint(EditKind::Delete);
            let b = self.byte(self.row, self.col);
            self.lines[self.row].remove(b);
        } else if self.row + 1 < self.lines.len() {
            self.checkpoint(EditKind::Other);
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        }
    }

    pub fn delete_word_back(&mut self) {
        let start = self.word_left_col();
        if start == self.col {
            return self.backspace();
        }
        self.checkpoint(EditKind::Other);
        let (a, b) = (self.byte(self.row, start), self.byte(self.row, self.col));
        self.lines[self.row].replace_range(a..b, "");
        self.col = start;
    }

    /// Indent (or with `outdent`, unindent) the current line by two spaces.
    pub fn indent(&mut self, outdent: bool) {
        let line = &self.lines[self.row];
        if outdent {
            let n = line.chars().take(2).take_while(|c| *c == ' ').count();
            if n == 0 {
                return;
            }
            self.checkpoint(EditKind::Other);
            self.lines[self.row].replace_range(..n, "");
            self.col = self.col.saturating_sub(n);
        } else {
            let is_list = list_prefix(line).is_some();
            self.checkpoint(EditKind::Other);
            if is_list {
                self.lines[self.row].insert_str(0, "  ");
            } else {
                let b = self.byte(self.row, self.col);
                self.lines[self.row].insert_str(b, "  ");
            }
            self.col += 2;
        }
    }

    /// Cycle the current line through `- [ ] ` → `- [x] ` → `- [ ] `, adding a
    /// checkbox if it has none.
    pub fn toggle_checkbox(&mut self) {
        self.checkpoint(EditKind::Other);
        let line = &self.lines[self.row];
        let body = line.trim_start();
        let at = line.len() - body.len();
        let (from, to) = if body.starts_with("- [ ] ") {
            (6, "- [x] ")
        } else if body.starts_with("- [x] ") || body.starts_with("- [X] ") {
            (6, "- [ ] ")
        } else if body.starts_with("- ") || body.starts_with("* ") {
            (2, "- [ ] ")
        } else {
            (0, "- [ ] ")
        };
        // Markers are ASCII, so bytes and chars agree.
        let (from_len, to_len) = (from, to.len());
        self.lines[self.row].replace_range(at..at + from, to);
        let indent = self.lines[self.row][..at].chars().count();
        if self.col >= indent + from_len {
            self.col = self.col + to_len - from_len;
        } else if self.col > indent {
            self.col = indent + to_len;
        }
    }

    // Movement

    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.len(self.row);
        }
        self.moved();
    }

    pub fn right(&mut self) {
        if self.col < self.len(self.row) {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
        self.moved();
    }

    fn word_left_col(&self) -> usize {
        let chars: Vec<char> = self.lines[self.row].chars().take(self.col).collect();
        let mut i = chars.len();
        while i > 0 && class(chars[i - 1]) == CharClass::Space {
            i -= 1;
        }
        if i > 0 {
            let c = class(chars[i - 1]);
            while i > 0 && class(chars[i - 1]) == c {
                i -= 1;
            }
        }
        i
    }

    pub fn word_left(&mut self) {
        if self.col == 0 {
            return self.left();
        }
        self.col = self.word_left_col();
        self.moved();
    }

    pub fn word_right(&mut self) {
        let chars: Vec<char> = self.lines[self.row].chars().collect();
        if self.col >= chars.len() {
            return self.right();
        }
        let mut i = self.col;
        let c = class(chars[i]);
        while i < chars.len() && class(chars[i]) == c {
            i += 1;
        }
        while i < chars.len() && class(chars[i]) == CharClass::Space {
            i += 1;
        }
        self.col = i;
        self.moved();
    }

    pub fn home(&mut self) {
        // First to the start of the text, then to column 0.
        let indent = self.lines[self.row]
            .chars()
            .take_while(|c| c.is_whitespace())
            .count();
        self.col = if self.col > indent { indent } else { 0 };
        self.moved();
    }

    pub fn end(&mut self) {
        self.col = self.len(self.row);
        self.moved();
    }

    pub fn doc_start(&mut self) {
        (self.row, self.col) = (0, 0);
        self.moved();
    }

    pub fn doc_end(&mut self) {
        self.row = self.lines.len() - 1;
        self.col = self.len(self.row);
        self.moved();
    }

    fn moved(&mut self) {
        self.goal_x = None;
        self.last_kind = EditKind::Other;
    }

    /// Move by `delta` visual rows, keeping the column.
    pub fn vertical(&mut self, delta: isize) {
        let rows = self.rows();
        let (vrow, x) = self.cursor_visual(&rows);
        let goal = *self.goal_x.get_or_insert(x);
        let target = (vrow as isize + delta).clamp(0, rows.len() as isize - 1) as usize;
        if target == vrow {
            // Already on the first or last row: go to its edge.
            if delta < 0 {
                self.col = rows[vrow].start;
            } else {
                self.col = rows[vrow].end;
            }
            self.row = rows[vrow].line;
            self.last_kind = EditKind::Other;
            return;
        }
        let r = rows[target];
        let last_of_line = rows.get(target + 1).is_none_or(|n| n.line != r.line);
        let mut col = r.start;
        let mut w = 0;
        for c in self.lines[r.line].chars().skip(r.start).take(r.end - r.start) {
            let cw = char_width(c);
            if w + cw > goal {
                break;
            }
            w += cw;
            col += 1;
        }
        // The end of a wrapped row is the start of the next one.
        if !last_of_line && col >= r.end {
            col = r.end - 1;
        }
        self.row = r.line;
        self.col = col;
        self.last_kind = EditKind::Other;
    }

    pub fn page(&mut self, down: bool) {
        let n = self.height.max(2) as isize - 1;
        self.vertical(if down { n } else { -n });
    }

    // Layout

    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            for (start, end) in wrap(line, self.wrap_width()) {
                rows.push(Row { line: i, start, end });
            }
        }
        rows
    }

    /// One column is kept free so the cursor fits after the last character.
    fn wrap_width(&self) -> usize {
        self.width.saturating_sub(1).max(1)
    }

    /// The cursor's visual row and column.
    pub fn cursor_visual(&self, rows: &[Row]) -> (usize, usize) {
        let mut found = 0;
        for (i, r) in rows.iter().enumerate() {
            if r.line == self.row && r.start <= self.col {
                found = i;
            }
            if r.line > self.row {
                break;
            }
        }
        let r = rows[found];
        let x = self.lines[r.line]
            .chars()
            .skip(r.start)
            .take(self.col - r.start)
            .map(char_width)
            .sum();
        (found, x)
    }

    /// Scroll so the cursor is visible; returns the cursor's position on screen.
    pub fn scroll_to_cursor(&mut self, rows: &[Row]) -> (usize, usize) {
        let (vrow, x) = self.cursor_visual(rows);
        let h = self.height.max(1);
        if vrow < self.scroll {
            self.scroll = vrow;
        } else if vrow >= self.scroll + h {
            self.scroll = vrow + 1 - h;
        }
        self.scroll = self.scroll.min(rows.len().saturating_sub(1));
        (vrow - self.scroll, x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Editor {
        let mut e = Editor::new("");
        for c in text.chars() {
            e.insert_char(c);
        }
        e
    }

    #[test]
    fn wrapping() {
        assert_eq!(wrap("", 10), vec![(0, 0)]);
        assert_eq!(wrap("hello world foo", 11), vec![(0, 12), (12, 15)]);
        assert_eq!(wrap("abcdefgh", 3), vec![(0, 3), (3, 6), (6, 8)]);
        // Wide characters take two columns.
        assert_eq!(wrap("日本語", 4), vec![(0, 2), (2, 3)]);
    }

    #[test]
    fn lists_continue_and_end() {
        let mut e = typed("- milk\n");
        assert_eq!(e.text(), "- milk\n- ");
        e.newline();
        assert_eq!(e.text(), "- milk\n");
        let e = typed("1. one\n");
        assert_eq!(e.text(), "1. one\n2. ");
        let e = typed("  - [x] done\n");
        assert_eq!(e.text(), "  - [x] done\n  - [ ] ");
    }

    #[test]
    fn checkbox_toggle() {
        let mut e = typed("milk");
        e.toggle_checkbox();
        assert_eq!(e.text(), "- [ ] milk");
        e.toggle_checkbox();
        assert_eq!(e.text(), "- [x] milk");
        e.toggle_checkbox();
        assert_eq!(e.text(), "- [ ] milk");
        assert_eq!(e.col, 10);
    }

    #[test]
    fn undo_by_word() {
        let mut e = typed("hello big world");
        assert!(e.undo());
        assert_eq!(e.text(), "hello big ");
        assert!(e.undo());
        assert_eq!(e.text(), "hello ");
        assert!(e.redo());
        assert_eq!(e.text(), "hello big ");
    }

    #[test]
    fn unicode_editing() {
        let mut e = typed("привет мир");
        e.word_left();
        e.delete_word_back();
        assert_eq!(e.text(), "мир");
        e.end();
        e.backspace();
        assert_eq!(e.text(), "ми");
    }

    #[test]
    fn vertical_moves_through_wrapped_rows() {
        let mut e = Editor::new("aaaa bbbb cccc\nxy");
        e.width = 6;
        e.doc_start();
        e.right();
        e.right();
        e.vertical(1);
        assert_eq!((e.row, e.col), (0, 7));
        e.vertical(2);
        assert_eq!((e.row, e.col), (1, 2));
        e.vertical(-1);
        assert_eq!((e.row, e.col), (0, 12));
    }
}
