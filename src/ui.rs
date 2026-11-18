use chrono::{DateTime, FixedOffset, Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};

use crate::app::{App, Focus, Mode, branch_matches};
use crate::graph::Cell;
use crate::repo::{DiffKind, RefKind};

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

    match &app.mode {
        Mode::Help => draw_help(f),
        Mode::Branches { filter, selected } => draw_branches(f, app, filter, *selected),
        Mode::Search {
            input,
            results,
            selected,
        } => draw_search(f, app, input, results, *selected),
        _ => {}
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(
            " git-graph ",
            Style::new().bg(ACCENT).fg(Color::Black).bold(),
        ),
        Span::raw(" "),
        Span::styled(short_path(&app.snap.workdir), Style::new().bold()),
        Span::styled("  on ", Style::new().fg(DIM)),
        Span::styled(
            app.snap.head_name.clone(),
            Style::new().fg(PALETTE[0]).bold(),
        ),
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
                Style::new().fg(ACCENT).bold()
            } else {
                Style::new()
            },
        ))
}

fn draw_graph(f: &mut Frame, app: &mut App, area: Rect) {
    let block = panel("Commits", app.focus == Focus::Graph);
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
        let is_match = !app.query.is_empty() && app.matches.binary_search(&i).is_ok();
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
            spans.push(Span::styled(summary, summary_style));
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
        let mut first = true;
        for l in d.message.lines() {
            lines.push(if first {
                Line::styled(l.to_string(), Style::new().bold())
            } else {
                Line::raw(l.to_string())
            });
            first = false;
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
            let mut spans = vec![Span::styled(
                format!("{} ", fc.status),
                Style::new().fg(sc).bold(),
            )];
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
    f.render_stateful_widget(list, area, &mut state);
    app.files_offset = state.offset();
    app.files_area = area;
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
                DiffKind::Header => Style::new().bold(),
                DiffKind::Hunk => Style::new().fg(Color::Cyan),
                DiffKind::Add => Style::new().fg(Color::LightGreen),
                DiffKind::Del => Style::new().fg(Color::LightRed),
                DiffKind::Context => Style::new(),
            };
            Line::styled(l.text.clone(), st)
        })
        .collect();
    let hscroll = u16::try_from(d.hscroll).unwrap_or(u16::MAX);
    f.render_widget(Paragraph::new(lines).scroll((0, hscroll)), inner);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = match &app.mode {
        Mode::Confirm { prompt, .. } => Line::styled(
            prompt.clone(),
            Style::new().fg(Color::Black).bg(Color::Yellow).bold(),
        ),
        _ if !app.status.is_empty() => {
            Line::styled(format!(" {}", app.status), Style::new().fg(Color::Yellow))
        }
        _ => {
            let keys: &[(&str, &str)] = if app.diff.is_some() && app.focus == Focus::Diff {
                &[
                    ("j/k", "scroll"),
                    ("h/l", "scroll sideways"),
                    ("0/$", "line start/end"),
                    ("[ ]", "prev/next file"),
                    (
                        "z",
                        if app.diff_maximized {
                            "restore"
                        } else {
                            "maximize"
                        },
                    ),
                    ("←/→", "panes"),
                    ("esc", "back"),
                    ("?", "help"),
                ]
            } else if app.focus == Focus::Commit {
                &[
                    ("j/k", "scroll"),
                    ("←/→", "panes"),
                    ("esc", "back"),
                    ("?", "help"),
                    ("q", "quit"),
                ]
            } else if app.diff.is_some() {
                &[
                    ("j/k", "switch file"),
                    ("enter/l", "focus diff"),
                    ("←/→", "panes"),
                    ("esc", "back"),
                    ("?", "help"),
                ]
            } else {
                &[
                    ("j/k", "move"),
                    ("←/→", "panes"),
                    ("enter", "open"),
                    ("/", "search"),
                    ("b", "branches"),
                    ("p/m/c", "parent/merge/child"),
                    ("o", "checkout"),
                    ("?", "help"),
                    ("q", "quit"),
                ]
            };
            let mut spans = vec![Span::raw(" ")];
            for (k, v) in keys {
                spans.push(Span::styled(*k, Style::new().fg(ACCENT).bold()));
                spans.push(Span::styled(format!(" {v}  "), Style::new().fg(DIM)));
            }
            Line::from(spans)
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

fn draw_help(f: &mut Frame) {
    let rows: &[(&str, &str)] = &[
        ("Navigation", ""),
        ("j/k ↑/↓", "move selection"),
        ("ctrl-d/u, PgDn/PgUp", "half / full page"),
        ("g/G", "first / last commit"),
        ("H", "jump to HEAD"),
        ("p / c", "first parent / child"),
        ("m", "merged parent (repeat to cycle octopus)"),
        ("b", "branch & tag picker (type to filter)"),
        ("Search", ""),
        ("/", "search popup: message, author, hash, refs"),
        ("n / N", "next / previous match"),
        ("Panes", ""),
        ("← / →", "previous / next pane: graph, commit, files, diff"),
        ("tab", "switch focus graph ↔ files (diff ↔ files)"),
        ("j/k (commit)", "scroll the commit details"),
        ("enter / l", "open files / file diff"),
        ("D", "diff of the whole commit"),
        ("Diff (replaces the graph)", ""),
        ("tab", "switch focus diff ↔ files"),
        ("z", "maximize / restore the diff"),
        ("[ ]", "previous / next file"),
        ("j/k (files)", "switch file, diff follows"),
        ("h / l", "scroll diff sideways (H/L: half page)"),
        ("0 / $", "scroll to line start / end"),
        ("esc", "close diff, back to the graph"),
        ("Actions", ""),
        ("o", "checkout branch at commit (or detach)"),
        ("y", "copy commit hash (OSC 52)"),
        ("r", "reload repository"),
        ("q", "quit"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                Line::styled(format!(" {k}"), Style::new().fg(ACCENT).bold())
            } else {
                Line::from(vec![
                    Span::styled(format!("   {k:<22}"), Style::new().fg(Color::Yellow)),
                    Span::raw(*v),
                ])
            }
        })
        .collect();
    let area = centered(f.area(), 66, lines.len() as u16 + 2);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(panel("Help — any key to close", true)),
        area,
    );
}

