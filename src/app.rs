use std::io::Write;

use anyhow::Result;
use git2::{Oid, Repository};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::input::{LineEditor, push_history};
use crate::matcher::Matcher;
use crate::repo::{self, CommitDetail, DiffLine, LoadOptions, RefKind, Snapshot};
use crate::selection::{Pane, PaneLayout, Selection};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Graph,
    /// Commit details (scrollable).
    Commit,
    Files,
    /// The diff panel (shown in place of the commit graph).
    Diff,
}

/// Columns moved per h/l (or ←/→) in the diff view.
const HSTEP: usize = 4;

pub enum Mode {
    Normal,
    /// The search box is open; what is typed is in `App::editor`.
    Search,
    /// Branch picker; the filter is typed into `App::editor`.
    Branches {
        selected: usize,
    },
    Confirm {
        prompt: String,
        action: Pending,
    },
    /// Help overlay, scrolled down by `scroll` lines.
    Help {
        scroll: u16,
    },
}

pub enum Pending {
    Checkout { id: Oid, branch: Option<String> },
}

pub struct DiffView {
    pub title: String,
    pub lines: Vec<DiffLine>,
    pub scroll: usize,
    /// Horizontal scroll (columns).
    pub hscroll: usize,
    /// Width of the longest line, to bound horizontal scrolling.
    pub max_width: usize,
    pub file_index: Option<usize>,
}

pub struct App {
    pub repo: Repository,
    pub opts: LoadOptions,
    pub snap: Snapshot,
    pub selected: usize,
    pub offset: usize,
    pub focus: Focus,
    pub mode: Mode,
    pub detail: Option<CommitDetail>,
    pub file_selected: usize,
    pub diff: Option<DiffView>,
    /// The active search and the commits it matches, in graph order.
    pub search: Option<Matcher>,
    pub matches: Vec<usize>,
    /// Esc hides the search highlights (like vim's :noh); n / N show them again.
    pub search_hidden: bool,
    /// What is typed in the search box, and earlier searches.
    pub editor: LineEditor,
    search_history: Vec<String>,
    /// State when the search box opened, restored on cancel.
    search_origin: Option<SearchOrigin>,
    pub status: String,
    pub quit: bool,
    /// Height of the graph list in rows, updated on draw.
    pub page: usize,
    /// Top of the graph list area on screen (for mouse clicks).
    pub list_top: u16,
    pub list_right: u16,
    /// Cycle state for `m`: (merge commit, parent index last visited,
    /// commit selected after that jump).
    merge_cycle: Option<(Oid, usize, Oid)>,
    /// Where the file list was last drawn, and its scroll offset (for mouse clicks).
    pub files_area: Rect,
    pub files_offset: usize,
    /// Where the diff panel was last drawn (inner text area).
    pub diff_area: Rect,
    /// Diff fills the whole area, hiding commit details and files (toggle: z).
    pub diff_maximized: bool,
    /// Scroll offset of the commit details pane, and where it was drawn.
    pub commit_scroll: u16,
    pub commit_area: Rect,
    /// How far the help can scroll (set when it is drawn).
    pub help_max: u16,
    /// Where each pane's text was drawn last frame (for mouse selection).
    pub panes: Vec<PaneLayout>,
    /// Text being selected with the mouse.
    pub selection: Option<Selection>,
}

impl App {
    pub fn new(repo: Repository, opts: LoadOptions) -> Result<Self> {
        let snap = repo::load(&repo, &opts)?;
        let mut app = App {
            repo,
            opts,
            snap,
            selected: 0,
            offset: 0,
            focus: Focus::Graph,
            mode: Mode::Normal,
            detail: None,
            file_selected: 0,
            diff: None,
            search: None,
            matches: Vec::new(),
            search_hidden: false,
            editor: LineEditor::default(),
            search_history: Vec::new(),
            search_origin: None,
            status: String::new(),
            quit: false,
            page: 20,
            list_top: 0,
            list_right: 0,
            merge_cycle: None,
            files_area: Rect::default(),
            files_offset: 0,
            diff_area: Rect::default(),
            diff_maximized: false,
            commit_scroll: 0,
            commit_area: Rect::default(),
            help_max: 0,
            panes: Vec::new(),
            selection: None,
        };
        if let Some(i) = app.snap.head.and_then(|h| app.snap.index.get(&h).copied()) {
            app.select(i);
        }
        app.load_detail();
        if app.snap.truncated {
            app.status = format!(
                "Showing first {} commits (use -n to change)",
                app.snap.commits.len()
            );
        }
        Ok(app)
    }

