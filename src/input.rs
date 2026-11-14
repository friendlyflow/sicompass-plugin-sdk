//! The multiline text field behind every `<input>`, as plain data.
//!
//! The app's Insert mode edits `<input>` elements with this model, and a
//! surface the app does not draw (a dashboard) can use the same
//! one instead of growing its own. Sharing it is the point: what the user sees
//! wrap, where the caret is drawn, what a selection covers and where Up/Down go
//! are all read from **one** list of [`InputLine`]s, so they cannot disagree.
//!
//! Nothing here measures text. Positions are **character columns**, which is
//! exact for the app's monospace UI font and for a dashboard's cell grid. The
//! caller produces the visual lines from its own layout (the app from its font
//! wrapper, a cell grid from [`wrap_cells`]) and passes them in.
//!
//! Byte offsets are used for every position into the text, so a caret can be
//! handed straight to `String::insert_str`. Columns are only used to line a
//! caret up with the line above or below it.

/// One visual line of a text field: a byte range into the text, plus how many
/// columns the line is indented by on screen.
///
/// Lines are in order and never overlap. They may leave a gap: the space a word
/// wrap broke at, or the `\n` that ended a line, belongs to no line. A caret on
/// such a gap byte sits at the end of the line before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputLine {
    pub start: usize,
    pub end: usize,
    /// Columns before the first character: a non-editable prefix on the first
    /// line, or a hanging indent on continuation lines.
    pub indent_cols: usize,
}

/// Lines split on `\n` only, with no wrapping. The right answer whenever no
/// visual layout is known yet, since it is always a valid view of the text.
pub fn hard_lines(text: &str) -> Vec<InputLine> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            lines.push(InputLine {
                start,
                end: i,
                indent_cols: 0,
            });
            start = i + 1;
        }
    }
    lines.push(InputLine {
        start,
        end: text.len(),
        indent_cols: 0,
    });
    lines
}

/// Word-wrap `text` for a cell grid.
///
/// The first line holds `first_cols` characters, later lines `rest_cols`, and
/// later lines are indented by `rest_indent`. `\n` always breaks. A word longer
/// than the line is split rather than allowed to run off the edge. Unlike a
/// `split_whitespace` wrap, nothing is dropped: repeated spaces and blank lines
/// survive, and every byte is either inside a line or is the single space or
/// `\n` a line was broken at.
pub fn wrap_cells(
    text: &str,
    first_cols: usize,
    rest_cols: usize,
    rest_indent: usize,
) -> Vec<InputLine> {
    let mut out = Vec::new();
    for hard in hard_lines(text) {
        wrap_one(text, hard, first_cols, rest_cols, rest_indent, &mut out);
    }
    out
}

fn wrap_one(
    text: &str,
    hard: InputLine,
    first_cols: usize,
    rest_cols: usize,
    rest_indent: usize,
    out: &mut Vec<InputLine>,
) {
    let mut start = hard.start;
    loop {
        let (width, indent) = if out.is_empty() {
            (first_cols.max(1), 0)
        } else {
            (rest_cols.max(1), rest_indent)
        };
        let rest = &text[start..hard.end];
        // Byte offset (relative to `start`) just past the `width`-th character.
        let Some((cut, _)) = rest.char_indices().nth(width) else {
            out.push(InputLine {
                start,
                end: hard.end,
                indent_cols: indent,
            });
            return;
        };
        // Break at the last space that still fits, if there is one.
        match rest[..=cut].rfind(' ').filter(|&sp| sp > 0) {
            Some(sp) => {
                out.push(InputLine {
                    start,
                    end: start + sp,
                    indent_cols: indent,
                });
                start += sp + 1;
            }
            None => {
                out.push(InputLine {
                    start,
                    end: start + cut,
                    indent_cols: indent,
                });
                start += cut;
            }
        }
    }
}

/// Index of the visual line `pos` is on.
pub fn line_index(lines: &[InputLine], pos: usize) -> usize {
    lines
        .iter()
        .rposition(|l| l.start <= pos)
        .unwrap_or_default()
}

/// `(line, column)` of `pos`, the column counted in characters from the left
/// edge of the field, indent included. What a caret is drawn from.
pub fn line_of(text: &str, lines: &[InputLine], pos: usize) -> (usize, usize) {
    let Some(line) = lines.get(line_index(lines, pos)) else {
        return (0, 0);
    };
    let to = pos.clamp(line.start, line.end.min(text.len()));
    let col = line.indent_cols + text[line.start..to].chars().count();
    (line_index(lines, pos), col)
}

