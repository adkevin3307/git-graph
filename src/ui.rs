use chrono::{DateTime, FixedOffset, Local, TimeZone};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Wrap,
};

use crate::app::{App, Focus, Mode, branch_matches};
use crate::graph::Cell;
use crate::matcher::Matcher;
use crate::repo::{DiffKind, RefKind};
use crate::selection::{self, Pane, PaneLayout};

// Fixed RGB colors (not terminal theme colors, which vary and can be dark,
// e.g. "bright blue" in Windows Terminal) chosen to stay legible on dark
// backgrounds, including the selected-row highlight.
const PALETTE: [Color; crate::graph::PALETTE_SIZE] = [
    Color::Rgb(94, 149, 217),  // blue (primary branch)
    Color::Rgb(102, 187, 102), // green
    Color::Rgb(200, 115, 200), // magenta
    Color::Rgb(204, 178, 85),  // yellow
    Color::Rgb(85, 183, 196),  // cyan
    Color::Rgb(217, 106, 106), // red
    Color::Rgb(217, 144, 76),  // orange
    Color::Rgb(157, 132, 217), // purple
    Color::Rgb(217, 132, 174), // pink
    Color::Rgb(162, 200, 85),  // lime
    Color::Rgb(187, 162, 119), // tan
    Color::Rgb(60, 178, 157),  // teal
];
const TAG: Color = Color::Rgb(212, 191, 110);
const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const YELLOW: Color = Color::Rgb(204, 178, 85);
const RED: Color = Color::Rgb(217, 106, 106);

fn color(i: usize) -> Color {
    PALETTE[i % PALETTE.len()]
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, main, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(f.area());
    app.panes.clear();
    draw_header(f, app, header);

    let wide = main.width >= 120;
    let (list, side) = if wide {
        let [a, b] = Layout::horizontal([Constraint::Percentage(62), Constraint::Percentage(38)])
            .areas(main);
        (a, b)
    } else {
        let [a, b] =
            Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(main);
        (a, b)
    };
    if app.diff_fullscreen() {
        // Maximized diff (z): use the whole area, hiding commit details and files.
        app.files_area = Rect::default();
        app.commit_area = Rect::default();
        draw_diff(f, app, main);
    } else {
        // Otherwise a diff takes the graph's place beside the details and files.
        if app.diff.is_some() {
            draw_diff(f, app, list);
        } else {
            draw_graph(f, app, list);
        }
        draw_side(f, app, side, wide);
    }
    draw_footer(f, app, footer);
    draw_selection(f.buffer_mut(), app);

    if let Mode::Help { scroll } = app.mode {
        draw_help(f, app, scroll);
    }
    match &app.mode {
        Mode::Branches { selected } => draw_branches(f, app, *selected),
        Mode::Search => draw_search(f, app),
        _ => {}
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(" git-graph ", Style::new().bg(ACCENT).fg(Color::Black)),
        Span::raw(" "),
        Span::styled(short_path(&app.snap.workdir), Style::new()),
        Span::styled("  on ", Style::new().fg(DIM)),
        Span::styled(app.snap.head_name.clone(), Style::new().fg(PALETTE[0])),
    ];
    if let Some(p) = &app.snap.primary {
        spans.push(Span::styled(format!("  trunk: {p}"), Style::new().fg(DIM)));
    }
    let count = format!(
        "{}/{}{} ",
        app.selected + 1,
        app.snap.commits.len(),
        if app.snap.truncated { "+" } else { "" }
    );
    let used: usize = spans.iter().map(|s| s.width()).sum();
    let pad = (area.width as usize).saturating_sub(used + count.len());
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(count, Style::new().fg(DIM)));
    f.render_widget(Line::from(spans), area);
}

fn panel(title: &str, focused: bool) -> Block<'_> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { ACCENT } else { DIM }))
        .title(Span::styled(
            format!(" {title} "),
            if focused {
                Style::new().fg(ACCENT)
            } else {
                Style::new()
            },
        ))
}

