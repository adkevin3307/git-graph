//! Branch-based commit graph layout, modelled on git-igitt's renderer
//! (the gleisbau crate), so graphs look the same:
//!
//! * Commits are attributed to branches ("tracks") by tracing first parents
//!   from branch tips and from merge commits (named via merge messages), in
//!   persistence order (main, develop, feature, release, hotfix, bugfix, other).
//! * Each track owns one column for its whole extent. Columns are grouped
//!   git-flow style (main | hotfix/release | develop | other) and filled
//!   shortest-branch-first.
//! * A connection between columns is a zig-zag: down the child's column,
//!   across on a "split" row, down the parent's column. Forks split on the row
//!   just above the parent (`├───╯`), merges on the merge commit's own row
//!   with an arrow (`○<──╮`). Split rows that would collide get extra
//!   connector rows; in compact mode they share commit rows when free.
//!
//! Differences from gleisbau: every parent of an octopus merge is connected
//! (gleisbau draws only the first two), and colors come from the caller.

use std::cmp::max;
use std::collections::HashMap;

pub type Id = git2::Oid;

/// Number of colors in the palette. Color 0 is the primary branch.
pub const PALETTE_SIZE: usize = 12;

// Glyph codes, in gleisbau's order.
const SPACE: u8 = 0;
const DOT: u8 = 1;
const CIRCLE: u8 = 2;
const VER: u8 = 3;
const HOR: u8 = 4;
const CROSS: u8 = 5;
const R_U: u8 = 6;
const R_D: u8 = 7;
const L_D: u8 = 8;
const L_U: u8 = 9;
const VER_L: u8 = 10;
const VER_R: u8 = 11;
const HOR_U: u8 = 12;
const HOR_D: u8 = 13;
const ARR_L: u8 = 14;
const ARR_R: u8 = 15;
const CHARS: [char; 16] = [
    ' ', '●', '○', '│', '─', '┼', '╰', '╭', '╮', '╯', '┤', '├', '┴', '┬', '<', '>',
];

/// Persistence of an unclassified branch (lowest priority).
pub const PERS_OTHER: u8 = 6;
/// Persistence of an empty cell: anything drawn overrides it.
const PERS_EMPTY: u8 = PERS_OTHER + 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    code: u8,
    pub color: usize,
    pers: u8,
}

impl Cell {
    const EMPTY: Cell = Cell {
        code: SPACE,
        color: 0,
        pers: PERS_EMPTY,
    };

    pub fn ch(&self) -> char {
        CHARS[self.code as usize]
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    /// Commit drawn on this row; `None` for connector-only rows.
    pub commit: Option<usize>,
    pub cells: Vec<Cell>,
}

pub struct Node<'a> {
    pub id: Id,
    pub parents: &'a [Id],
}

/// A branch to trace: an actual branch tip, or a branch merged at `merge`
/// (then `start` is the merged parent and the name comes from the message).
pub struct BranchSpec {
    pub name: Option<String>,
    pub start: Id,
    pub merge: Option<usize>,
    /// Claim priority (lower claims shared commits first).
    pub persistence: u8,
    /// Column group (lower groups are further left).
    pub group: usize,
    pub color: usize,
    /// Color to use if this merged branch turns out to be a fork (same name
    /// as the branch it was merged into); it is then renamed `fork/<name>`.
    pub fork_color: usize,
}

pub struct Graph {
    pub rows: Vec<Row>,
    /// Row index of each commit.
    pub commit_row: Vec<usize>,
    pub commit_branch: Vec<Option<String>>,
    pub commit_color: Vec<usize>,
    /// Width of the graph in cells (same for every row).
    pub width: usize,
}

struct Track {
    name: Option<String>,
    persistence: u8,
    group: usize,
    color: usize,
    fork_color: usize,
    merge_target: Option<usize>,
    /// (top, bottom) row extent; `None` = open-ended.
    range: (Option<usize>, Option<usize>),
    source: Option<usize>,
    target: Option<usize>,
    column: Option<usize>,
}