    pub fn reload(&mut self) {
        let current = self.snap.commits.get(self.selected).map(|c| c.id);
        match repo::load(&self.repo, &self.opts) {
            Ok(s) => {
                self.snap = s;
                let i = current
                    .and_then(|id| self.snap.index.get(&id).copied())
                    .unwrap_or(0);
                self.selected = 0;
                self.select(i);
                self.recompute_matches();
                self.status = "Reloaded".into();
            }
            Err(e) => self.status = format!("Reload failed: {e:#}"),
        }
    }

    fn select(&mut self, i: usize) {
        let n = self.snap.commits.len();
        if n == 0 {
            return;
        }
        let i = i.min(n - 1);
        let changed = i != self.selected || self.detail.is_none();
        self.selected = i;
        // Keep the selected row inside the viewport with a small margin. The
        // offset counts graph rows, which include connector-only rows.
        let rows = self.snap.rows.len();
        let row = self.snap.commit_row[i];
        let margin = 3.min(self.page / 4);
        if row < self.offset + margin {
            self.offset = row.saturating_sub(margin);
        } else if row + margin >= self.offset + self.page {
            self.offset = (row + margin + 1).saturating_sub(self.page);
        }
        self.offset = self.offset.min(rows.saturating_sub(self.page.min(rows)));
        if changed {
            self.load_detail();
        }
    }

    fn load_detail(&mut self) {
        self.file_selected = 0;
        self.files_offset = 0;
        self.commit_scroll = 0;
        self.detail = self
            .snap
            .commits
            .get(self.selected)
            .and_then(|c| repo::detail(&self.repo, c.id).ok());
    }