fn draw_graph(f: &mut Frame, app: &mut App, area: Rect) {
    let mut block = panel("Commits", app.focus == Focus::Graph);
    if let Some(title) = search_title(app) {
        block = block.title(title);
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.page = inner.height.max(1) as usize;
    app.list_top = inner.y;
    app.list_right = area.x + area.width;
    // Re-clamp in case the viewport changed size.
    if let Some(&sel_row) = app.snap.commit_row.get(app.selected)
        && sel_row >= app.offset + app.page
    {
        app.offset = sel_row + 1 - app.page;
    }

    let end = (app.offset + app.page).min(app.snap.rows.len());
    // The graph has one width for every row (like git-igitt), so text lines
    // up; cap it so text stays readable on very wide histories.
    let max_graph = (inner.width as usize * 2 / 5).max(8);
    let graph_w = app.snap.graph_width.min(max_graph);
    let show_author = inner.width >= 90;

    let now = chrono::Utc::now().timestamp();
    let mut lines = Vec::with_capacity(end.saturating_sub(app.offset));
    for r in app.offset..end {
        let row = &app.snap.rows[r];
        let mut spans: Vec<Span> = Vec::new();

        let cells: &[Cell] = &row.cells;
        let truncated = cells.len() > graph_w;
        for (x, cell) in cells.iter().take(graph_w).enumerate() {
            let ch = if truncated && x == graph_w - 1 {
                '…'
            } else {
                cell.ch()
            };
            spans.push(Span::styled(
                ch.to_string(),
                Style::new().fg(color(cell.color)),
            ));
        }
        let used = cells.len().min(graph_w);
        spans.push(Span::raw(" ".repeat(graph_w - used + 1)));

        // Connector rows carry only graph lines.
        let Some(i) = row.commit else {
            lines.push(Line::from(spans));
            continue;
        };
        let c = &app.snap.commits[i];
        let selected = i == app.selected;
        let is_match = !app.search_hidden && app.matches.binary_search(&i).is_ok();
        let hash_style = if is_match {
            Style::new().fg(Color::Black).bg(Color::Yellow)
        } else {
            Style::new().fg(Color::Yellow)
        };
        spans.push(Span::styled(c.short.clone(), hash_style));
        spans.push(Span::raw(" "));

        if let Some(labels) = app.snap.refs.get(&c.id) {
            for l in labels {
                let (text, style) =
                    ref_style(&l.name, l.kind, app.snap.label_color(l).unwrap_or(c.color));
                spans.push(Span::styled(text, style));
                spans.push(Span::raw(" "));
            }
        }
        let mut summary_style = Style::new();
        if c.parents.len() > 1 {
            summary_style = summary_style.fg(Color::Gray);
        }
        // Right-aligned author + relative date; the summary is truncated to fit.
        let meta = if show_author {
            format!(
                " {:>18} · {:>7} ",
                truncate(&c.author, 18),
                relative(now - c.time)
            )
        } else {
            format!(" {:>7} ", relative(now - c.time))
        };
        let used: usize = spans.iter().map(|s| s.width()).sum();
        let avail = inner.width as usize;
        let meta_w = meta.chars().count();
        let room = avail.saturating_sub(used + meta_w);
        if room >= 12 {
            let summary = truncate(&c.summary, room);
            let pad = room - summary.chars().count();
            let search = app.search.as_ref().filter(|_| is_match);
            spans.extend(highlight(&summary, search, summary_style));
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(meta, Style::new().fg(DIM)));
        } else {
            spans.push(Span::styled(c.summary.clone(), summary_style));
        }
        let mut line = Line::from(spans);
        if selected {
            let bg = if app.focus == Focus::Graph {
                Color::Rgb(45, 55, 75)
            } else {
                Color::Rgb(35, 38, 45)
            };
            line = line.style(Style::new().bg(bg));
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), inner);
    let rows = app.snap.rows.len();
    draw_scroll(
        f,
        area,
        app.focus == Focus::Graph,
        app.offset,
        app.page,
        rows,
    );
    app.panes.push(PaneLayout {
        pane: Pane::Graph,
        area: inner,
        row_offset: app.offset,
        col_offset: 0,
    });
}

/// Records the selected pane's rows and highlights the selection.
fn draw_selection(buf: &mut Buffer, app: &mut App) {
    app.resolve_drag();
    let Some(mut sel) = app.selection.take() else {
        return;
    };
    if let Some(layout) = app.panes.iter().find(|p| p.pane == sel.pane).copied() {
        if sel.pane != Pane::Diff {
            let a = layout.area;
            for y in a.y..a.bottom() {
                let row = layout.row_offset + (y - a.y) as usize;
                sel.rows.insert(row, selection::screen_row(buf, a, y));
            }
        }
        selection::highlight(buf, &layout, &sel, |row| app.selection_row_text(&sel, row));
    }
    app.selection = Some(sel);
}