impl Track {
    fn new(
        name: Option<String>,
        persistence: u8,
        group: usize,
        color: usize,
        merge: Option<usize>,
    ) -> Self {
        Track {
            name,
            persistence,
            group,
            color,
            fork_color: color,
            merge_target: merge,
            // A merged branch's column starts below its merge commit.
            range: (merge.map(|m| m + 1), None),
            source: None,
            target: None,
            column: None,
        }
    }
}

#[derive(Clone, Copy)]
enum Occ {
    Commit(usize, usize),
    Range(usize, usize, usize, usize),
}

impl Occ {
    fn overlaps(&self, (start, end): (usize, usize)) -> bool {
        match *self {
            Occ::Commit(_, col) => start <= col && end >= col,
            Occ::Range(_, _, s, e) => s <= end && e >= start,
        }
    }
}

/// `groups`: number of named column groups; unmatched branches use group
/// `groups`. `compact`: let connector lines share commit rows when free.
pub fn layout(nodes: &[Node], mut specs: Vec<BranchSpec>, groups: usize, compact: bool) -> Graph {
    let n = nodes.len();
    let index: HashMap<Id, usize> = nodes.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, c) in nodes.iter().enumerate() {
        for p in c.parents {
            if let Some(&pi) = index.get(p) {
                children[pi].push(i);
            }
        }
    }
    let is_merge = |i: usize| nodes[i].parents.len() > 1;

    // 1. Trace branches; within a persistence level merged branches go first.
    specs.sort_by_key(|s| (s.persistence, s.merge.is_none()));
    let mut tracks: Vec<Track> = Vec::new();
    let mut trace_of: Vec<Option<usize>> = vec![None; n];
    for s in specs {
        tracks.push(Track::new(s.name, s.persistence, s.group, s.color, s.merge));
        let t = tracks.len() - 1;
        tracks[t].fork_color = s.fork_color;
        trace(
            nodes,
            &index,
            &children,
            &mut trace_of,
            &mut tracks,
            s.start,
            t,
        );
    }
    // Commits no branch reached get anonymous tracks.
    for i in 0..n {
        if trace_of[i].is_none() {
            tracks.push(Track::new(
                None,
                PERS_OTHER,
                groups,
                anonymous_color(nodes[i].id),
                None,
            ));
            let t = tracks.len() - 1;
            trace(
                nodes,
                &index,
                &children,
                &mut trace_of,
                &mut tracks,
                nodes[i].id,
                t,
            );
        }
    }
    let trace_of: Vec<usize> = trace_of
        .into_iter()
        .map(|t| t.expect("every commit is traced"))
        .collect();

    // 2. Sources (forked from) and targets (merged into). A branch merged
    // into a branch of the same name (e.g. a pull request from a fork's
    // "master") is a fork: rename it and put it with the other branches.
    for t in 0..tracks.len() {
        let target = tracks[t].merge_target.map(|m| trace_of[m]);
        tracks[t].target = target;
        if let Some(tt) = target
            && tt != t
            && tracks[t].name.is_some()
            && tracks[tt].name == tracks[t].name
        {
            let name = tracks[t].name.take().unwrap();
            tracks[t].name = Some(format!("fork/{name}"));
            tracks[t].group = groups;
            tracks[t].color = tracks[t].fork_color;
        }
    }
    for i in 0..n {
        for p in nodes[i].parents {
            if let Some(&pi) = index.get(p)
                && trace_of[pi] != trace_of[i]
            {
                tracks[trace_of[i]].source = Some(trace_of[pi]);
            }
        }
    }

    // 3. Columns.
    assign_columns(&mut tracks, &trace_of, groups, n);
    let col: Vec<usize> = (0..n)
        .map(|i| tracks[trace_of[i]].column.unwrap())
        .collect();

    // 4. Connector rows: where each cross-column connection turns.
    let deviate = |i: usize, par: usize| -> usize {
        if !is_merge(i) {
            return par - 1;
        }
        let siblings = children[par]
            .iter()
            .filter(|&&s| s != i && col[s] == col[par]);
        siblings.copied().fold(i, max)
    };
    let mut inserts: Vec<Vec<Vec<Occ>>> =
        (0..n).map(|i| vec![vec![Occ::Commit(i, col[i])]]).collect();
    for i in 0..n {
        for (k, p) in nodes[i].parents.iter().enumerate() {
            let Some(&par) = index.get(p) else { continue };
            let (c, pc) = (col[i], col[par]);
            if c == pc {
                continue;
            }
            let span = (c.min(pc), c.max(pc));
            let rows = &mut inserts[deviate(i, par)];
            let free = rows.iter().position(|row| {
                !row.iter().any(|o| {
                    o.overlaps(span)
                        && match *o {
                            // A merge may draw its secondary parents on its own row.
                            Occ::Commit(target, _) => {
                                !compact || !is_merge(i) || i != target || k == 0
                            }
                            Occ::Range(oi, op, _, _) => i != oi && par != op,
                        }
                })
            });
            let occ = Occ::Range(i, par, span.0, span.1);
            match free {
                Some(r) => rows[r].push(occ),
                None => rows.push(vec![occ]),
            }
        }
    }
    let mut commit_row = Vec::with_capacity(n);
    let mut total = 0;
    for rows in &inserts {
        commit_row.push(total);
        total += rows.len();
    }

    // 5. Draw.
    let max_col = col.iter().copied().max().unwrap_or(0);
    let width = 2 * max_col + 1;
    let mut grid = Grid {
        width,
        cells: vec![Cell::EMPTY; width * total],
    };
    for i in 0..n {
        let t = &tracks[trace_of[i]];
        let (c, row) = (col[i], commit_row[i]);
        grid.set(
            c * 2,
            row,
            if is_merge(i) { CIRCLE } else { DOT },
            t.color,
            t.persistence,
        );
        for (k, p) in nodes[i].parents.iter().enumerate() {
            let Some(&par) = index.get(p) else {
                // Parent not loaded: run the line off the bottom.
                vline(&mut grid, (row, total), c, t.color, t.persistence);
                continue;
            };
            let pt = &tracks[trace_of[par]];
            let (color, pers) = if is_merge(i) {
                (pt.color, pt.persistence)
            } else {
                (t.color, t.persistence)
            };
            let (pc, prow) = (col[par], commit_row[par]);
            if c == pc {
                if prow > row + 1 {
                    vline(&mut grid, (row, prow), c, color, pers);
                }
                continue;
            }
            let split = deviate(i, par);
            let sub = inserts[split]
                .iter()
                .position(|r| {
                    r.iter()
                        .any(|o| matches!(*o, Occ::Range(a, b, _, _) if a == i && b == par))
                })
                .expect("connection has a row");
            let srow = commit_row[split] + sub;
            vline(&mut grid, (row, srow), c, color, pers);
            hline(&mut grid, srow, (pc, c), is_merge(i) && k > 0, color, pers);
            vline(&mut grid, (srow, prow), pc, color, pers);
        }
    }

    let mut rows: Vec<Row> = (0..total)
        .map(|r| Row {
            commit: None,
            cells: grid.cells[r * width..(r + 1) * width].to_vec(),
        })
        .collect();
    for (i, &r) in commit_row.iter().enumerate() {
        rows[r].commit = Some(i);
    }
    Graph {
        rows,
        commit_row,
        commit_branch: trace_of.iter().map(|&t| tracks[t].name.clone()).collect(),
        commit_color: trace_of.iter().map(|&t| tracks[t].color).collect(),
        width,
    }
}