/// Byte position on `line` closest to display column `col`.
fn pos_at_col(text: &str, line: InputLine, col: usize) -> usize {
    let want = col.saturating_sub(line.indent_cols);
    text[line.start..line.end]
        .char_indices()
        .nth(want)
        .map_or(line.end, |(i, _)| line.start + i)
}

/// Where Up (`down == false`) or Down moves a caret at `pos`.
///
/// The caret keeps its column on the adjacent visual line. `goal` is the column
/// a run of Up/Down presses is aiming for, so passing a short line does not
/// pull the caret left for good. Pass `None` after any other move and keep the
/// returned goal for the next Up/Down. On the first line Up goes to the start
/// of the text, and on the last line Down goes to the end.
pub fn vertical(
    text: &str,
    lines: &[InputLine],
    pos: usize,
    goal: Option<usize>,
    down: bool,
) -> (usize, Option<usize>) {
    if lines.is_empty() {
        return (pos, goal);
    }
    let (li, col) = line_of(text, lines, pos);
    let col = goal.unwrap_or(col);
    let target = if down {
        li + 1
    } else if li == 0 {
        return (0, None);
    } else {
        li - 1
    };
    match lines.get(target) {
        Some(&line) => (pos_at_col(text, line, col), Some(col)),
        None => (text.len(), None),
    }
}

/// Start of the visual line `pos` is on.
pub fn line_home(lines: &[InputLine], pos: usize) -> usize {
    lines.get(line_index(lines, pos)).map_or(0, |l| l.start)
}

/// End of the visual line `pos` is on.
pub fn line_end(lines: &[InputLine], pos: usize) -> usize {
    lines.get(line_index(lines, pos)).map_or(pos, |l| l.end)
}

/// The part of each visual line covered by the byte range `from..to`, as
/// `(line, start, end)` byte ranges. Lines the range only touches at a gap are
/// left out, so a highlight never draws a zero-width sliver.
pub fn selection_spans(lines: &[InputLine], from: usize, to: usize) -> Vec<(usize, usize, usize)> {
    let (from, to) = (from.min(to), from.max(to));
    lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            let s = from.max(l.start);
            let e = to.min(l.end);
            (s < e).then_some((i, s, e))
        })
        .collect()
}

/// What a word move stops at: runs of word characters, runs of punctuation, and
/// the whitespace between them (a line break is whitespace).
#[derive(Clone, Copy, PartialEq, Eq)]
enum WordClass {
    Space,
    Word,
    Punct,
}

fn word_class(c: char) -> WordClass {
    if c.is_whitespace() {
        WordClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        WordClass::Word
    } else {
        WordClass::Punct
    }
}

/// Where Ctrl+Left lands from `pos`: back over any whitespace, then over the run
/// of word characters (or of punctuation) before it, so at the start of a word.
/// A position past the end or inside a character is clamped first.
pub fn word_left(text: &str, pos: usize) -> usize {
    let mut pos = floor_boundary(text, pos);
    let mut chars = text[..pos].char_indices().rev().peekable();
    while let Some(&(i, c)) = chars.peek() {
        if word_class(c) != WordClass::Space {
            break;
        }
        pos = i;
        chars.next();
    }
    let Some(&(_, first)) = chars.peek() else {
        return pos;
    };
    let class = word_class(first);
    for (i, c) in chars {
        if word_class(c) != class {
            break;
        }
        pos = i;
    }
    pos
}

/// Where Ctrl+Right lands from `pos`: over any whitespace, then over the run of
/// word characters (or of punctuation) after it, so at the end of a word, the way
/// text fields on Linux move.
pub fn word_right(text: &str, pos: usize) -> usize {
    let mut pos = floor_boundary(text, pos);
    let mut chars = text[pos..].chars().peekable();
    while let Some(&c) = chars.peek() {
        if word_class(c) != WordClass::Space {
            break;
        }
        pos += c.len_utf8();
        chars.next();
    }
    let Some(&first) = chars.peek() else {
        return pos;
    };
    let class = word_class(first);
    for c in chars {
        if word_class(c) != class {
            break;
        }
        pos += c.len_utf8();
    }
    pos
}

fn floor_boundary(text: &str, pos: usize) -> usize {
    let mut pos = pos.min(text.len());
    while !text.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// A text field's editable state, for a surface that edits text itself.
///
/// The app keeps these three values as separate fields on its renderer and calls
/// the free functions above. A dashboard can hold one of these instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputState {
    pub text: String,
    /// Caret as a byte offset, always on a character boundary.
    pub caret: usize,
    /// The other end of a selection, when there is one.
    pub anchor: Option<usize>,
    /// The column a run of Up/Down presses aims for. See [`vertical`].
    pub goal_col: Option<usize>,
}

