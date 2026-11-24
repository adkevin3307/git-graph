//! Mouse text selection: drag over a pane to highlight text, release to copy it.
//!
//! Positions are (content row, column) within one pane, so a selection survives
//! scrolling. Text comes from the diff lines for the diff, and from what was drawn
//! on screen for the other panes.

use std::collections::BTreeMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use unicode_width::UnicodeWidthChar;

const SELECTION_BG: Color = Color::Rgb(70, 90, 130);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Graph,
    Commit,
    Files,
    Diff,
}

/// Where a pane's text was drawn last frame.
#[derive(Clone, Copy, Debug)]
pub struct PaneLayout {
    pub pane: Pane,
    /// Inner text area.
    pub area: Rect,
    /// Content row shown on the first screen row.
    pub row_offset: usize,
    /// Content column shown on the first screen column.
    pub col_offset: usize,
}

impl PaneLayout {
    pub fn contains(&self, x: u16, y: u16) -> bool {
        let a = self.area;
        x >= a.x && x < a.right() && y >= a.y && y < a.bottom()
    }

    /// Content (row, column) under terminal cell (x, y), clamped to the area.
    pub fn content_pos(&self, x: u16, y: u16) -> (usize, usize) {
        let a = self.area;
        let x = x.clamp(a.x, a.right().saturating_sub(1));
        let y = y.clamp(a.y, a.bottom().saturating_sub(1));
        (self.row_offset + (y - a.y) as usize, self.col_offset + (x - a.x) as usize)
    }
}

pub struct Selection {
    pub pane: Pane,
    pub anchor: (usize, usize),
    pub cursor: (usize, usize),
    /// The mouse moved after the button went down (a plain click selects nothing).
    pub dragged: bool,
    /// Latest drag position (terminal cell), resolved against the next layout.
    pub pending: Option<(u16, u16)>,
    /// Text of the content rows seen on screen during the drag (panes other than
    /// the diff, whose text is read from its lines).
    pub rows: BTreeMap<usize, String>,
}

impl Selection {
    pub fn new(pane: Pane, at: (usize, usize)) -> Self {
        Self { pane, anchor: at, cursor: at, dragged: false, pending: None, rows: BTreeMap::new() }
    }

    /// Start and end in reading order; both ends are inclusive.
    pub fn range(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// Columns `[from, to)` selected on content row `row`, if any.
    pub fn columns(&self, row: usize) -> Option<(usize, usize)> {
        if !self.dragged {
            return None;
        }
        let (start, end) = self.range();
        if row < start.0 || row > end.0 {
            return None;
        }
        let from = if row == start.0 { start.1 } else { 0 };
        let to = if row == end.0 { end.1.saturating_add(1) } else { usize::MAX };
        Some((from, to))
    }

    /// The selected text, given each row's full text. Trailing spaces are trimmed.
    pub fn text(&self, row_text: impl Fn(usize) -> Option<String>) -> String {
        let (start, end) = self.range();
        let mut lines = Vec::new();
        for row in start.0..=end.0 {
            let Some((from, to)) = self.columns(row) else { continue };
            let Some(text) = row_text(row) else { continue };
            lines.push(slice_columns(&text, from, to).trim_end().to_string());
        }
        lines.join("\n")
    }
}

fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// The characters of `text` within display columns `[from, to)`.
pub fn slice_columns(text: &str, from: usize, to: usize) -> String {
    let mut out = String::new();
    let mut col = 0;
    for c in text.chars() {
        if col >= to {
            break;
        }
        let w = char_width(c);
        if col < to && col + w.max(1) > from {
            out.push(c);
        }
        col += w;
    }
    out
}

/// The text drawn on screen row `y` within `area` (wide characters once).
pub fn screen_row(buf: &Buffer, area: Rect, y: u16) -> String {
    let mut out = String::new();
    let mut x = area.x;
    while x < area.right() {
        let sym = buf[(x, y)].symbol();
        out.push_str(sym);
        let w: usize = sym.chars().map(char_width).sum();
        x += w.max(1) as u16;
    }
    out
}

/// Gives the selected cells of a pane a selection background, up to the end of
/// each row's text. `row_text` gives each visible content row's full text.
pub fn highlight(
    buf: &mut Buffer,
    layout: &PaneLayout,
    sel: &Selection,
    row_text: impl Fn(usize) -> Option<String>,
) {
    let a = layout.area;
    for y in a.y..a.bottom() {
        let row = layout.row_offset + (y - a.y) as usize;
        let Some((from, to)) = sel.columns(row) else { continue };
        let text_end = row_text(row).map_or(0, |t| t.trim_end().chars().map(char_width).sum());
        let to = to.min(text_end.max(from + 1));
        for x in a.x..a.right() {
            let col = layout.col_offset + (x - a.x) as usize;
            if col >= from && col < to {
                buf[(x, y)].set_bg(SELECTION_BG);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_and_text_in_reading_order() {
        let rows = ["first line", "second", "third line  "];
        let mut sel = Selection::new(Pane::Diff, (2, 4));
        sel.cursor = (0, 6);
        sel.dragged = true;
        assert_eq!(sel.columns(0), Some((6, usize::MAX)));
        assert_eq!(sel.columns(1), Some((0, usize::MAX)));
        assert_eq!(sel.columns(2), Some((0, 5)));
        assert_eq!(sel.columns(3), None);
        let text = sel.text(|r| rows.get(r).map(|s| s.to_string()));
        assert_eq!(text, "line\nsecond\nthird");
        // A plain click selects nothing.
        assert_eq!(Selection::new(Pane::Diff, (0, 0)).columns(0), None);
    }

    #[test]
    fn slices_by_display_width() {
        assert_eq!(slice_columns("ab日本cd", 0, 3), "ab日");
        assert_eq!(slice_columns("ab日本cd", 3, 5), "日本");
        assert_eq!(slice_columns("ab日本cd", 6, usize::MAX), "cd");
    }

    #[test]
    fn reads_and_highlights_screen_rows() {
        let area = Rect::new(0, 0, 8, 2);
        let mut buf = Buffer::empty(area);
        buf.set_string(0, 0, "日本 ok", ratatui::style::Style::new());
        buf.set_string(0, 1, "abc", ratatui::style::Style::new());
        assert_eq!(screen_row(&buf, area, 0), "日本 ok ");
        let layout = PaneLayout { pane: Pane::Files, area, row_offset: 0, col_offset: 0 };
        let mut sel = Selection::new(Pane::Files, (0, 5));
        sel.cursor = (1, 7);
        sel.dragged = true;
        let rows = ["日本 ok", "abc"];
        highlight(&mut buf, &layout, &sel, |r| rows.get(r).map(|s| s.to_string()));
        // Row 0 from column 5 to its text end (7); row 1 only up to "abc".
        let bg = |x, y| buf[(x, y)].bg == SELECTION_BG;
        assert!(!bg(4, 0) && bg(5, 0) && bg(6, 0) && !bg(7, 0));
        assert!(bg(0, 1) && bg(2, 1) && !bg(3, 1));
    }
}