/// Follow first parents from `start`, claiming commits for track `t` until
/// one already claimed by another branch (the fork point) is reached.
fn trace(
    nodes: &[Node],
    index: &HashMap<Id, usize>,
    children: &[Vec<usize>],
    trace_of: &mut [Option<usize>],
    tracks: &mut [Track],
    start: Id,
    t: usize,
) {
    let mut cur = start;
    let mut prev: Option<usize> = None;
    let mut bottom: Option<i64> = None;
    while let Some(&i) = index.get(&cur) {
        if let Some(old) = trace_of[i] {
            let same = tracks[t].name.is_some() && tracks[old].name == tracks[t].name;
            if same && tracks[old].range.0.unwrap_or(0) >= tracks[t].range.0.unwrap_or(0) {
                // The same branch merged again further down: extend the old one.
                tracks[old].range.0 = Some(i);
                if tracks[old].range.1.is_some_and(|limit| i > limit) {
                    tracks[old].range = (None, None);
                }
            } else {
                bottom = Some(match prev {
                    Some(p) if nodes[p].parents.len() > 1 => {
                        children[i].iter().copied().fold(p, max) as i64
                    }
                    _ => i as i64 - 1,
                });
                break;
            }
        }
        trace_of[i] = Some(t);
        match nodes[i].parents.first() {
            None => {
                bottom = Some(i as i64);
                break;
            }
            Some(&p) => {
                prev = Some(i);
                cur = p;
            }
        }
    }
    let tr = &mut tracks[t];
    tr.range = match (tr.range.0, bottom) {
        (Some(top), Some(b)) if b < top as i64 => (None, None),
        (top, b) => (top, b.map(|b| b.max(0) as usize)),
    };
}