fn draw_branches(f: &mut Frame, app: &App, filter: &str, selected: usize) {
    let idx = branch_matches(&app.snap, filter);
    let area = centered(f.area(), 60, 20);
    f.render_widget(Clear, area);
    let block = panel("Branches", true);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [input, list_area, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    f.render_widget(
        popup_hint(&[("↑/↓", "select"), ("enter", "jump"), ("esc", "cancel")]),
        hint,
    );
    f.render_widget(
        Line::from(vec![
            Span::styled(" filter: ", Style::new().fg(DIM)),
            Span::raw(filter.to_string()),
            Span::styled("█", Style::new().fg(ACCENT)),
        ]),
        input,
    );
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
        .highlight_style(Style::new().bg(Color::Rgb(45, 55, 75)))
        .highlight_symbol("▶ ");
    let mut state =
        ListState::default().with_selected(if idx.is_empty() { None } else { Some(selected) });
    f.render_stateful_widget(list, list_area, &mut state);
}

fn popup_hint(keys: &[(&str, &str)]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (k, v) in keys {
        spans.push(Span::styled(k.to_string(), Style::new().fg(ACCENT)));
        spans.push(Span::styled(format!(" {v}  "), Style::new().fg(DIM)));
    }
    Line::from(spans)
}

/// Search popup: query input and the live list of matching commits.
fn draw_search(f: &mut Frame, app: &App, input: &str, results: &[usize], selected: usize) {
    let full = f.area();
    let width = (full.width * 4 / 5).clamp(40, 110);
    let height = (full.height * 3 / 4).max(8);
    let area = centered(full, width, height);
    f.render_widget(Clear, area);
    let title = match (input.is_empty(), results.len()) {
        (true, _) => "Search".to_string(),
        (false, 1) => "Search — 1 match".to_string(),
        (false, n) => format!("Search — {n} matches"),
    };
    let block = panel(&title, true);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [input_area, list_area, hint] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    f.render_widget(
        Line::from(vec![
            Span::styled(" / ", Style::new().fg(ACCENT)),
            Span::raw(input.to_string()),
            Span::styled("█", Style::new().fg(ACCENT)),
            Span::styled("   message, author, hash or ref", Style::new().fg(DIM)),
        ]),
        input_area,
    );
    f.render_widget(
        popup_hint(&[
            ("↑/↓", "select"),
            ("enter", "jump"),
            ("ctrl-u", "clear"),
            ("esc", "cancel"),
        ]),
        hint,
    );

    let now = chrono::Utc::now().timestamp();
    let text_w = list_area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = results
        .iter()
        .map(|&i| {
            let c = &app.snap.commits[i];
            let mut spans = vec![
                Span::styled(c.short.clone(), Style::new().fg(Color::Yellow)),
                Span::raw(" "),
            ];
            if let Some(labels) = app.snap.refs.get(&c.id) {
                for l in labels {
                    let (t, s) =
                        ref_style(&l.name, l.kind, app.snap.label_color(l).unwrap_or(c.color));
                    spans.push(Span::styled(t, s));
                    spans.push(Span::raw(" "));
                }
            }
            let meta = format!("  {} · {}", truncate(&c.author, 16), relative(now - c.time));
            let used: usize = spans.iter().map(|s| s.width()).sum();
            let room = text_w.saturating_sub(used + meta.chars().count());
            let summary = truncate(&c.summary, room);
            let pad = room.saturating_sub(summary.chars().count());
            spans.extend(highlight(&summary, input));
            spans.push(Span::raw(" ".repeat(pad)));
            spans.push(Span::styled(meta, Style::new().fg(DIM)));
            ListItem::new(Line::from(spans))
        })
        .collect();
    if items.is_empty() && !input.is_empty() {
        f.render_widget(
            Line::styled("  no matching commits", Style::new().fg(DIM)),
            list_area,
        );
        return;
    }
    let list = List::new(items)
        .highlight_style(Style::new().bg(Color::Rgb(45, 55, 75)))
        .highlight_symbol("▶ ");
    let mut state = ListState::default().with_selected((!results.is_empty()).then_some(selected));
    f.render_stateful_widget(list, list_area, &mut state);
}

/// Split `text` into spans, emphasizing case-insensitive matches of `query`.
fn highlight(text: &str, query: &str) -> Vec<Span<'static>> {
    let lower = text.to_lowercase();
    let q = query.to_lowercase();
    // Byte offsets only line up when lowercasing keeps lengths (e.g. ASCII).
    if q.is_empty() || lower.len() != text.len() {
        return vec![Span::raw(text.to_string())];
    }
    let mut spans = Vec::new();
    let mut pos = 0;
    while let Some(found) = lower[pos..].find(&q) {
        let start = pos + found;
        let end = start + q.len();
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            break;
        }
        spans.push(Span::raw(text[pos..start].to_string()));
        spans.push(Span::styled(
            text[start..end].to_string(),
            Style::new().fg(Color::Black).bg(Color::Yellow),
        ));
        pos = end;
    }
    spans.push(Span::raw(text[pos..].to_string()));
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