fn ref_style(name: &str, kind: RefKind, lane_color: usize) -> (String, Style) {
    match kind {
        RefKind::Head => (
            format!("[HEAD → {name}]"),
            Style::new().fg(color(lane_color)),
        ),
        RefKind::Local => (format!("[{name}]"), Style::new().fg(color(lane_color))),
        RefKind::Remote => (
            format!("[{name}]"),
            Style::new()
                .fg(color(lane_color))
                .add_modifier(Modifier::ITALIC),
        ),
        RefKind::Tag => (format!("<{name}>"), Style::new().fg(TAG)),
        RefKind::Stash => (format!("[{name}]"), Style::new().fg(color(lane_color))),
    }
}

fn draw_side(f: &mut Frame, app: &mut App, area: Rect, wide: bool) {
    let [info, files] = if wide {
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(area)
    } else {
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(area)
    };

    let Some(c) = app.snap.commits.get(app.selected) else {
        f.render_widget(panel("Commit", false), info);
        f.render_widget(panel("Files", false), files);
        return;
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("hash      ", Style::new().fg(DIM)),
            Span::styled(c.id.to_string(), Style::new().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("author    ", Style::new().fg(DIM)),
            Span::raw(format!("{} <{}>", c.author, c.email)),
        ]),
        Line::from(vec![
            Span::styled("date      ", Style::new().fg(DIM)),
            Span::raw(fmt_time(c.time, c.offset_min)),
        ]),
    ];
    if let Some(d) = &app.detail
        && (!d.committer.starts_with(&format!("{} <", c.author)) || d.commit_time != c.time)
    {
        lines.push(Line::from(vec![
            Span::styled("committer ", Style::new().fg(DIM)),
            Span::raw(format!(
                "{} · {}",
                d.committer,
                fmt_time(d.commit_time, d.commit_offset)
            )),
        ]));
    }
    if !c.parents.is_empty() {
        let mut spans = vec![Span::styled(
            if c.parents.len() > 1 {
                "parents   "
            } else {
                "parent    "
            },
            Style::new().fg(DIM),
        )];
        for p in &c.parents {
            let s = p.to_string();
            let loaded = app.snap.index.contains_key(p);
            spans.push(Span::styled(
                s[..8].to_string(),
                Style::new().fg(if loaded { Color::Yellow } else { DIM }),
            ));
            spans.push(Span::raw(" "));
        }
        lines.push(Line::from(spans));
    }
    if let Some(b) = &c.branch {
        lines.push(Line::from(vec![
            Span::styled("branch    ", Style::new().fg(DIM)),
            Span::styled(b.clone(), Style::new().fg(color(c.color))),
        ]));
    }
    if let Some(labels) = app.snap.refs.get(&c.id) {
        let mut spans = vec![Span::styled("refs      ", Style::new().fg(DIM))];
        for l in labels {
            let (t, s) = ref_style(&l.name, l.kind, app.snap.label_color(l).unwrap_or(c.color));
            spans.push(Span::styled(t, s));
            spans.push(Span::raw(" "));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::raw(""));
    if let Some(d) = &app.detail {
        for l in d.message.lines() {
            lines.push(Line::raw(l.to_string()));
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.commit_scroll, 0))
            .block(panel("Commit", app.focus == Focus::Commit)),
        info,
    );
    app.commit_area = info;
    app.panes.push(PaneLayout {
        pane: Pane::Commit,
        area: info.inner(Margin::new(1, 1)),
        row_offset: app.commit_scroll as usize,
        col_offset: 0,
    });

    draw_files(f, app, files, app.focus == Focus::Files);
}