/// Give every visible track a column: per group, shortest branches first,
/// each in the leftmost column free over its row range.
fn assign_columns(tracks: &mut [Track], trace_of: &[usize], groups: usize, n: usize) {
    // Visible tracks, in order of first appearance.
    let mut visible: Vec<usize> = Vec::new();
    let mut seen = vec![false; tracks.len()];
    for &t in trace_of {
        if !seen[t] {
            seen[t] = true;
            visible.push(t);
        }
    }
    let extent = |tr: &Track| {
        (
            tr.range.0.unwrap_or(0),
            tr.range.1.unwrap_or(n.saturating_sub(1)),
        )
    };
    let group_of =
        |tracks: &[Track], t: Option<usize>| t.filter(|&t| seen[t]).map(|t| tracks[t].group);
    let none_group = groups + 1;
    let mut order: Vec<(usize, (usize, i64, i64))> = visible
        .iter()
        .map(|&t| {
            let tr = &tracks[t];
            let (s, e) = extent(tr);
            let g = max(
                group_of(tracks, tr.source).unwrap_or(none_group),
                group_of(tracks, tr.target).unwrap_or(none_group),
            );
            (t, (g, e as i64 - s as i64, s as i64))
        })
        .collect();
    order.sort_by_key(|&(_, key)| key);

    let mut occupied: Vec<Vec<Vec<(usize, usize)>>> = vec![Vec::new(); groups + 1];
    for (t, _) in order {
        let (s, e) = extent(&tracks[t]);
        let g = tracks[t].group;
        let align_right = group_of(tracks, tracks[t].source).is_some_and(|sg| sg > g)
            || group_of(tracks, tracks[t].target).is_some_and(|tg| tg > g);
        let target_col = tracks[t].target.and_then(|tt| tracks[tt].column);
        let cols = &mut occupied[g];
        let count = cols.len();
        let found = (0..count)
            .map(|i| if align_right { count - i - 1 } else { i })
            .find(|&ci| {
                !cols[ci].iter().any(|&(os, oe)| s <= oe && e >= os) && target_col != Some(ci)
            })
            .unwrap_or(count);
        if found == count {
            cols.push(Vec::new());
        }
        cols[found].push((s, e));
        tracks[t].column = Some(found);
    }
    let mut offset = vec![0; groups + 1];
    for g in 1..=groups {
        offset[g] = offset[g - 1] + occupied[g - 1].len();
    }
    for &t in &visible {
        let g = tracks[t].group;
        tracks[t].column = tracks[t].column.map(|c| c + offset[g]);
    }
}

fn anonymous_color(id: Id) -> usize {
    1 + id.as_bytes()[0] as usize % (PALETTE_SIZE - 1)
}

struct Grid {
    width: usize,
    cells: Vec<Cell>,
}