impl InputState {
    /// A field holding `text`, caret at the end (`a`) or the start (`i`).
    pub fn new(text: impl Into<String>, caret_at_end: bool) -> Self {
        let text = text.into();
        let caret = if caret_at_end { text.len() } else { 0 };
        Self {
            text,
            caret,
            anchor: None,
            goal_col: None,
        }
    }

    /// The selected byte range, if any and non-empty.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        (a != self.caret).then(|| (a.min(self.caret), a.max(self.caret)))
    }

    fn delete_selection(&mut self) -> bool {
        let Some((s, e)) = self.selection() else {
            self.anchor = None;
            return false;
        };
        self.text.replace_range(s..e, "");
        self.caret = s;
        self.anchor = None;
        true
    }

    fn settle(&mut self) {
        self.anchor = None;
        self.goal_col = None;
    }

    /// Type `s` at the caret, replacing the selection. `"\n"` is a new line.
    pub fn insert_str(&mut self, s: &str) {
        self.delete_selection();
        self.text.insert_str(self.caret, s);
        self.caret += s.len();
        self.goal_col = None;
    }

    /// Delete the selection, or the character before the caret.
    pub fn backspace(&mut self) {
        self.goal_col = None;
        if self.delete_selection() {
            return;
        }
        if let Some((i, _)) = self.text[..self.caret].char_indices().next_back() {
            self.text.replace_range(i..self.caret, "");
            self.caret = i;
        }
    }

    /// Delete the selection, or the character after the caret.
    pub fn delete_forward(&mut self) {
        self.goal_col = None;
        if self.delete_selection() {
            return;
        }
        if let Some(c) = self.text[self.caret..].chars().next() {
            self.text
                .replace_range(self.caret..self.caret + c.len_utf8(), "");
        }
    }

    pub fn left(&mut self) {
        if let Some((i, _)) = self.text[..self.caret].char_indices().next_back() {
            self.caret = i;
        }
        self.settle();
    }

    pub fn right(&mut self) {
        if let Some(c) = self.text[self.caret..].chars().next() {
            self.caret += c.len_utf8();
        }
        self.settle();
    }

    /// Start of the current visual line.
    pub fn home(&mut self, lines: &[InputLine]) {
        self.caret = line_home(lines, self.caret);
        self.settle();
    }

    /// End of the current visual line.
    pub fn end(&mut self, lines: &[InputLine]) {
        self.caret = line_end(lines, self.caret);
        self.settle();
    }

    /// Put the caret at `pos`, growing the selection when `extend`, else dropping it.
    fn move_caret(&mut self, pos: usize, extend: bool) {
        if extend {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        self.caret = pos;
        self.goal_col = None;
    }

    /// Ctrl+Left: the start of the word before the caret. See [`word_left`].
    pub fn word_left(&mut self, extend: bool) {
        self.move_caret(word_left(&self.text, self.caret), extend);
    }

    /// Ctrl+Right: the end of the word after the caret. See [`word_right`].
    pub fn word_right(&mut self, extend: bool) {
        self.move_caret(word_right(&self.text, self.caret), extend);
    }

    /// Ctrl+Home: the start of the whole text.
    pub fn text_home(&mut self, extend: bool) {
        self.move_caret(0, extend);
    }

    /// Ctrl+End: the end of the whole text.
    pub fn text_end(&mut self, extend: bool) {
        self.move_caret(self.text.len(), extend);
    }

    /// Ctrl+Backspace: delete the selection, or back to the start of the word.
    pub fn delete_word_back(&mut self) {
        self.goal_col = None;
        if self.delete_selection() {
            return;
        }
        let from = word_left(&self.text, self.caret);
        self.text.replace_range(from..self.caret, "");
        self.caret = from;
    }

    /// Ctrl+Delete: delete the selection, or on to the end of the word.
    pub fn delete_word_forward(&mut self) {
        self.goal_col = None;
        if self.delete_selection() {
            return;
        }
        let to = word_right(&self.text, self.caret);
        self.text.replace_range(self.caret..to, "");
    }

    /// Up one visual line. `extend` grows the selection instead of dropping it.
    pub fn up(&mut self, lines: &[InputLine], extend: bool) {
        self.vertical(lines, extend, false);
    }

    /// Down one visual line. `extend` grows the selection instead of dropping it.
    pub fn down(&mut self, lines: &[InputLine], extend: bool) {
        self.vertical(lines, extend, true);
    }

    fn vertical(&mut self, lines: &[InputLine], extend: bool, down: bool) {
        if extend {
            self.anchor.get_or_insert(self.caret);
        } else {
            self.anchor = None;
        }
        let (pos, goal) = vertical(&self.text, lines, self.caret, self.goal_col, down);
        self.caret = pos;
        self.goal_col = goal;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts<'a>(text: &'a str, lines: &[InputLine]) -> Vec<&'a str> {
        lines.iter().map(|l| &text[l.start..l.end]).collect()
    }

    #[test]
    fn hard_lines_split_on_newlines_and_keep_a_trailing_empty_line() {
        let t = "ab\n\ncd\n";
        assert_eq!(texts(t, &hard_lines(t)), ["ab", "", "cd", ""]);
        assert_eq!(hard_lines("").len(), 1);
    }

    #[test]
    fn wrap_cells_breaks_at_spaces_and_honours_newlines() {
        let t = "one two three\nfour";
        let lines = wrap_cells(t, 8, 8, 0);
        assert_eq!(texts(t, &lines), ["one two", "three", "four"]);
    }

    #[test]
    fn wrap_cells_keeps_repeated_spaces_and_blank_lines() {
        let t = "a  b\n\nc";
        assert_eq!(texts(t, &wrap_cells(t, 20, 20, 0)), ["a  b", "", "c"]);
    }

    #[test]
    fn wrap_cells_splits_a_word_longer_than_the_line() {
        let t = "abcdefgh";
        assert_eq!(texts(t, &wrap_cells(t, 3, 3, 0)), ["abc", "def", "gh"]);
    }

    #[test]
    fn wrap_cells_indents_continuation_lines_and_uses_their_width() {
        let t = "aaa bbb ccc";
        let lines = wrap_cells(t, 3, 5, 2);
        assert_eq!(texts(t, &lines), ["aaa", "bbb", "ccc"]);
        assert_eq!(lines[0].indent_cols, 0);
        assert_eq!(lines[1].indent_cols, 2);
    }

    #[test]
    fn wrap_cells_loses_at_most_one_break_byte_between_lines() {
        let t = "the quick  brown\nfox jumps over   the lazy dog";
        let lines = wrap_cells(t, 7, 6, 1);
        assert_eq!(lines.first().unwrap().start, 0);
        assert_eq!(lines.last().unwrap().end, t.len());
        for w in lines.windows(2) {
            assert!(w[1].start == w[0].end || w[1].start == w[0].end + 1);
        }
    }

    #[test]
    fn down_and_up_walk_a_wrapped_single_line() {
        let t = "one two three";
        let lines = wrap_cells(t, 8, 8, 0); // "one two" | "three"
        let (pos, goal) = vertical(t, &lines, 2, None, true);
        assert_eq!(pos, 10); // "th|ree"
        let (pos, _) = vertical(t, &lines, pos, goal, false);
        assert_eq!(pos, 2);
    }

    #[test]
    fn up_on_the_first_line_goes_to_the_start_and_down_on_the_last_to_the_end() {
        let t = "abc\ndef";
        let lines = hard_lines(t);
        assert_eq!(vertical(t, &lines, 2, None, false), (0, None));
        assert_eq!(vertical(t, &lines, 5, None, true), (t.len(), None));
    }

    #[test]
    fn goal_column_survives_a_short_line() {
        let t = "abcdef\nx\nabcdef";
        let lines = hard_lines(t);
        let (pos, goal) = vertical(t, &lines, 4, None, true);
        assert_eq!(pos, 8); // end of "x"
        let (pos, _) = vertical(t, &lines, pos, goal, true);
        assert_eq!(pos, 9 + 4);
    }

    #[test]
    fn columns_include_the_indent() {
        // A prefix of 4 columns on the first line: caret under "ab|" (col 6)
        // lands at col 6 of the next line.
        let t = "abcd\nabcdefgh";
        let mut lines = hard_lines(t);
        lines[0].indent_cols = 4;
        assert_eq!(line_of(t, &lines, 2), (0, 6));
        assert_eq!(vertical(t, &lines, 2, None, true).0, 5 + 6);
    }

    #[test]
    fn vertical_moves_by_characters_not_bytes() {
        let t = "héllo\nwörld";
        let lines = hard_lines(t);
        let pos = "hél".len();
        let (down, _) = vertical(t, &lines, pos, None, true);
        assert_eq!(&t[down..], "ld");
    }

    #[test]
    fn caret_on_a_wrap_gap_belongs_to_the_line_before() {
        let t = "one two";
        let lines = wrap_cells(t, 4, 4, 0); // "one" | "two", space at 3 is the gap
        assert_eq!(line_of(t, &lines, 3), (0, 3));
        assert_eq!(line_of(t, &lines, 4), (1, 0));
    }

    #[test]
    fn selection_spans_follow_wrapped_lines() {
        let t = "one two three";
        let lines = wrap_cells(t, 8, 8, 0); // "one two" | "three"
        assert_eq!(selection_spans(&lines, 4, 10), vec![(0, 4, 7), (1, 8, 10)]);
        assert_eq!(selection_spans(&lines, 10, 4), vec![(0, 4, 7), (1, 8, 10)]);
        assert!(selection_spans(&lines, 7, 8).is_empty());
    }

    #[test]
    fn state_edits_replace_the_selection() {
        let mut s = InputState::new("hello world", true);
        let lines = hard_lines(&s.text);
        s.home(&lines);
        s.anchor = Some(5);
        s.insert_str("bye");
        assert_eq!(s.text, "bye world");
        assert_eq!(s.caret, 3);
        s.backspace();
        s.delete_forward();
        assert_eq!(s.text, "byworld");
    }

    #[test]
    fn state_newline_then_up_down_cross_it() {
        let mut s = InputState::new("ab", true);
        s.insert_str("\n");
        s.insert_str("cd");
        let lines = hard_lines(&s.text);
        s.up(&lines, false);
        assert_eq!(s.caret, 2);
        s.down(&lines, true);
        assert_eq!(s.caret, s.text.len());
        assert_eq!(s.selection(), Some((2, 5)));
    }

    #[test]
    fn state_left_right_step_whole_characters() {
        let mut s = InputState::new("aé", true);
        s.left();
        assert_eq!(s.caret, 1);
        s.right();
        assert_eq!(s.caret, 3);
        s.right();
        assert_eq!(s.caret, 3);
    }

    #[test]
    fn word_left_lands_on_the_start_of_the_word_before() {
        let t = "hello brave  world";
        assert_eq!(word_left(t, t.len()), 13);
        assert_eq!(word_left(t, 13), 6);
        assert_eq!(word_left(t, 8), 6);
        assert_eq!(word_left(t, 6), 0);
        assert_eq!(word_left(t, 0), 0);
    }

    #[test]
    fn word_right_lands_on_the_end_of_the_word_after() {
        let t = "hello brave  world";
        assert_eq!(word_right(t, 0), 5);
        assert_eq!(word_right(t, 5), 11);
        assert_eq!(word_right(t, 11), t.len());
        assert_eq!(word_right(t, t.len()), t.len());
    }

    #[test]
    fn punctuation_is_a_word_of_its_own() {
        let t = "foo.bar(baz)";
        assert_eq!(word_right(t, 0), 3);
        assert_eq!(word_right(t, 3), 4);
        assert_eq!(word_left(t, t.len()), 11);
        assert_eq!(word_left(t, 11), 8);
        assert_eq!(word_left(t, 8), 7);
    }

    #[test]
    fn a_word_crosses_line_breaks_and_whole_characters() {
        let t = "één\n  café_2";
        assert_eq!(word_right(t, 0), "één".len());
        assert_eq!(word_right(t, "één".len()), t.len());
        assert_eq!(word_left(t, t.len()), "één\n  ".len());
        assert_eq!(word_left(t, "één\n  ".len()), 0);
        // A position inside a character is clamped down to its start.
        assert_eq!(word_left(t, 1), 0);
        assert_eq!(word_right(t, 1), "één".len());
        assert_eq!(word_right(t, 99), t.len());
    }

    #[test]
    fn state_word_moves_extend_and_drop_the_selection() {
        let mut s = InputState::new("one two three", true);
        s.word_left(false);
        assert_eq!(s.caret, 8);
        assert_eq!(s.selection(), None);
        s.word_left(true);
        assert_eq!(s.selection(), Some((4, 8)));
        s.word_right(false);
        assert_eq!((s.caret, s.selection()), (7, None));
        s.text_end(false);
        s.text_home(true);
        assert_eq!(s.selection(), Some((0, 13)));
        s.text_end(false);
        assert_eq!((s.caret, s.selection()), (13, None));
    }

    #[test]
    fn state_word_deletes_take_a_word_or_the_selection() {
        let mut s = InputState::new("one two three", true);
        s.delete_word_back();
        assert_eq!((s.text.as_str(), s.caret), ("one two ", 8));
        s.text_home(false);
        s.delete_word_forward();
        assert_eq!((s.text.as_str(), s.caret), (" two ", 0));
        s.word_right(true);
        s.delete_word_back();
        assert_eq!((s.text.as_str(), s.caret), (" ", 0));
    }
}
