//! Lane-based commit graph layout.
//!
//! Input is a list of commits in topological order (children before parents).
//! Each commit occupies one text row. Every row is rendered as a list of
//! cells, two cells per lane: the lane cell (`x = 2 * lane`) and a gap cell
//! used for horizontal connectors.
//!
//! Improvements over a naive layout:
//! * The first-parent chain of the primary branch (main/master) is pinned to
//!   lane 0, so the trunk never jumps between columns.
//! * Connectors are composed from direction bits, so overlapping merges and
//!   forks render as proper junctions (`┴ ┬ ┼ ┤ ├`) instead of clobbering
//!   each other.
//! * A lane freed on a row is never reused on that same row, which avoids
//!   ambiguous glyphs where one line ends and another starts.

use std::collections::{HashMap, HashSet};

pub type Id = git2::Oid;

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

/// Number of colors the layout cycles through. Color 0 is reserved for the
/// primary branch.
pub const PALETTE_SIZE: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Empty,
    Commit,
    MergeCommit,
    Line(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub glyph: Glyph,
    pub color: usize,
}

impl Cell {
    const EMPTY: Cell = Cell {
        glyph: Glyph::Empty,
        color: 0,
    };

    pub fn ch(&self) -> char {
        match self.glyph {
            Glyph::Empty => ' ',
            Glyph::Commit => '●',
            Glyph::MergeCommit => '○',
            Glyph::Line(bits) => line_char(bits),
        }
    }
}

fn line_char(bits: u8) -> char {
    let (u, d, l, r) = (
        bits & UP != 0,
        bits & DOWN != 0,
        bits & LEFT != 0,
        bits & RIGHT != 0,
    );
    match (u, d, l, r) {
        (true, true, true, true) => '┼',
        (true, true, true, false) => '┤',
        (true, true, false, true) => '├',
        (true, false, true, true) => '┴',
        (false, true, true, true) => '┬',
        (true, true, false, false) => '│',
        (false, false, true, true) => '─',
        (false, true, true, false) => '╮',
        (false, true, false, true) => '╭',
        (true, false, true, false) => '╯',
        (true, false, false, true) => '╰',
        (true, false, false, false) | (false, true, false, false) => '│',
        (false, false, true, false) | (false, false, false, true) => '─',
        (false, false, false, false) => ' ',
    }
}

#[derive(Clone, Debug)]
pub struct Row {
    /// Color of the commit's lane.
    pub color: usize,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Copy, Debug)]
struct Lane {
    target: Id,
    color: usize,
}

pub struct Node<'a> {
    pub id: Id,
    pub parents: &'a [Id],
    /// Fixed color for this commit (e.g. derived from its branch name).
    /// `None` falls back to the lane's color or a rotating palette color.
    pub color: Option<usize>,
}

#[derive(Clone, Copy)]
enum Link {
    /// Lane comes from above and ends at this commit.
    MergeIn,
    /// New lane starts here and continues down.
    ForkOut,
    /// Existing lane continues; this commit also connects to it.
    Join,
}

