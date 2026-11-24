# git-graph

An interactive terminal viewer for git history, in the spirit of
[git-igitt](https://github.com/git-bahn/git-igitt): a branch-aware commit
graph that you can browse, search and diff from the keyboard.

> **About this README and this code.** I'm Claude (Opus 5.5), an AI model made
> by Anthropic. I wrote this project — the code, the tests and this README —
> in Claude Code, working step by step from the repository owner's requests
> and feedback. The owner decided what to build and how it should look and
> behave; I implemented it, checked it, and committed each change.

```
    ●            5ce676b (develop) develop 25
    │ ●          08aca3d (feature/live) live 24
    ├─╯
    ○<╮          2715ba5 Merge branch 'feature/c' into develop
    │ ●          06786c3 c 22
    ○<┤          6a859bb Merge branch 'feature/c' into develop
    │ ●          f1ee81b c 20
    ● │          a92a19a develop 19
    │ ●          45d3501 c 18
    ├─╯
  ╭>○            b3f7e12 Merge branch 'hotfix/x' into develop
○<┤ │            762abd9 (main) Merge branch 'hotfix/x'
│ ● │            0de545b hotfix 15
├─╯ ○            88dad9d Merge branch 'release/1.0' into develop
│ ╭>┤
○<┤ │            0e4f277 Merge branch 'release/1.0'
│ ● │            5905747 release 12
│ ╰─┤
│   ○<──╮        540f61b Merge branch 'feature/b' into develop
│   │   ●        a636ebf b 10
│   ├──>○        55e3c6e Merge branch 'develop' into feature/b
```

*(Output of `git-graph --print` on a small git-flow test repository.)*

## Features

- **git-igitt-style graph.** Each branch keeps its own column, ordered
  git-flow style (main · hotfix/release · develop · everything else) but
  packed tighter than git-igitt, without empty columns. Merges
  are drawn on the merge commit's row with an arrow (`○<──╮`), forks on a
  connector row just above the parent (`├───╯`). See
  [How the graph is drawn](#how-the-graph-is-drawn).
- **Stable branch colors.** A branch's color comes from its name, so it stays
  the same across merges, rebases and restarts; `origin/x` matches `x`, and
  the trunk is always blue. Deleted branches are named from their merge
  messages.
- **Commit details and changed files** beside the graph, with `+/-` counts
  and rename detection.
- **Diffs in place of the graph** with the commit details and file list still
  visible; maximize with `z`, scroll sideways with `h`/`l`, `tab` through
  files.
- **Search popup** with live results across message, author, email, hash and
  branch/tag names; `n`/`N` to step through matches, `s` to toggle
  highlighting.
- **Stashes** appear as `stash@{n}` commits hanging off the commit they were
  made on, with their changed files and diff (hide them with `--no-stash`).
- **Branch, tag & stash picker** with type-to-filter.
- **Walk the history**: first parent, merged parent (repeat `m` to cycle the
  parents of an octopus merge), child, HEAD.
- **Safe checkout** of the branch at a commit (or a detached HEAD), with
  confirmation; refuses to overwrite local changes.
- **Copy a commit hash** to the clipboard via the OSC 52 terminal escape
  (works over SSH in most modern terminals).
- **Mouse support**: scroll, click commits, files and panes, and drag to
  select text in any pane and copy it (see [Copying text](#copying-text)).

## Install

Requires Rust 1.88 or newer and a C compiler (for libgit2, which the `git2`
crate builds if your system doesn't provide it).

```sh
cargo install --path .
```

This installs a `git-graph` binary into `~/.cargo/bin`. Note that another,
unrelated tool on crates.io is also called `git-graph`; if you have it
installed, the two binaries will clash.

### Build an executable only

```sh
cargo build --release
```

The binary is written to `target/release/git-graph` (about 3 MB). Run it
directly, or copy it anywhere on your `PATH`:

```sh
./target/release/git-graph ~/some/repo
cp target/release/git-graph ~/.local/bin/
```

libgit2 is compiled into the binary; at runtime it only needs zlib and the C
library, which practically every Linux system has. A binary built on one
machine runs on others with the same architecture and the same or a newer
glibc.

To build and run in one step from the checkout:

```sh
cargo run --release -- [PATH]
```

## Usage

```
git-graph [OPTIONS] [PATH]
```

`PATH` is any directory inside a repository (default: the current directory).

| Option | Description |
|---|---|
| `--head-only` | Only show history reachable from HEAD (default: all branches and tags) |
| `-n, --limit <N>` | Maximum number of commits to load (default: 20000) |
| `-m, --main <BRANCH>` | Trunk branch: leftmost column and color 0 (default: first of `main`, `master`, `trunk`, `develop`, `origin/main`, `origin/master`) |
| `--no-compact` | Give every branch connector its own row instead of sharing free commit rows (like git-igitt's `--sparse`) |
| `--no-stash` | Don't show stash entries |
| `--no-mouse` | Disable mouse support, so plain drag uses the terminal's own text selection |
| `--print` | Print the graph to stdout instead of starting the TUI |

## Layout

```
╭ Commits ──────────────────────────╮╭ Commit ──────────────╮
│ graph · hash · refs · summary     ││ hash, author, date,   │
│                                   ││ parents, branch, refs,│
│ (replaced by the diff when one    ││ full message          │
│  is open)                         │╰───────────────────────╯
│                                   │╭ Files ───────────────╮
│                                   ││ M src/app.rs +1 -1    │
╰───────────────────────────────────╯╰───────────────────────╯
 key hints / status
```

On terminals narrower than 120 columns the side panes move below the graph.
The focused pane has a highlighted border; `←`/`→` move focus through
**graph → commit → files → diff**, as in git-igitt.

## Keys

Press `?` in the app for this list.

What `j`/`k`, `g`/`G` and paging do depends on the focused pane (`←`/`→`
move focus).

**Commits (graph)**

| Key | Action |
|---|---|
| `j`/`k`, `↓`/`↑` | Move the selection |
| `ctrl-d`/`ctrl-u`, `PgDn`/`PgUp` | Half / full page |
| `g` / `G` | First / last commit |
| `H` | Jump to HEAD |
| `p` / `c` | First parent / child |
| `m` | Merged parent; press again to cycle through every parent of an octopus merge |
| `b` | Branch, tag & stash picker (type to filter, `enter` to jump) |
| `←` / `→` | Previous / next pane: graph, commit, files, diff |

**Search**

| Key | Action |
|---|---|
| `/` | Search popup (starts from the last query; `ctrl-u` clears) |
| `n` / `N` | Next / previous match |
| `s` | Toggle search highlighting |

**Commit details**

| Key | Action |
|---|---|
| `j`/`k`, `g`/`G` | Scroll / top / bottom |

**Files**

| Key | Action |
|---|---|
| `j`/`k`, `g`/`G` | Select a file / first / last; if a diff is shown, it follows |
| `enter` / `l` | Show the selected file's diff (from the graph, `enter` focuses the files) |
| `D` | Diff of the whole commit |

**Diff** (shown in place of the graph)

| Key | Action |
|---|---|
| `j`/`k` | Scroll up / down |
| `ctrl-d`/`ctrl-u`, `PgDn`/`PgUp` | Half / full page |
| `g` / `G` | Top / bottom |
| `h` / `l`, `H` / `L` | Scroll sideways (4 columns / half a page) |
| `0` / `$` | Scroll to line start / end |
| `tab` / `shift-tab` | Next / previous file, wrapping around |
| `z` | Maximize / restore the diff |
| `esc` | Close the diff and return to the graph |

**Actions**

| Key | Action |
|---|---|
| `o` | Check out the branch at this commit, or detach HEAD there (asks first; not for stashes) |
| `y` | Copy the commit hash (OSC 52) |
| mouse drag | Select text in a pane; it's copied (OSC 52) when you release |
| shift + drag | Your terminal's own text selection |
| `r` | Reload the repository |
| `?` | Help (`j`/`k` scroll it if it doesn't fit; any other key closes it) |
| `q` | Quit |

## Copying text

Drag over a pane (commits, commit details, files or diff) to select text;
the selection stays within the pane where it started. Dragging past the top
or bottom edge scrolls the commit details and the diff. When you release the
button, the text is copied to the system clipboard with OSC 52, like `y`
does for a commit hash; in the diff, whole lines are copied even when they
are scrolled sideways. Inside tmux, run `tmux set -s set-clipboard on`.

Shift + drag (Option + drag in iTerm2) still gives your terminal's own
selection, or start `git-graph --no-mouse` to leave the mouse to the
terminal.

## How the graph is drawn

The layout is a port of the model used by git-igitt's renderer (the
[gleisbau](https://crates.io/crates/gleisbau) crate), with columns packed
more tightly:

1. **Commits are attributed to branches** by following first parents from
   each branch tip, and from each merge commit's merged parent (named from the
   merge message: `Merge branch '…'`, GitHub pull requests, GitLab and
   Bitbucket formats, …). Branches claim commits in priority order — main,
   develop, feature, release, hotfix, bugfix, then others — with merged
   branches first within each level. A branch merged into one of the same name
   (e.g. a pull request from a fork's `master`) becomes `fork/<name>`.
2. **Each branch gets one column** for its whole extent. Branches are placed in
   group order — main, hotfix/release, develop, other — shortest first, each
   in the leftmost free column that is right of every earlier-group branch
   overlapping it. So groups stay in order left to right, but a group with no
   branch at some point in history doesn't leave an empty column there. A
   branch's extent runs from the first row where anything is drawn in its
   column (its newest commit, or a merge into it) down to where it forked.
3. **Connections zig-zag** between columns: down the child's column, across on
   a "split" row, down the parent's column. Forks turn on the row just above
   the parent; merges on the merge commit's own row, with an arrow into it.
   Turns that would collide get connector-only rows, which share a free commit
   row when possible (disable with `--no-compact`).
4. **The graph has one width** for the whole history, so the text column
   never shifts while scrolling.

Before column packing was added, the port was checked against gleisbau 0.7.3
directly: on git-igitt's own history, a synthetic git-flow repository and
several other repositories the graphs were identical row for row, in both
compact and sparse mode. The packing changes only which column a branch gets;
rows, merges and forks are drawn the same way.

**Deliberate differences from git-igitt:**

- **Narrower graphs.** git-igitt reserves a block of columns per group for
  the whole history, so e.g. a single old hotfix branch leaves an empty
  column next to main everywhere; and a branch that still exists keeps its
  column from the very top of the graph, even when its newest commit is far
  down. Here empty columns are dropped and a branch's column only starts
  where the branch is first drawn.
- **Octopus merges** (three or more parents) connect every parent. gleisbau
  draws only the first two and drops commits reachable only through the
  others.
- **Colors** are derived from branch names rather than from the branch type,
  so a branch keeps its color everywhere.
- **Column ties** between equally long branches are broken in a fixed order;
  gleisbau's choice varies from run to run.

## Development

```sh
cargo test     # graph layout and merge-message parsing tests
cargo clippy
```

| File | Contents |
|---|---|
| `src/graph.rs` | Branch tracing, column assignment, connector rows and drawing (pure, unit-tested) |
| `src/repo.rs` | Loading history, refs and branch specs with git2; commit details, diffs, checkout |
| `src/app.rs` | Application state and key/mouse handling |
| `src/ui.rs` | Rendering with ratatui |
| `src/selection.rs` | Mouse text selection and copying |
| `src/main.rs` | Command-line options, terminal setup, `--print` |

`git-graph --print` is handy for checking layout changes: it prints the same
graph the TUI shows, including connector rows.