    fn select_id(&mut self, id: Oid) -> bool {
        match self.snap.index.get(&id).copied() {
            Some(i) => {
                self.select(i);
                true
            }
            None => {
                self.status = format!("{} is not loaded (try a larger -n)", &id.to_string()[..8]);
                false
            }
        }
    }

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        if let Mode::Help { scroll } = &mut self.mode {
            match ev.kind {
                MouseEventKind::ScrollDown => *scroll = (*scroll + 3).min(self.help_max),
                MouseEventKind::ScrollUp => *scroll = scroll.saturating_sub(3),
                _ => {}
            }
            return;
        }
        if !matches!(self.mode, Mode::Normal) {
            return;
        }
        // ctrl + wheel scrolls faster, as in track and monitor.
        let lines = if ev.modifiers.contains(KeyModifiers::CONTROL) {
            15
        } else {
            3
        };
        let (x, y) = (ev.column, ev.row);
        match ev.kind {
            // Pressing starts a selection; the click below still does its usual thing.
            MouseEventKind::Down(MouseButton::Left) => {
                self.status.clear();
                self.selection = self
                    .panes
                    .iter()
                    .find(|p| p.contains(x, y))
                    .map(|p| Selection::new(p.pane, p.content_pos(x, y)));
            }
            MouseEventKind::Drag(MouseButton::Left) if self.selection.is_some() => {
                self.drag_selection(x, y);
                return;
            }
            MouseEventKind::Up(MouseButton::Left) if self.selection.is_some() => {
                self.finish_selection();
                return;
            }
            _ => {}
        }
        match ev.kind {
            MouseEventKind::ScrollDown => self.scroll(lines),
            MouseEventKind::ScrollUp => self.scroll(-lines),
            MouseEventKind::ScrollRight => self.hscroll(HSTEP as isize),
            MouseEventKind::ScrollLeft => self.hscroll(-(HSTEP as isize)),
            MouseEventKind::Down(_) if inside(self.files_area, ev.column, ev.row, 1) => {
                // Click a file to show its diff in place of the graph.
                let i = self.files_offset + (ev.row - self.files_area.y - 1) as usize;
                if self.detail.as_ref().is_some_and(|d| i < d.files.len()) {
                    self.file_selected = i;
                    self.open_diff(Some(i));
                    self.focus = Focus::Files;
                }
            }
            MouseEventKind::Down(_) if inside(self.commit_area, ev.column, ev.row, 0) => {
                self.focus = Focus::Commit;
            }
            MouseEventKind::Down(_)
                if self.diff.is_some() && inside(self.diff_area, ev.column, ev.row, 0) =>
            {
                self.focus = Focus::Diff;
            }
            MouseEventKind::Down(_)
                if self.diff.is_none()
                    && ev.column < self.list_right
                    && ev.row >= self.list_top =>
            {
                // A connector row selects the commit above it.
                let r = self.offset + (ev.row - self.list_top) as usize;
                let rows = &self.snap.rows;
                let commit = rows[..rows.len().min(r + 1)]
                    .iter()
                    .rev()
                    .find_map(|row| row.commit);
                if let Some(i) = commit {
                    self.focus = Focus::Graph;
                    self.select(i);
                }
            }
            _ => {}
        }
    }

    fn pane_layout(&self, pane: Pane) -> Option<PaneLayout> {
        self.panes.iter().find(|p| p.pane == pane).copied()
    }

    fn drag_selection(&mut self, x: u16, y: u16) {
        let Some(sel) = &self.selection else { return };
        let pane = sel.pane;
        // Dragging past the top or bottom edge scrolls the panes that scroll freely.
        if let Some(layout) = self.pane_layout(pane) {
            let delta = if y < layout.area.y {
                -1
            } else if y >= layout.area.bottom() {
                1
            } else {
                0
            };
            match pane {
                _ if delta == 0 => {}
                Pane::Commit => self.scroll_commit(delta),
                Pane::Diff if self.diff.is_some() => self.scroll(delta),
                _ => {}
            }
        }
        if let Some(sel) = &mut self.selection {
            sel.dragged = true;
            // Resolved after the next layout, which reflects the scrolling above
            sel.pending = Some((x, y));
        }
    }

    /// Moves the selection cursor to the latest drag position, using the last layout.
    pub fn resolve_drag(&mut self) {
        let Some(sel) = &self.selection else { return };
        let (Some((x, y)), Some(layout)) = (sel.pending, self.pane_layout(sel.pane)) else {
            return;
        };
        if let Some(sel) = &mut self.selection {
            sel.cursor = layout.content_pos(x, y);
            sel.pending = None;
        }
    }

    /// The full text of a content row of the selection's pane.
    pub fn selection_row_text(&self, sel: &Selection, row: usize) -> Option<String> {
        match sel.pane {
            Pane::Diff => self.diff.as_ref()?.lines.get(row).map(|l| l.text.clone()),
            _ => sel.rows.get(&row).cloned(),
        }
    }

    /// Copies the selected text to the clipboard.
    fn finish_selection(&mut self) {
        self.resolve_drag();
        let Some(sel) = self.selection.take() else {
            return;
        };
        if !sel.dragged {
            return;
        }
        let text = sel.text(|row| self.selection_row_text(&sel, row));
        if text.is_empty() {
            return;
        }
        copy_to_clipboard(&text);
        let n = text.chars().count();
        self.status = format!(
            "Copied {n} character{} to the clipboard",
            if n == 1 { "" } else { "s" }
        );
    }

    fn scroll(&mut self, delta: isize) {
        if let Some(d) = &mut self.diff {
            d.scroll = d
                .scroll
                .saturating_add_signed(delta)
                .min(d.lines.len().saturating_sub(1));
        } else {
            self.move_by(delta);
        }
    }

    fn hscroll(&mut self, delta: isize) {
        let width = self.diff_area.width as usize;
        if let Some(d) = &mut self.diff {
            let max = d.max_width.saturating_sub(width);
            d.hscroll = d.hscroll.saturating_add_signed(delta).min(max);
        }
    }

    fn close_diff(&mut self) {
        self.diff = None;
        self.diff_maximized = false;
        self.focus = Focus::Files;
    }

    /// Whether the diff currently fills the whole area.
    pub fn diff_fullscreen(&self) -> bool {
        self.diff.is_some() && self.diff_maximized && self.focus == Focus::Diff
    }

    /// → : move focus to the next pane (graph → commit → files → diff),
    /// opening the selected file's diff when entering the diff pane.
    fn focus_right(&mut self) {
        self.focus = match self.focus {
            Focus::Graph => Focus::Commit,
            Focus::Commit => Focus::Files,
            Focus::Files | Focus::Diff => {
                if self.diff.is_none() {
                    self.open_diff(Some(self.file_selected));
                }
                if self.diff.is_some() {
                    Focus::Diff
                } else {
                    Focus::Files
                }
            }
        };
    }

    /// ← : move focus to the previous pane (diff → files → commit → graph).
    /// Going back to the graph closes the diff that replaced it.
    fn focus_left(&mut self) {
        self.focus = match self.focus {
            Focus::Diff => {
                self.diff_maximized = false;
                Focus::Files
            }
            Focus::Files => Focus::Commit,
            Focus::Commit | Focus::Graph => {
                self.diff = None;
                self.diff_maximized = false;
                Focus::Graph
            }
        };
    }

    fn scroll_commit(&mut self, delta: isize) {
        let lines = self
            .detail
            .as_ref()
            .map_or(0, |d| d.message.lines().count())
            + 8;
        let v = (self.commit_scroll as isize + delta).clamp(0, lines as isize - 1);
        self.commit_scroll = v as u16;
    }

    fn move_by(&mut self, delta: isize) {
        match self.focus {
            Focus::Diff => {}
            Focus::Commit => self.scroll_commit(delta),
            Focus::Graph => self.select(self.selected.saturating_add_signed(delta)),
            Focus::Files => {
                let n = self.detail.as_ref().map_or(0, |d| d.files.len());
                if n > 0 {
                    self.file_selected = self.file_selected.saturating_add_signed(delta).min(n - 1);
                }
            }
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        self.selection = None;
        match &mut self.mode {
            Mode::Normal => self.key_normal(key),
            Mode::Help { scroll } => match key.code {
                // Scroll when the help doesn't fit; any other key closes it.
                KeyCode::Down | KeyCode::Char('j') => *scroll = (*scroll + 1).min(self.help_max),
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::PageDown => *scroll = (*scroll + 10).min(self.help_max),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::Char('g') => *scroll = 0,
                KeyCode::Char('G') => *scroll = self.help_max,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.quit = true
                }
                _ => self.mode = Mode::Normal,
            },
            Mode::Search => self.key_search(key),
            Mode::Branches { .. } => self.key_branches(key),
            Mode::Confirm { .. } => {
                let Mode::Confirm { action, .. } = std::mem::replace(&mut self.mode, Mode::Normal)
                else {
                    unreachable!()
                };
                if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    match action {
                        Pending::Checkout { id, branch } => {
                            match repo::checkout(&self.repo, id, branch.as_deref()) {
                                Ok(msg) => {
                                    self.reload();
                                    self.status = msg;
                                }
                                Err(e) => self.status = format!("{e:#}"),
                            }
                        }
                    }
                } else {
                    self.status = "Cancelled".into();
                }
            }
        }
    }

    fn key_normal(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let half = (self.page / 2).max(1) as isize;
        self.status.clear();
        if self.diff.is_some() {
            self.key_diff(key, ctrl, half);
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help { scroll: 0 },
            KeyCode::Down | KeyCode::Char('j') => self.move_by(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_by(-1),
            KeyCode::Char('d') if ctrl => self.move_by(half),
            KeyCode::Char('u') if ctrl => self.move_by(-half),
            KeyCode::PageDown | KeyCode::Char('f') if key.code == KeyCode::PageDown || ctrl => {
                self.move_by(self.page as isize)
            }
            KeyCode::PageUp | KeyCode::Char('b') if key.code == KeyCode::PageUp || ctrl => {
                self.move_by(-(self.page as isize))
            }
            KeyCode::Char('g') => self.move_by(isize::MIN / 2),
            KeyCode::Char('G') => self.move_by(isize::MAX / 2),
            KeyCode::Right => self.focus_right(),
            KeyCode::Left => self.focus_left(),
            KeyCode::Enter => match self.focus {
                Focus::Graph | Focus::Commit => {
                    if self.detail.as_ref().is_some_and(|d| !d.files.is_empty()) {
                        self.focus = Focus::Files;
                    }
                }
                Focus::Files | Focus::Diff => {
                    self.open_diff(Some(self.file_selected));
                    if self.diff.is_some() {
                        self.focus = Focus::Diff;
                    }
                }
            },
            // Esc first hides the search highlights (n / N show them again).
            KeyCode::Esc if self.search.is_some() && !self.search_hidden => {
                self.search_hidden = true
            }
            KeyCode::Esc => self.focus = Focus::Graph,
            KeyCode::Char('D') => {
                self.open_diff(None);
                if self.diff.is_some() {
                    self.focus = Focus::Diff;
                }
            }
            KeyCode::Char('/') => {
                self.search_origin = Some(SearchOrigin {
                    selected: self.selected,
                    offset: self.offset,
                    focus: self.focus,
                    search: self.search.clone(),
                    matches: self.matches.clone(),
                    hidden: self.search_hidden,
                });
                self.editor.clear();
                self.mode = Mode::Search;
            }
            KeyCode::Char('n') => self.next_match(true, false),
            KeyCode::Char('N') => self.next_match(false, false),
            KeyCode::Char('b') => {
                self.editor.clear();
                self.mode = Mode::Branches { selected: 0 }
            }
            KeyCode::Char('H') => {
                if let Some(h) = self.snap.head
                    && self.select_id(h)
                {
                    self.status = format!("HEAD → {}", self.snap.head_name);
                }
            }
            KeyCode::Char('p') => self.goto_parent(0),
            KeyCode::Char('m') => self.cycle_merged_parent(),
            KeyCode::Char('c') => self.child(),
            KeyCode::Char('o') => self.ask_checkout(),
            KeyCode::Char('y') => self.yank(),
            KeyCode::Char('r') => self.reload(),
            _ => {}
        }
    }

    /// Keys while a diff is shown in place of the graph. Focus is either
    /// the diff panel or the file list beside it.
    fn key_diff(&mut self, key: KeyEvent, ctrl: bool, half: isize) {
        match key.code {
            KeyCode::Tab => return self.cycle_diff_file(1),
            KeyCode::BackTab => return self.cycle_diff_file(-1),
            KeyCode::Char('z') => {
                self.diff_maximized = !self.diff_maximized;
                self.focus = Focus::Diff;
                return;
            }
            KeyCode::Right => return self.focus_right(),
            KeyCode::Left => return self.focus_left(),
            KeyCode::Esc => return self.close_diff(),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help { scroll: 0 },
            KeyCode::Char('y') => self.yank(),
            _ => {}
        }
        if self.focus == Focus::Files {
            let n = self.detail.as_ref().map_or(0, |d| d.files.len());
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => self.step_diff_file(1),
                KeyCode::Up | KeyCode::Char('k') => self.step_diff_file(-1),
                KeyCode::Char('g') if n > 0 => self.goto_diff_file(0),
                KeyCode::Char('G') if n > 0 => self.goto_diff_file(n - 1),
                KeyCode::Enter => self.focus = Focus::Diff,
                _ => {}
            }
            return;
        }
        if self.focus == Focus::Commit {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => self.scroll_commit(1),
                KeyCode::Up | KeyCode::Char('k') => self.scroll_commit(-1),
                KeyCode::Char('g') => self.commit_scroll = 0,
                KeyCode::Char('G') => self.scroll_commit(isize::MAX / 2),
                _ => {}
            }
            return;
        }
        let page = self.page as isize;
        let wide = (self.diff_area.width as isize / 2).max(1);
        let d = self.diff.as_mut().unwrap();
        let max = d.lines.len().saturating_sub(1);
        let mut scroll = |delta: isize| d.scroll = d.scroll.saturating_add_signed(delta).min(max);
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => scroll(1),
            KeyCode::Up | KeyCode::Char('k') => scroll(-1),
            KeyCode::Char('d') if ctrl => scroll(half),
            KeyCode::Char('u') if ctrl => scroll(-half),
            KeyCode::Char('f') if ctrl => scroll(page),
            KeyCode::Char('b') if ctrl => scroll(-page),
            KeyCode::PageDown => scroll(page),
            KeyCode::PageUp => scroll(-page),
            KeyCode::Char('g') => d.scroll = 0,
            KeyCode::Char('G') => d.scroll = max,
            KeyCode::Char('h') => self.hscroll(-(HSTEP as isize)),
            KeyCode::Char('l') => self.hscroll(HSTEP as isize),
            KeyCode::Char('H') => self.hscroll(-wide),
            KeyCode::Char('L') => self.hscroll(wide),
            KeyCode::Home => self.hscroll(isize::MIN / 2),
            KeyCode::End => self.hscroll(isize::MAX / 2),
            _ => {}
        }
    }

    fn step_diff_file(&mut self, delta: isize) {
        let n = self.detail.as_ref().map_or(0, |d| d.files.len());
        let j = match self.diff.as_ref().and_then(|d| d.file_index) {
            Some(i) => i.saturating_add_signed(delta),
            // From the whole-commit diff, start at the highlighted file.
            None => self.file_selected,
        };
        if j < n {
            self.goto_diff_file(j);
        }
    }

    /// Next/previous file in the diff, wrapping around at either end.
    fn cycle_diff_file(&mut self, delta: isize) {
        let n = self.detail.as_ref().map_or(0, |d| d.files.len()) as isize;
        if n == 0 {
            return;
        }
        let j = match self.diff.as_ref().and_then(|d| d.file_index) {
            Some(i) => (i as isize + delta).rem_euclid(n) as usize,
            // From the whole-commit diff, start at the highlighted file.
            None => self.file_selected,
        };
        self.goto_diff_file(j);
    }

    fn goto_diff_file(&mut self, j: usize) {
        if self.diff.as_ref().and_then(|d| d.file_index) != Some(j) {
            self.file_selected = j;
            self.open_diff(Some(j));
        }
    }

    fn open_diff(&mut self, file: Option<usize>) {
        let Some(detail) = &self.detail else { return };
        let f = file.and_then(|i| detail.files.get(i));
        if file.is_some() && f.is_none() {
            return;
        }
        let short = &detail.id.to_string()[..8];
        let title = match f {
            Some(f) => format!(
                "{short} {}  [{}/{}]",
                f.path,
                file.unwrap() + 1,
                detail.files.len()
            ),
            None => format!("{short} (all files)"),
        };
        match repo::file_diff(&self.repo, detail.id, f) {
            Ok(lines) => {
                let max_width = lines
                    .iter()
                    .map(|l| l.text.chars().count())
                    .max()
                    .unwrap_or(0);
                self.diff = Some(DiffView {
                    title,
                    lines,
                    scroll: 0,
                    hscroll: 0,
                    max_width,
                    file_index: file,
                })
            }
            Err(e) => self.status = format!("Diff failed: {e:#}"),
        }
    }

    /// Jump to the merge's second parent; pressing `m` again right after
    /// moves on to the next merged parent, wrapping back to the first.
    fn cycle_merged_parent(&mut self) {
        let Some(current) = self.snap.commits.get(self.selected).map(|c| c.id) else {
            return;
        };
        let (merge, idx) = match self.merge_cycle {
            Some((merge, last, landed)) if landed == current => {
                let n = self
                    .snap
                    .index
                    .get(&merge)
                    .map_or(0, |&i| self.snap.commits[i].parents.len());
                (merge, if last + 1 < n { last + 1 } else { 1 })
            }
            _ => {
                if self.snap.commits[self.selected].parents.len() < 2 {
                    self.merge_cycle = None;
                    self.status = "Not a merge commit".into();
                    return;
                }
                (current, 1)
            }
        };
        // The merge may have vanished after a reload.
        let Some(parents) = self
            .snap
            .index
            .get(&merge)
            .map(|&i| &self.snap.commits[i].parents)
        else {
            self.merge_cycle = None;
            return;
        };
        let Some(&target) = parents.get(idx) else {
            self.merge_cycle = None;
            return;
        };
        let total = parents.len() - 1;
        let jumped = self.select_id(target);
        let landed = self.snap.commits[self.selected].id;
        self.merge_cycle = Some((merge, idx, landed));
        if jumped {
            let short = &merge.to_string()[..7];
            self.status = if total > 1 {
                format!("Merged parent {idx}/{total} of {short} (m: next)")
            } else {
                format!("Merged parent of {short}")
            };
        }
    }

    fn goto_parent(&mut self, n: usize) {
        let Some(c) = self.snap.commits.get(self.selected) else {
            return;
        };
        match c.parents.get(n).copied() {
            Some(p) => {
                self.select_id(p);
            }
            None => {
                self.status = if n == 0 {
                    "Root commit".into()
                } else {
                    "Not a merge commit".into()
                }
            }
        }
    }

    fn child(&mut self) {
        let Some(c) = self.snap.commits.get(self.selected) else {
            return;
        };
        let id = c.id;
        // Children always appear above the commit in topological order; pick the nearest.
        let child = (0..self.selected)
            .rev()
            .find(|&i| self.snap.commits[i].parents.contains(&id));
        match child {
            Some(i) => self.select(i),
            None => self.status = "No child commit".into(),
        }
    }

    fn ask_checkout(&mut self) {
        let Some(c) = self.snap.commits.get(self.selected) else {
            return;
        };
        let labels = self.snap.refs.get(&c.id);
        if labels.is_some_and(|ls| ls.iter().any(|l| l.kind == RefKind::Stash)) {
            self.status = "This is a stash; use `git stash apply` / `pop` to restore it".into();
            return;
        }
        let branch = self
            .snap
            .refs
            .get(&c.id)
            .and_then(|labels| {
                labels
                    .iter()
                    .find(|l| matches!(l.kind, RefKind::Local | RefKind::Head))
            })
            .map(|l| l.name.clone());
        let prompt = match &branch {
            Some(b) => format!("Checkout branch '{b}'? (y/N)"),
            None => format!("Checkout {} (detached HEAD)? (y/N)", c.short),
        };
        self.mode = Mode::Confirm {
            prompt,
            action: Pending::Checkout { id: c.id, branch },
        };
    }

    /// Copy the commit hash to the clipboard using the OSC 52 terminal escape.
    fn yank(&mut self) {
        let Some(c) = self.snap.commits.get(self.selected) else {
            return;
        };
        let hash = c.id.to_string();
        copy_to_clipboard(&hash);
        self.status = format!("Copied {hash}");
    }

    pub fn recompute_matches(&mut self) {
        self.matches = match &self.search {
            Some(m) => find_matches(&self.snap, m),
            None => Vec::new(),
        };
    }

    /// Keys while the search box is open: edit, search while typing, submit or cancel.
    fn key_search(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let before = self.editor.text().to_string();
        match key.code {
            KeyCode::Esc => return self.cancel_search(),
            KeyCode::Char('c') if ctrl => return self.cancel_search(),
            KeyCode::Backspace if self.editor.text().is_empty() => return self.cancel_search(),
            KeyCode::Enter => {
                let text = self.editor.text().to_string();
                push_history(&mut self.search_history, text.trim());
                self.mode = Mode::Normal;
                if text.trim().is_empty() {
                    // An empty search clears it.
                    self.restore_origin();
                    self.search = None;
                    self.matches.clear();
                } else if self.matches.is_empty() {
                    let label = self.search.as_ref().map(Matcher::label).unwrap_or_default();
                    self.status = format!("Pattern not found: {label}");
                }
                self.search_origin = None;
                return;
            }
            KeyCode::Up => self.editor.history_prev(&self.search_history),
            KeyCode::Down => self.editor.history_next(&self.search_history),
            _ => {
                edit_input(&mut self.editor, key);
            }
        }
        if self.editor.text() != before {
            self.preview_search();
        }
    }

    /// Keys in the branch picker: the filter is edited like the search box,
    /// ↑/↓ select, enter jumps, and esc (or ctrl-c) cancels.
    fn key_branches(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Mode::Branches { mut selected } = self.mode else {
            return;
        };
        let visible = branch_matches(&self.snap, self.editor.text());
        let before = self.editor.text().to_string();
        match key.code {
            KeyCode::Esc => return self.mode = Mode::Normal,
            KeyCode::Char('c') if ctrl => return self.mode = Mode::Normal,
            KeyCode::Backspace if self.editor.text().is_empty() => {
                return self.mode = Mode::Normal;
            }
            KeyCode::Down => selected = (selected + 1).min(visible.len().saturating_sub(1)),
            KeyCode::Up => selected = selected.saturating_sub(1),
            KeyCode::Enter => {
                let target = visible.get(selected).map(|&i| {
                    let b = &self.snap.branches[i];
                    (b.target, b.label.name.clone())
                });
                self.mode = Mode::Normal;
                if let Some((id, name)) = target
                    && self.select_id(id)
                {
                    self.focus = Focus::Graph;
                    self.diff = None;
                    self.status = format!("Jumped to {name}");
                }
                return;
            }
            _ => {
                edit_input(&mut self.editor, key);
            }
        }
        if self.editor.text() != before {
            selected = 0;
        }
        self.mode = Mode::Branches { selected };
    }

    /// Search while typing: highlight the matches and select the first one at or
    /// after the commit selected when the search started (wrapping around).
    fn preview_search(&mut self) {
        self.restore_origin();
        self.search = Matcher::new(self.editor.text(), false);
        self.search_hidden = false;
        self.recompute_matches();
        let from = self.selected;
        if let Some(&target) = self
            .matches
            .iter()
            .find(|&&i| i >= from)
            .or(self.matches.first())
        {
            self.focus = Focus::Graph;
            self.select(target);
        }
    }

    /// Back to the selection (and search) from before the search box opened.
    fn restore_origin(&mut self) {
        let Some(o) = &self.search_origin else { return };
        let (selected, offset, focus) = (o.selected, o.offset, o.focus);
        self.select(selected);
        self.offset = offset;
        self.focus = focus;
    }

    fn cancel_search(&mut self) {
        self.restore_origin();
        if let Some(o) = self.search_origin.take() {
            self.search = o.search;
            self.matches = o.matches;
            self.search_hidden = o.hidden;
        }
        self.mode = Mode::Normal;
    }

    /// Position of the selected commit among the matches, if it is one.
    pub fn match_position(&self) -> Option<usize> {
        self.matches.binary_search(&self.selected).ok()
    }

    fn next_match(&mut self, forward: bool, inclusive: bool) {
        if self.matches.is_empty() {
            self.status = match &self.search {
                None => "No active search (press / to search)".into(),
                Some(m) => format!("Pattern not found: {}", m.label()),
            };
            return;
        }
        let cur = self.selected;
        let found = if forward {
            self.matches
                .iter()
                .copied()
                .find(|&i| if inclusive { i >= cur } else { i > cur })
        } else {
            self.matches.iter().rev().copied().find(|&i| i < cur)
        };
        let target = found.unwrap_or_else(|| {
            if forward {
                self.status = "Search hit BOTTOM, continuing at TOP".into();
                self.matches[0]
            } else {
                self.status = "Search hit TOP, continuing at BOTTOM".into();
                *self.matches.last().unwrap()
            }
        });
        self.focus = Focus::Graph;
        self.search_hidden = false;
        self.select(target);
    }
}