/// `compact`: when a lane's left neighbour slot is free, step the lane left
/// (drawn as `╭─╯`) so the graph does not keep growing wide gaps.
pub fn layout<'a>(
    nodes: impl IntoIterator<Item = Node<'a>>,
    primary: &HashSet<Id>,
    compact: bool,
) -> Vec<Row> {
    let reserve0 = !primary.is_empty();
    let mut lanes: Vec<Option<Lane>> = Vec::new();
    let mut next_color = 1usize;
    let mut new_color = || {
        let c = next_color;
        next_color = if next_color + 1 >= PALETTE_SIZE {
            1
        } else {
            next_color + 1
        };
        c
    };
    let nodes: Vec<Node<'a>> = nodes.into_iter().collect();
    let fixed: HashMap<Id, usize> = nodes
        .iter()
        .filter_map(|n| Some((n.id, n.color?)))
        .collect();
    let mut rows = Vec::new();

    for node in nodes {
        let is_primary = primary.contains(&node.id);
        let matches: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, l)| l.is_some_and(|l| l.target == node.id))
            .map(|(i, _)| i)
            .collect();

        let above: Vec<bool> = lanes.iter().map(Option::is_some).collect();

        // Choose the commit's lane.
        let lane0_ok = reserve0
            && is_primary
            && lanes
                .first()
                .is_none_or(|l| l.is_none_or(|l| l.target == node.id));
        let col = if lane0_ok {
            0
        } else if let Some(&m) = matches.first() {
            m
        } else {
            alloc(&lanes, reserve0, &[])
        };
        if col >= lanes.len() {
            lanes.resize(col + 1, None);
        }
        let color = match (node.color, lanes[col]) {
            _ if is_primary && reserve0 => 0,
            (Some(c), _) => c,
            (None, Some(l)) => l.color,
            (None, None) => new_color(),
        };

        let mut links: Vec<(usize, Link)> = Vec::new();
        let mut merge_colors: Vec<(usize, usize)> = Vec::new();
        let mut freed = Vec::new();
        for &m in &matches {
            if m != col {
                links.push((m, Link::MergeIn));
                merge_colors.push((m, lanes[m].map_or(color, |l| l.color)));
                lanes[m] = None;
                freed.push(m);
            }
        }

        // First parent continues in the commit's lane.
        match node.parents.first() {
            Some(&p) => lanes[col] = Some(Lane { target: p, color }),
            None => lanes[col] = None,
        }

        // Further parents: join an existing lane or fork a new one.
        for &p in node.parents.iter().skip(1) {
            let existing = lanes
                .iter()
                .enumerate()
                .find(|(i, l)| *i != col && l.is_some_and(|l| l.target == p))
                .map(|(i, _)| i);
            if let Some(j) = existing {
                let link = if above.get(j).copied().unwrap_or(false) {
                    Link::Join
                } else {
                    Link::ForkOut
                };
                links.push((j, link));
                continue;
            }
            let primary_slot = reserve0
                && primary.contains(&p)
                && lanes.first().is_some_and(|l| l.is_none())
                && !freed.contains(&0)
                && col != 0;
            let k = if primary_slot {
                0
            } else {
                alloc(&lanes, reserve0, &freed)
            };
            if k >= lanes.len() {
                lanes.resize(k + 1, None);
            }
            // A forked lane leads to the merged parent, so it takes that branch's color.
            let c = match fixed.get(&p) {
                _ if primary_slot => 0,
                Some(&c) => c,
                None => new_color(),
            };
            lanes[k] = Some(Lane {
                target: p,
                color: c,
            });
            links.push((k, Link::ForkOut));
        }

        // Render the row.
        let width = above.len().max(lanes.len());
        let mut bits = vec![0u8; width * 2];
        let mut colors = vec![0usize; width * 2];
        for i in 0..width {
            let passes = i != col
                && above.get(i).copied().unwrap_or(false)
                && lanes.get(i).is_some_and(|l| l.is_some())
                && !links.iter().any(|(j, _)| *j == i);
            if passes {
                bits[2 * i] |= UP | DOWN;
                colors[2 * i] = lanes[i].unwrap().color;
            }
        }
        // Draw far links first so nearer links' colors win at shared cells.
        let mut ordered = links.clone();
        ordered.sort_by_key(|(j, _)| std::cmp::Reverse(j.abs_diff(col)));
        for (j, link) in ordered {
            let link_color = match link {
                Link::MergeIn => merge_colors
                    .iter()
                    .find(|(m, _)| *m == j)
                    .map_or(color, |&(_, c)| c),
                _ => lanes[j].map(|l| l.color).unwrap_or(color),
            };
            let (from, to) = (2 * col, 2 * j);
            let (lo, hi) = if from < to { (from, to) } else { (to, from) };
            for x in lo + 1..hi {
                let vertical = bits[x] & (UP | DOWN) != 0;
                bits[x] |= LEFT | RIGHT;
                if !vertical {
                    colors[x] = link_color;
                }
            }
            let side = if to > from { LEFT } else { RIGHT };
            let vert = match link {
                Link::MergeIn => UP,
                Link::ForkOut => DOWN,
                Link::Join => UP | DOWN,
            };
            bits[to] |= side | vert;
            colors[to] = link_color;
        }

        if compact {
            for i in 1..lanes.len() {
                let Some(l) = lanes[i] else { continue };
                let t = i - 1;
                let target_free =
                    lanes[t].is_none() && t != col && bits[2 * t] == 0 && bits[2 * t + 1] == 0;
                let allowed = !(reserve0 && t == 0 && !primary.contains(&l.target));
                let movable = i == col || bits[2 * i] == UP | DOWN;
                if !(target_free && allowed && movable) {
                    continue;
                }
                if i != col {
                    bits[2 * i] = UP | LEFT;
                }
                bits[2 * t + 1] = LEFT | RIGHT;
                colors[2 * t + 1] = l.color;
                bits[2 * t] = DOWN | RIGHT;
                colors[2 * t] = l.color;
                lanes[t] = Some(l);
                lanes[i] = None;
            }
        }

        let mut cells: Vec<Cell> = bits
            .iter()
            .zip(&colors)
            .map(|(&b, &c)| {
                if b == 0 {
                    Cell::EMPTY
                } else {
                    Cell {
                        glyph: Glyph::Line(b),
                        color: c,
                    }
                }
            })
            .collect();
        cells[2 * col] = Cell {
            glyph: if node.parents.len() > 1 {
                Glyph::MergeCommit
            } else {
                Glyph::Commit
            },
            color,
        };
        // Trim trailing empties.
        while cells.last().is_some_and(|c| c.glyph == Glyph::Empty) {
            cells.pop();
        }
        rows.push(Row { color, cells });

        while lanes.last().is_some_and(|l| l.is_none()) {
            lanes.pop();
        }
    }
    rows
}

