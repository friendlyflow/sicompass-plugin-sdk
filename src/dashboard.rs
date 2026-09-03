//! Types describing the interactive-dashboard surface a provider can opt into.
//!
//! The legacy fullscreen mode (`Provider::dashboard_image_path`) renders a
//! static image and is unaffected by anything in this module. Providers that
//! return [`DashboardKind::Interactive`] from `dashboard_kind()` instead
//! receive raw input events and supply a [`DashboardFrame`] every frame.
//!
//! All graphics are described in *cells* — the app turns each cell into a
//! background rectangle plus an optional glyph using its existing font and
//! rectangle pipelines. The SDK never references the windowing or graphics
//! crates directly.

// ---------------------------------------------------------------------------
// Opt-in
// ---------------------------------------------------------------------------

/// What kind of fullscreen view, if any, a provider supports.
///
/// The default is `None`: pressing the `d` key while this provider is active
/// is a no-op. The two opt-in variants are mutually exclusive and route the
/// app down completely separate render and input paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DashboardKind {
    /// No fullscreen view.
    #[default]
    None,
    /// Static image — provider supplies a path via `dashboard_image_path()`.
    /// The app reads/scales/centers the image and only listens for `Escape`.
    Image,
    /// Cell-grid view driven by per-frame `dashboard_render(cols, rows)`.
    /// The app forwards keystrokes, text input, and resize events to the
    /// provider; `Escape` still exits back to the previous coordinate.
    Interactive,
}

// ---------------------------------------------------------------------------
// Cell grid
// ---------------------------------------------------------------------------

/// Per-cell text attributes. Reserved for future SGR support — Phase 2a only
/// honours `reverse` (swap fg/bg).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellAttrs {
    pub bold: bool,
    pub underline: bool,
    pub reverse: bool,
}

/// One character cell in a [`DashboardFrame`].
///
/// Colors are packed `0xRRGGBBAA`, matching the rest of the app's palette
/// representation. An alpha of 0 in `bg` means "draw no background fill"
/// (the window clear colour shows through).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DashboardCell {
    pub ch: char,
    pub fg: u32,
    pub bg: u32,
    pub attrs: CellAttrs,
}

impl Default for DashboardCell {
    fn default() -> Self {
        DashboardCell {
            ch: ' ',
            fg: 0xFFFFFFFF,
            bg: 0x00000000,
            attrs: CellAttrs::default(),
        }
    }
}

/// A rectangle of cells the app should paint as a **selection**, the way it
/// paints the selected row of a list: one rounded shape, slightly inset from the
/// rows above and below.
///
/// It exists because a cell grid cannot express either half of that. Rounding is
/// per-rectangle, so a multi-row selection drawn as one rectangle per row comes
/// out as a stack of separate rounded blobs; and a cell is a whole row tall, so
/// there is no way to leave a *part* of a row as breathing space around the
/// highlight. Naming the region lets the app draw it once, as one shape.
///
/// Only one, and only where the provider means "this is the cursor". A dashboard
/// that paints arbitrary coloured spans — a terminal emulator rendering SGR
/// backgrounds — leaves this `None` and gets the plain per-cell fills, which is
/// what those spans should look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DashboardSelection {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
}

/// How the app should draw the frame's cursor.
///
/// A terminal's cursor is a filled cell, because that is what a terminal cursor
/// is. A caret in a text field is not: the app draws its own insert-mode caret as
/// a thin blinking bar between two characters, and a dashboard editing text
/// should look like that rather than like a shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DashboardCursor {
    /// Fill the cell. What a terminal emulator wants.
    #[default]
    Block,
    /// A thin blinking bar on the cell's leading edge, drawn the same way the
    /// app draws its own insert-mode caret.
    Bar,
}

/// A snapshot of the provider's terminal grid for one frame.
///
/// `cells` is row-major with length `cols * rows`. `cursor` is `(col, row)`
/// in cell coordinates (0-indexed); `None` hides the cursor.
#[derive(Debug, Clone)]
pub struct DashboardFrame {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<DashboardCell>,
    pub cursor: Option<(u16, u16)>,
    /// The one region to paint as a selection, if any. See
    /// [`DashboardSelection`].
    pub selection: Option<DashboardSelection>,
    /// Rows after which the app leaves **half a line height** of extra space.
    ///
    /// A cell is a whole row tall, so a provider that wants less air than a blank
    /// row and more than none has no way to say so in the grid itself: a blank
    /// row is a full line, and no row is nothing. This names the places where the
    /// app should open a half-line instead, which it can do because it turns
    /// rows into pixels.
    ///
    /// Everything below a gap shifts down by half a line, so the last part-row
    /// may fall off the bottom — the same clipping a grid one row too short
    /// already has. Empty for a provider that wants a plain grid.
    pub half_gap_rows: Vec<u16>,
    /// How to draw [`DashboardFrame::cursor`]. See [`DashboardCursor`].
    pub cursor_style: DashboardCursor,
}

impl DashboardFrame {
    /// How many half-lines of extra space sit above `row`.
    ///
    /// The app multiplies this by half a cell height to place the row; a provider
    /// can use it to keep its own hit-testing in step.
    pub fn half_gaps_above(&self, row: u16) -> u16 {
        self.half_gap_rows.iter().filter(|r| **r < row).count() as u16
    }

    /// A frame filled with the default cell (space, white-on-transparent).
    pub fn empty(cols: u16, rows: u16) -> Self {
        let len = (cols as usize) * (rows as usize);
        DashboardFrame {
            cols,
            rows,
            cells: vec![DashboardCell::default(); len],
            cursor: None,
            selection: None,
            half_gap_rows: Vec::new(),
            cursor_style: DashboardCursor::Block,
        }
    }