/// Changed-files list for the selected commit (side pane and diff view).
fn draw_files(f: &mut Frame, app: &mut App, area: Rect, focused: bool) {
    let Some(d) = &app.detail else {
        f.render_widget(panel("Files", focused), area);
        return;
    };
    let (adds, dels): (usize, usize) = d
        .files
        .iter()
        .fold((0, 0), |(a, r), x| (a + x.added, r + x.removed));
    let title = format!("Files ({})  +{adds} -{dels}", d.files.len());
    let items: Vec<ListItem> = d
        .files
        .iter()
        .map(|fc| {
            let sc = match fc.status {
                'A' => Color::LightGreen,
                'D' => Color::LightRed,
                'R' | 'C' => Color::LightMagenta,
                _ => Color::LightYellow,
            };
            let mut spans = vec![Span::styled(format!("{} ", fc.status), Style::new().fg(sc))];
            if let Some(old) = &fc.old_path {
                spans.push(Span::styled(format!("{old} → "), Style::new().fg(DIM)));
            }
            spans.push(Span::raw(fc.path.clone()));
            spans.push(Span::styled(
                format!(" +{}", fc.added),
                Style::new().fg(Color::Green),
            ));
            spans.push(Span::styled(
                format!(" -{}", fc.removed),
                Style::new().fg(Color::Red),
            ));
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items)
        .block(panel(&title, focused))
        .highlight_style(Style::new().bg(if focused {
            Color::Rgb(45, 55, 75)
        } else {
            Color::Rgb(35, 38, 45)
        }))
        .highlight_symbol(if focused { "▶ " } else { "  " });
    // In the whole-commit diff no single file is current.
    let selected = match &app.diff {
        Some(diff) => diff.file_index,
        None => Some(app.file_selected),
    };
    let mut state = ListState::default()
        .with_selected(selected)
        .with_offset(app.files_offset);
    let files = d.files.len();
    f.render_stateful_widget(list, area, &mut state);
    app.files_offset = state.offset();
    app.files_area = area;
    let visible = area.height.saturating_sub(2) as usize;
    draw_scroll(f, area, focused, app.files_offset, visible, files);
    app.panes.push(PaneLayout {
        pane: Pane::Files,
        area: area.inner(Margin::new(1, 1)),
        row_offset: app.files_offset,
        col_offset: 0,
    });
}

fn draw_diff(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(d) = &app.diff else { return };
    let title = if d.hscroll > 0 {
        format!("{}  ← col {}", d.title, d.hscroll + 1)
    } else {
        d.title.clone()
    };
    let block = panel(&title, app.focus == Focus::Diff);
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.page = inner.height.max(1) as usize;
    app.diff_area = inner;
    // The panel width changes with focus; keep the scroll within the new bounds.
    let max_h = d.max_width.saturating_sub(inner.width as usize);
    let d = match app.diff.as_mut() {
        Some(d) => {
            d.hscroll = d.hscroll.min(max_h);
            &*d
        }
        None => return,
    };
    let lines: Vec<Line> = d
        .lines
        .iter()
        .skip(d.scroll)
        .take(inner.height as usize)
        .map(|l| {
            let st = match l.kind {
                DiffKind::Header => Style::new(),
                DiffKind::Hunk => Style::new().fg(Color::Cyan),
                DiffKind::Add => Style::new().fg(Color::LightGreen),
                DiffKind::Del => Style::new().fg(Color::LightRed),
                DiffKind::Context => Style::new(),
            };
            Line::styled(l.text.clone(), st)
        })
        .collect();
    let (row_offset, col_offset, total) = (d.scroll, d.hscroll, d.lines.len());
    let hscroll = u16::try_from(col_offset).unwrap_or(u16::MAX);
    f.render_widget(Paragraph::new(lines).scroll((0, hscroll)), inner);
    let height = inner.height as usize;
    draw_scroll(f, area, app.focus == Focus::Diff, row_offset, height, total);
    app.panes.push(PaneLayout {
        pane: Pane::Diff,
        area: inner,
        row_offset,
        col_offset,
    });
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = match &app.mode {
        Mode::Confirm { prompt, .. } => Line::styled(
            prompt.clone(),
            Style::new().fg(Color::Black).bg(Color::Yellow),
        ),
        _ if !app.status.is_empty() => {
            Line::styled(format!(" {}", app.status), Style::new().fg(YELLOW))
        }
        _ => {
            // Same order as track and monitor: movement, search, this pane's own
            // keys, back, then help and quit.
            let keys: &[(&str, &str)] = if app.diff.is_some() && app.focus == Focus::Diff {
                &[
                    ("j/k", "down/up"),
                    ("g/G", "top/bottom"),
                    ("h/l", "left/right"),
                    ("tab", "next file"),
                    ("←/→", "panes"),
                    ("esc", "back"),
                    ("?", "help"),
                    ("q", "quit"),
                ]
            } else if app.focus == Focus::Commit {
                &[
                    ("j/k", "down/up"),
                    ("g/G", "top/bottom"),
                    ("/", "search"),
                    ("n/N", "next/prev"),
                    ("←/→", "panes"),
                    ("esc", "back"),
                    ("?", "help"),
                    ("q", "quit"),
                ]
            } else if app.diff.is_some() {
                &[
                    ("j/k", "down/up"),
                    ("g/G", "top/bottom"),
                    ("enter", "open"),
                    ("←/→", "panes"),
                    ("esc", "back"),
                    ("?", "help"),
                    ("q", "quit"),
                ]
            } else {
                &[
                    ("j/k", "down/up"),
                    ("g/G", "top/bottom"),
                    ("/", "search"),
                    ("n/N", "next/prev"),
                    ("←/→", "panes"),
                    ("enter", "open"),
                    ("b", "branches"),
                    ("?", "help"),
                    ("q", "quit"),
                ]
            };
            footer_hints(keys, area.width)
        }
    };
    f.render_widget(line, area);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn draw_help(f: &mut Frame, app: &mut App, scroll: u16) {
    // The same sections and wording as track's and monitor's help.
    let rows: &[(&str, &str)] = &[
        ("Navigation", ""),
        ("j/k ↓/↑", "down / up"),
        ("ctrl-d/u", "half page down / up"),
        ("PgDn/PgUp, ctrl-f/b", "page down / up"),
        ("g/G", "top / bottom"),
        ("h/l", "left / right 4 columns (diff)"),
        ("H/L", "left / right half a page (diff)"),
        ("home/end", "line start / end (diff)"),
        (
            "← / →",
            "previous / next pane: commits, commit, files, diff",
        ),
        ("enter", "open: the files, then a file's diff"),
        ("Search", ""),
        (
            "/",
            "search (regex, smart case; message, author, hash, refs)",
        ),
        ("n / N", "next / previous match"),
        ("esc", "hide search highlights, then back to the commits"),
        ("Commits", ""),
        ("H", "jump to HEAD"),
        ("p / c", "first parent / child"),
        ("m", "merged parent (repeat to cycle)"),
        ("b", "branch, tag & stash picker"),
        ("D", "diff of the whole commit"),
        ("o", "checkout the branch at the commit (or detach)"),
        ("y", "copy the commit hash"),
        ("Diff", ""),
        ("tab / shift-tab", "next / previous file"),
        ("z", "maximize / restore"),
        ("esc", "back to the commits"),
        ("Input box", ""),
        ("←/→ home/end ctrl-a/e", "move the cursor"),
        ("ctrl-u / ctrl-w", "delete to start / previous word"),
        ("↑/↓", "history (branch picker: select)"),
        ("enter", "submit"),
        ("esc, ctrl-c", "cancel"),
        ("Mouse", ""),
        ("wheel", "scroll (ctrl: faster)"),
        ("click", "select a commit, file or pane"),
        ("drag", "select text; copied on release"),
        ("shift-drag", "the terminal's own selection"),
        ("General", ""),
        ("r", "refresh (reload the repository)"),
        ("?", "help"),
        ("q, ctrl-c", "quit"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                Line::styled(format!(" {k}"), Style::new().fg(ACCENT))
            } else {
                Line::from(vec![
                    Span::styled(format!("   {k:<24}"), Style::new().fg(YELLOW)),
                    Span::raw(*v),
                ])
            }
        })
        .collect();
    // The same help window as track and monitor.
    let area = centered(f.area(), 88, lines.len() as u16 + 2);
    let max = (lines.len() as u16).saturating_sub(area.height.saturating_sub(2));
    app.help_max = max;
    let title = if max > 0 {
        "Help — j/k scroll, any other key closes"
    } else {
        "Help — any key to close"
    };
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .scroll((scroll.min(max), 0))
            .block(panel(title, true)),
        area,
    );
}