impl Grid {
    fn get(&self, x: usize, y: usize) -> u8 {
        self.cells[y * self.width + x].code
    }
    fn set(&mut self, x: usize, y: usize, code: u8, color: usize, pers: u8) {
        self.cells[y * self.width + x] = Cell { code, color, pers };
    }
    /// Change the glyph (if given) and, when `pers` wins (is lower), the color.
    fn put(&mut self, x: usize, y: usize, code: Option<u8>, color: usize, pers: u8) {
        let cell = &mut self.cells[y * self.width + x];
        if let Some(code) = code {
            cell.code = code;
        }
        if pers < cell.pers {
            cell.color = color;
            cell.pers = pers;
        }
    }
    /// Change only the glyph.
    fn glyph(&mut self, x: usize, y: usize, code: u8) {
        self.cells[y * self.width + x].code = code;
    }
}

fn vline(grid: &mut Grid, (from, to): (usize, usize), column: usize, color: usize, pers: u8) {
    let x = column * 2;
    for y in from + 1..to {
        match grid.get(x, y) {
            DOT | CIRCLE => {}
            HOR | HOR_U | HOR_D => grid.set(x, y, CROSS, color, pers),
            CROSS | VER | VER_L | VER_R => grid.put(x, y, None, color, pers),
            L_D | L_U => grid.put(x, y, Some(VER_L), color, pers),
            R_D | R_U => grid.put(x, y, Some(VER_R), color, pers),
            _ => grid.put(x, y, Some(VER), color, pers),
        }
    }
}

/// Horizontal line on row `y` from column `from` to `to` (either direction).
/// For a merge, an arrow next to `to` points into the merge commit.
fn hline(
    grid: &mut Grid,
    y: usize,
    (from, to): (usize, usize),
    merge: bool,
    color: usize,
    pers: u8,
) {
    if from == to {
        return;
    }
    let (f2, t2) = (from * 2, to * 2);
    if from < to {
        for x in f2 + 1..t2 {
            if merge && x == t2 - 1 {
                grid.set(x, y, ARR_R, color, pers);
            } else {
                hcell(grid, x, y, color, pers);
            }
        }
        match grid.get(f2, y) {
            DOT | CIRCLE | VER_R => {}
            VER => grid.put(f2, y, Some(VER_R), color, pers),
            VER_L => grid.glyph(f2, y, CROSS),
            HOR | L_U => grid.put(f2, y, Some(HOR_U), color, pers),
            _ => grid.put(f2, y, Some(R_D), color, pers),
        }
        match grid.get(t2, y) {
            DOT | CIRCLE => {}
            VER => grid.glyph(t2, y, VER_L),
            VER_L | HOR_U => grid.put(t2, y, None, color, pers),
            HOR | R_U => grid.put(t2, y, Some(HOR_U), color, pers),
            _ => grid.put(t2, y, Some(L_U), color, pers),
        }
    } else {
        for x in t2 + 1..f2 {
            if merge && x == t2 + 1 {
                grid.set(x, y, ARR_L, color, pers);
            } else {
                hcell(grid, x, y, color, pers);
            }
        }
        match grid.get(t2, y) {
            DOT | CIRCLE => {}
            VER => grid.glyph(t2, y, VER_R),
            VER_R => grid.put(t2, y, None, color, pers),
            HOR | L_U => grid.put(t2, y, Some(HOR_U), color, pers),
            _ => grid.put(t2, y, Some(R_U), color, pers),
        }
        match grid.get(f2, y) {
            DOT | CIRCLE => {}
            VER => grid.put(f2, y, Some(VER_L), color, pers),
            VER_R => grid.glyph(f2, y, CROSS),
            VER_L => grid.put(f2, y, None, color, pers),
            HOR | R_D => grid.put(f2, y, Some(HOR_D), color, pers),
            _ => grid.put(f2, y, Some(L_D), color, pers),
        }
    }
}