    fn idx(&self, col: u16, row: u16) -> usize {
        (row as usize) * (self.cols as usize) + (col as usize)
    }

    pub fn cell(&self, col: u16, row: u16) -> &DashboardCell {
        &self.cells[self.idx(col, row)]
    }

    pub fn cell_mut(&mut self, col: u16, row: u16) -> &mut DashboardCell {
        let i = self.idx(col, row);
        &mut self.cells[i]
    }

    /// Set the character + fg color of a cell, preserving its existing bg/attrs.
    pub fn set_char(&mut self, col: u16, row: u16, ch: char, fg: u32) {
        if col < self.cols && row < self.rows {
            let c = self.cell_mut(col, row);
            c.ch = ch;
            c.fg = fg;
        }
    }

    /// Write a string starting at `(col, row)`, left-to-right, clipping at
    /// the right edge. Tabs and newlines are not interpreted.
    pub fn write_str(&mut self, col: u16, row: u16, s: &str, fg: u32) {
        if row >= self.rows {
            return;
        }
        let mut c = col;
        for ch in s.chars() {
            if c >= self.cols {
                break;
            }
            self.set_char(c, row, ch, fg);
            c += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// The app's active colour palette, handed to a provider so an interactive
/// dashboard can draw in the colours the rest of the app is already using.
///
/// Without it a provider has to invent its own, which is wrong twice over: the
/// dashboard stops matching the list the user just came from, and it stops
/// following the light/dark theme, because a hardcoded constant cannot know
/// which one is active.
///
/// The fields mirror the app's own palette one for one. Their meanings, in the
/// order a dashboard usually needs them:
///
/// * `background` — the window ground. A cell with `bg` alpha 0 shows it.
/// * `text` — every ordinary glyph.
/// * `selected` — the fill behind whatever the cursor is on. In the list this is
///   the highlight on the current row, so a dashboard that uses it for its own
///   cursor reads as the same idea rather than a second convention.
/// * `header_sep` — the app's chrome divider. Subtle against the background in
///   both themes, which makes it the right fill for a band or a rule.
/// * `error` — the status line when something failed.
/// * `ext_search`, `scroll_search` — the two find-highlight fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DashboardPalette {
    pub background: u32,
    pub text: u32,
    pub header_sep: u32,
    pub selected: u32,
    pub ext_search: u32,
    pub scroll_search: u32,
    pub error: u32,
}

impl Default for DashboardPalette {
    /// The app's dark theme, so a provider that is never handed one still draws
    /// in real colours rather than in black on black.
    fn default() -> Self {
        DashboardPalette {
            background: 0x000000FF,
            text: 0xFFFFFFFF,
            header_sep: 0x333333FF,
            selected: 0x2D4A28FF,
            ext_search: 0x696969FF,
            scroll_search: 0x264F78FF,
            error: 0xFF0000FF,
        }
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// A non-printable / modified key event forwarded while the provider is in
/// the interactive dashboard. Printable text arrives separately through
/// `Provider::dashboard_text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DashboardKey {
    pub keysym: DashboardKeysym,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// A request from a provider for the app to switch its dashboard mode.
///
/// Returned by [`crate::Provider::take_dashboard_request`]. The app polls
/// this in its tick loop after `Provider::tick()` and dispatches accordingly.
/// Honored only when the requesting provider is the active one — never yanks
/// the user out of one tab into another tab's dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardRequest {
    /// Enter the active provider's interactive dashboard. Equivalent to the
    /// user pressing `d`. No-op if a dashboard is already entered.
    Enter,
    /// Leave the dashboard, restoring the prior coordinate. No-op if no
    /// dashboard is currently entered.
    Leave,
}

/// Keysym variants the app maps SDL keycodes onto. Kept deliberately small —
/// providers handle printable input through `dashboard_text` and reach for
/// `Char(c)` only when modifiers (e.g. Ctrl+letter) suppress the text-input
/// event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardKeysym {
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
    /// Letter / digit when the text-input event is suppressed (e.g. Ctrl held).
    /// Stored lowercase; modifier flags live on the surrounding `DashboardKey`.
    Char(char),
    /// Anything we don't recognise. Providers should generally ignore this.
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_frame_has_correct_dimensions() {
        let f = DashboardFrame::empty(80, 24);
        assert_eq!(f.cols, 80);
        assert_eq!(f.rows, 24);
        assert_eq!(f.cells.len(), 80 * 24);
        assert!(f.cursor.is_none());
    }

    #[test]
    fn write_str_clips_at_right_edge() {
        let mut f = DashboardFrame::empty(5, 1);
        f.write_str(3, 0, "hello", 0xFFFFFFFF);
        assert_eq!(f.cell(3, 0).ch, 'h');
        assert_eq!(f.cell(4, 0).ch, 'e');
    }

    #[test]
    fn write_str_no_op_past_last_row() {
        let mut f = DashboardFrame::empty(5, 1);
        f.write_str(0, 5, "x", 0xFFFFFFFF);
        assert_eq!(f.cell(0, 0).ch, ' ');
    }

    #[test]
    fn set_char_out_of_bounds_is_no_op() {
        let mut f = DashboardFrame::empty(2, 2);
        f.set_char(99, 99, 'x', 0xFFFFFFFF);
        assert_eq!(f.cell(0, 0).ch, ' ');
    }

    #[test]
    fn dashboard_kind_default_is_none() {
        assert_eq!(DashboardKind::default(), DashboardKind::None);
    }
}