fn alloc(lanes: &[Option<Lane>], reserve0: bool, exclude: &[usize]) -> usize {
    let start = usize::from(reserve0);
    (start..lanes.len())
        .find(|&i| lanes[i].is_none() && !exclude.contains(&i))
        .unwrap_or(lanes.len().max(start))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> Id {
        Id::from_bytes(&[n; 20]).unwrap()
    }

    fn render(commits: &[(u8, Vec<u8>)], primary: &[u8]) -> Vec<String> {
        render_with(commits, primary, false)
    }

    fn render_with(commits: &[(u8, Vec<u8>)], primary: &[u8], compact: bool) -> Vec<String> {
        let parents: Vec<Vec<Id>> = commits
            .iter()
            .map(|(_, ps)| ps.iter().map(|&p| id(p)).collect())
            .collect();
        let nodes = commits.iter().zip(&parents).map(|((c, _), ps)| Node {
            id: id(*c),
            parents: ps,
            color: None,
        });
        let primary: HashSet<Id> = primary.iter().map(|&p| id(p)).collect();
        layout(nodes, &primary, compact)
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
        let g = render(&[(3, vec![2]), (2, vec![1]), (1, vec![])], &[3, 2, 1]);
        assert_eq!(g, vec!["●", "●", "●"]);
    }

    #[test]
    fn merge_and_fork() {
        // 4 = merge(3, 2); 3 -> 1; 2 -> 1
        let g = render(
            &[(4, vec![3, 2]), (3, vec![1]), (2, vec![1]), (1, vec![])],
            &[4, 3, 1],
        );
        assert_eq!(g, vec!["○─╮", "● │", "│ ●", "●─╯"]);
    }

    #[test]
    fn primary_stays_in_lane_zero_even_when_feature_is_newer() {
        // Feature tip 5 (newer) -> 2; main 3 -> 2 -> 1.
        let g = render(
            &[(5, vec![2]), (3, vec![2]), (2, vec![1]), (1, vec![])],
            &[3, 2, 1],
        );
        assert_eq!(g, vec!["  ●", "● │", "●─╯", "●"]);
    }

    #[test]
    fn two_lanes_merge_into_one_commit_make_junction() {
        // 1 is parent of 4, 3, 2 (three branches off 1).
        let g = render(
            &[(4, vec![1]), (3, vec![1]), (2, vec![1]), (1, vec![])],
            &[4, 1],
        );
        assert_eq!(g, vec!["●", "│ ●", "│ │ ●", "●─┴─╯"]);
    }

    #[test]
    fn octopus_fork_uses_tee() {
        let g = render(
            &[(4, vec![3, 2, 1]), (3, vec![]), (2, vec![]), (1, vec![])],
            &[4, 3],
        );
        assert_eq!(g, vec!["○─┬─╮", "● │ │", "  ● │", "    ●"]);
    }

    #[test]
    fn crossing_lane_renders_cross() {
        // main: 5 -> 3 -> 1. feature A: 4 -> 2. 5 merges in 4? Instead: lane 1 passes
        // while lane 0 links to lane 2.
        let g = render(
            &[
                (6, vec![1]),
                (5, vec![4]),
                (4, vec![3, 2]),
                (3, vec![1]),
                (2, vec![1]),
                (1, vec![]),
            ],
            &[5, 4, 3, 1],
        );
        assert_eq!(g[2], "○─┼─╮");
    }

    #[test]
    fn compaction_steps_lane_left() {
        // 5 is a root in lane 1; once it ends, 8's lane (2) steps left, then
        // into the trunk lane right before reaching the trunk commit.
        let g = render_with(
            &[
                (9, vec![5]),
                (8, vec![7]),
                (5, vec![]),
                (7, vec![6]),
                (6, vec![1]),
                (1, vec![]),
            ],
            &[1],
            true,
        );
        assert_eq!(g, vec!["  ●", "  │ ●", "  ● │", "  ╭─●", "╭─●", "●"]);
    }

    #[test]
    fn compaction_never_steals_primary_lane() {
        let g = render_with(&[(8, vec![7]), (7, vec![6]), (6, vec![])], &[1], true);
        assert_eq!(g, vec!["  ●", "  ●", "  ●"]);
    }

    #[test]
    fn fixed_colors_follow_branches() {
        // 4 = merge(3, 2) on main; 2 is a feature commit with fixed color 5.
        let parents: Vec<Vec<Id>> = vec![vec![id(3), id(2)], vec![id(1)], vec![id(1)], vec![]];
        let colors = [Some(0), Some(0), Some(5), Some(0)];
        let nodes = [4u8, 3, 2, 1]
            .iter()
            .zip(&parents)
            .zip(colors)
            .map(|((&c, ps), color)| Node {
                id: id(c),
                parents: ps,
                color,
            });
        let primary: HashSet<Id> = [4, 3, 1].iter().map(|&p| id(p)).collect();
        let rows = layout(nodes, &primary, false);
        // The fork connector (─╮) toward the feature commit uses its color...
        assert_eq!(rows[0].cells[1].color, 5);
        assert_eq!(rows[0].cells[2].color, 5);
        // ...as do the feature commit and the line merging back into main.
        assert_eq!(rows[2].color, 5);
        assert_eq!(rows[3].cells[2].color, 5);
        assert_eq!(rows[3].color, 0);
    }
}