fn hcell(grid: &mut Grid, x: usize, y: usize, color: usize, pers: u8) {
    match grid.get(x, y) {
        DOT | CIRCLE => {}
        VER => grid.glyph(x, y, CROSS),
        HOR | CROSS | HOR_U | HOR_D => grid.put(x, y, None, color, pers),
        L_U | R_U => grid.put(x, y, Some(HOR_U), color, pers),
        L_D | R_D => grid.put(x, y, Some(HOR_D), color, pers),
        _ => grid.put(x, y, Some(HOR), color, pers),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> Id {
        Id::from_bytes(&[n; 20]).unwrap()
    }

    /// commits: (id, parents) top-down; branches: (name, tip, persistence, group).
    fn render(commits: &[(u8, &[u8])], branches: &[(&str, u8, u8, usize)]) -> Vec<String> {
        let parents: Vec<Vec<Id>> = commits
            .iter()
            .map(|(_, ps)| ps.iter().map(|&p| id(p)).collect())
            .collect();
        let nodes: Vec<Node> = commits
            .iter()
            .zip(&parents)
            .map(|((c, _), ps)| Node {
                id: id(*c),
                parents: ps,
            })
            .collect();
        let specs = branches
            .iter()
            .map(|&(name, tip, persistence, group)| BranchSpec {
                name: Some(name.to_string()),
                start: id(tip),
                merge: None,
                persistence,
                group,
                color: 0,
                fork_color: 0,
            })
            .collect();
        layout(&nodes, specs, 3, true)
            .rows
            .iter()
            .map(|r| {
                r.cells
                    .iter()
                    .map(Cell::ch)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn linear() {
        let g = render(&[(3, &[2]), (2, &[1]), (1, &[])], &[("main", 3, 0, 0)]);
        assert_eq!(g, vec!["●", "●", "●"]);
    }

    #[test]
    fn merge_has_arrow_and_fork_gets_connector_row() {
        // 4 = merge(3, 2); 3 -> 1; 2 -> 1 (2 is on a merged, deleted branch).
        let g = render(
            &[(4, &[3, 2]), (3, &[1]), (2, &[1]), (1, &[])],
            &[("main", 4, 0, 0)],
        );
        assert_eq!(g, vec!["○<╮", "● │", "│ ●", "├─╯", "●"]);
    }

    #[test]
    fn live_branch_forks_above_parent() {
        // feature tip 3 -> 1; main 2 -> 1.
        let g = render(
            &[(3, &[1]), (2, &[1]), (1, &[])],
            &[("main", 2, 0, 0), ("feature", 3, 2, 3)],
        );
        assert_eq!(g, vec!["  ●", "● │", "├─╯", "●"]);
    }

    #[test]
    fn octopus_connects_every_parent() {
        let g = render(
            &[(4, &[3, 2, 1]), (3, &[]), (2, &[]), (1, &[])],
            &[("main", 4, 0, 0)],
        );
        assert_eq!(g, vec!["○<┬─╮", "● │ │", "  ● │", "    ●"]);
    }

    #[test]
    fn branch_merged_into_same_name_is_a_fork() {
        // A pull request from a fork's "main" merged into main: the merged
        // branch is renamed fork/main and must not take main's column.
        let parents: Vec<Vec<Id>> = vec![vec![id(2), id(3)], vec![id(1)], vec![id(1)], vec![]];
        let nodes: Vec<Node> = [4u8, 2, 3, 1]
            .iter()
            .zip(&parents)
            .map(|(&c, ps)| Node {
                id: id(c),
                parents: ps,
            })
            .collect();
        let spec = |start: u8, merge: Option<usize>| BranchSpec {
            name: Some("main".into()),
            start: id(start),
            merge,
            persistence: 0,
            group: 0,
            color: 0,
            fork_color: 7,
        };
        let g = layout(&nodes, vec![spec(4, None), spec(3, Some(0))], 3, true);
        let rows: Vec<String> = g
            .rows
            .iter()
            .map(|r| r.cells.iter().map(Cell::ch).collect::<String>())
            .collect();
        assert_eq!(rows, vec!["○<╮", "● │", "│ ●", "├─╯", "●  "]);
        assert_eq!(g.commit_branch[2].as_deref(), Some("fork/main"));
        assert_eq!(g.commit_color[2], 7);
    }
}