/// Editing keys shared by the search box and the branch picker, the same as in
/// track's and monitor's input boxes. Returns false for keys it doesn't handle.
fn edit_input(editor: &mut LineEditor, key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Backspace => editor.backspace(),
        KeyCode::Delete => editor.delete(),
        KeyCode::Left => editor.left(),
        KeyCode::Right => editor.right(),
        KeyCode::Home => editor.home(),
        KeyCode::End => editor.end(),
        KeyCode::Char('a') if ctrl => editor.home(),
        KeyCode::Char('e') if ctrl => editor.end(),
        KeyCode::Char('u') if ctrl => editor.kill_to_start(),
        KeyCode::Char('w') if ctrl => editor.delete_word(),
        KeyCode::Char(c) if !ctrl => editor.insert(c),
        _ => return false,
    }
    true
}

/// What the search box restores on cancel.
struct SearchOrigin {
    selected: usize,
    offset: usize,
    focus: Focus,
    search: Option<Matcher>,
    matches: Vec<usize>,
    hidden: bool,
}

/// Commits whose message, author, email, hash or ref names match the search,
/// in graph order.
pub fn find_matches(snap: &Snapshot, search: &Matcher) -> Vec<usize> {
    snap.commits
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            search.is_match(&c.summary)
                || search.is_match(&c.author)
                || search.is_match(&c.email)
                || search.is_match(&c.id.to_string())
                || snap
                    .refs
                    .get(&c.id)
                    .is_some_and(|ls| ls.iter().any(|l| search.is_match(&l.name)))
        })
        .map(|(i, _)| i)
        .collect()
}

pub fn branch_matches(snap: &Snapshot, filter: &str) -> Vec<usize> {
    let f = filter.to_lowercase();
    snap.branches
        .iter()
        .enumerate()
        .filter(|(_, b)| b.label.name.to_lowercase().contains(&f))
        .map(|(i, _)| i)
        .collect()
}

/// Puts `text` on the system clipboard with the OSC 52 terminal escape.
fn copy_to_clipboard(text: &str) {
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let mut out = std::io::stdout();
    let _ = out.write_all(seq.as_bytes()).and_then(|_| out.flush());
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Whether (x, y) lies in `r`, excluding a border of `border` cells.
fn inside(r: Rect, x: u16, y: u16, border: u16) -> bool {
    x >= r.x + border
        && x + border < r.x + r.width
        && y >= r.y + border
        && y + border < r.y + r.height
}