/// Footer key hints, the same in track, monitor and git-graph: movement, search,
/// the tool's own keys, then `? help` and `q quit` last. When the line is too
/// narrow, the tool's own keys are dropped from the end first, so help and quit
/// always stay visible.
fn footer_hints(keys: &[(&str, &str)], width: u16) -> Line<'static> {
    let hint_width = |(k, v): &(&str, &str)| Line::raw(*k).width() + Line::raw(*v).width() + 3;
    let mut keys = keys.to_vec();
    while keys.len() > 2 && 1 + keys.iter().map(hint_width).sum::<usize>() > width as usize {
        keys.remove(keys.len() - 3);
    }
    let mut spans = vec![Span::raw(" ")];
    for (k, v) in keys {
        spans.push(Span::styled(k.to_string(), Style::new().fg(ACCENT)));
        spans.push(Span::styled(format!(" {v}  "), Style::new().fg(DIM)));
    }
    Line::from(spans)
}

/// A scrollbar on the right border of a panel, the same in track, monitor and
/// git-graph. It takes the border's color (cyan when the panel is focused) and
/// is drawn only when the content doesn't fit.
fn draw_scroll(
    f: &mut Frame,
    panel: Rect,
    focused: bool,
    offset: usize,
    visible: usize,
    total: usize,
) {
    if total <= visible || panel.height < 3 {
        return;
    }
    let style = Style::new().fg(if focused { ACCENT } else { DIM });
    let mut state = ScrollbarState::new(total - visible)
        .position(offset)
        .viewport_content_length(visible);
    f.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .thumb_style(style)
            .track_style(style),
        panel.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut state,
    );
}

/// Branch, tag & stash picker: the shared input box with the list below it.
fn draw_branches(f: &mut Frame, app: &App, selected: usize) {
    let filter = app.editor.text();
    let idx = branch_matches(&app.snap, filter);
    let info = Span::styled(
        format!("{}/{}", idx.len(), app.snap.branches.len()),
        Style::new().fg(DIM),
    );
    let input = draw_input(
        f,
        "Branches",
        "\u{e725}",
        filter,
        app.editor.cursor(),
        Some(info),
        &[("↑/↓", "select"), ("enter", "jump"), ("esc", "cancel")],
    );
    let Some(input) = input else { return };
    let full = f.area();
    let height = (idx.len() as u16 + 2)
        .clamp(3, 16)
        .min(full.bottom().saturating_sub(input.bottom()));
    if height < 3 {
        return;
    }
    let area = Rect {
        y: input.bottom(),
        height,
        ..input
    };
    let items: Vec<ListItem> = idx
        .iter()
        .map(|&i| {
            let b = &app.snap.branches[i];
            let lane_color = app
                .snap
                .index
                .get(&b.target)
                .map(|&i| app.snap.commits[i].color)
                .unwrap_or(0);
            let (t, s) = ref_style(
                &b.label.name,
                b.label.kind,
                app.snap.label_color(&b.label).unwrap_or(lane_color),
            );
            ListItem::new(Line::from(vec![
                Span::styled(t, s),
                Span::styled(
                    format!("  {}", &b.target.to_string()[..8]),
                    Style::new().fg(DIM),
                ),
            ]))
        })
        .collect();
    let list = List::new(items)
        .block(
            Block::new()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().fg(DIM)),
        )
        .highlight_style(Style::new().bg(Color::Rgb(45, 55, 75)))
        .highlight_symbol("▶ ");
    let mut state =
        ListState::default().with_selected(if idx.is_empty() { None } else { Some(selected) });
    f.render_widget(Clear, area);
    f.render_stateful_widget(list, area, &mut state);
}

/// The search box, the same as in track and monitor.
fn draw_search(f: &mut Frame, app: &App) {
    let info = (!app.editor.text().is_empty()).then(|| match app.matches.len() {
        0 => Span::styled("no match", Style::new().fg(RED)),
        1 => Span::styled("1 match", Style::new().fg(DIM)),
        n => Span::styled(format!("{n} matches"), Style::new().fg(DIM)),
    });
    draw_input(
        f,
        "Search",
        "\u{f002}",
        app.editor.text(),
        app.editor.cursor(),
        info,
        &[("enter", "search"), ("↑/↓", "history"), ("esc", "cancel")],
    );
}

/// The search and the current match for the Commits title; dimmed while the
/// highlights are hidden.
fn search_title(app: &App) -> Option<Line<'static>> {
    let search = app
        .search
        .as_ref()
        .filter(|_| !matches!(app.mode, Mode::Search))?;
    let count = match (app.match_position(), app.matches.len()) {
        _ if app.search_hidden => "hidden".to_string(),
        (_, 0) => "no match".to_string(),
        (Some(i), n) => format!("{}/{n}", i + 1),
        (None, 1) => "1 match".to_string(),
        (None, n) => format!("{n} matches"),
    };
    let style = if app.search_hidden { DIM } else { YELLOW };
    Some(
        Line::from(vec![
            Span::styled(format!(" /{}", search.label()), Style::new().fg(style)),
            Span::styled(format!("  {count} "), Style::new().fg(DIM)),
        ])
        .right_aligned(),
    )
}

/// A one-line input box: rounded border, centered title, icon, block cursor,
/// optional info on the right and key hints in the bottom border.
fn draw_input(
    f: &mut Frame,
    title: &str,
    icon: &str,
    text: &str,
    cursor: usize,
    info: Option<Span<'static>>,
    hints: &[(&str, &str)],
) -> Option<Rect> {
    let full = f.area();
    let width = full.width.saturating_sub(4).min(60);
    if width < 10 || full.height < 3 {
        return None;
    }
    let area = Rect {
        x: full.x + (full.width - width) / 2,
        y: full.y + 2.min(full.height - 3),
        width,
        height: 3,
    };
    let mut hint_spans = vec![Span::raw(" ")];
    for (i, (k, v)) in hints.iter().enumerate() {
        if i > 0 {
            hint_spans.push(Span::styled(" · ", Style::new().fg(DIM)));
        }
        hint_spans.push(Span::styled(k.to_string(), Style::new().fg(ACCENT)));
        hint_spans.push(Span::styled(format!(" {v}"), Style::new().fg(DIM)));
    }
    hint_spans.push(Span::raw(" "));
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(Line::styled(format!(" {title} "), Style::new().fg(ACCENT)).centered())
        .title_bottom(Line::from(hint_spans).centered());
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    let info_width = info.as_ref().map_or(0, |s| s.width() as u16 + 1);
    let [input, info_area] =
        Layout::horizontal([Constraint::Min(1), Constraint::Length(info_width)]).areas(inner);
    // Keep the cursor in view: drop characters from the start of long input
    let room = (input.width as usize).saturating_sub(4);
    let (mut before, after) = text.split_at(cursor);
    while Line::raw(before).width() > room {
        let mut chars = before.chars();
        chars.next();
        before = chars.as_str();
    }
    let mut after_chars = after.chars();
    let spans = vec![
        Span::styled(format!(" {icon} "), Style::new().fg(ACCENT)),
        Span::raw(before.to_string()),
        match after_chars.next() {
            Some(c) => Span::styled(c.to_string(), Style::new().reversed()),
            None => Span::styled("█", Style::new().fg(ACCENT)),
        },
        Span::raw(after_chars.as_str().to_string()),
    ];
    f.render_widget(Line::from(spans), input);
    if let Some(info) = info {
        f.render_widget(Line::from(info).right_aligned(), info_area);
    }
    Some(area)
}

/// Split `text` into spans, emphasizing the matches of `search`.
fn highlight(text: &str, search: Option<&Matcher>, base: Style) -> Vec<Span<'static>> {
    let Some(search) = search else {
        return vec![Span::styled(text.to_string(), base)];
    };
    let mut spans = Vec::new();
    let mut pos = 0;
    for r in search.find_ranges(text) {
        spans.push(Span::styled(text[pos..r.start].to_string(), base));
        spans.push(Span::styled(
            text[r.clone()].to_string(),
            Style::new().fg(Color::Black).bg(Color::Yellow),
        ));
        pos = r.end;
    }
    spans.push(Span::styled(text[pos..].to_string(), base));
    spans
}

fn short_path(p: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && p.starts_with(&home) => format!("~{}", &p[home.len()..]),
        _ => p.to_string(),
    }
    .rsplit('/')
    .take(2)
    .collect::<Vec<_>>()
    .into_iter()
    .rev()
    .collect::<Vec<_>>()
    .join("/")
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else if n == 0 {
        String::new()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

fn relative(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m ago", s / 60),
        3600..86400 => format!("{}h ago", s / 3600),
        86400..2_592_000 => format!("{}d ago", s / 86400),
        2_592_000..31_536_000 => format!("{}mo ago", s / 2_592_000),
        _ => format!("{}y ago", s / 31_536_000),
    }
}

fn fmt_time(secs: i64, offset_min: i32) -> String {
    let tz =
        FixedOffset::east_opt(offset_min * 60).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
    match tz.timestamp_opt(secs, 0).single() {
        Some(t) => {
            let local: DateTime<Local> = t.with_timezone(&Local);
            format!(
                "{} ({})",
                t.format("%Y-%m-%d %H:%M %:z"),
                local.format("%a")
            )
        }
        None => secs.to_string(),
    }
}
