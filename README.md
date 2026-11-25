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
- **Search** across the whole message (subject and body), author, email, hash and branch/tag names in a
  small input box: regex with smart case, matches highlighted and the first
  one selected while you type, `n`/`N` to step through them and `esc` to
  hide the highlights. See [Shared search](#shared-search).
- **Stashes** appear as `stash@{n}` commits hanging off the commit they were
  made on, with their changed files and diff (hide them with `--no-stash`).
- **Branch, tag & stash picker** with type-to-filter.
- **Walk the history**: first parent, merged parent (repeat `m` to cycle the
  parents of an octopus merge), child, HEAD.
- **Safe checkout** of the branch at a commit (or a detached HEAD), with
  confirmation; refuses to overwrite local changes or to check out a branch
  that another worktree has checked out, and checks everything before any
  file is touched.
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
| `--no-mouse` | Don't capture the mouse: drag uses the terminal's own selection, and the wheel scrolls only where the terminal supports alternate scroll mode |
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
The commit list, the file list and the diff show a scrollbar on their right
border when they don't fit, as in track and monitor.
The focused pane has a highlighted border; `←`/`→` move focus through
**graph → commit → files → diff**, as in git-igitt.

## Keys

Press `?` in the app for this list (`j`/`k` or the wheel scroll it when it
doesn't fit; any other key closes it). The keys and their wording are the
same in track, monitor and git-graph (see the shared key guideline below).

What `j`/`k`, `g`/`G` and paging do depends on the focused pane (`←`/`→`
move focus): they move the selection in the commit and file lists (a shown
diff follows the file), and scroll the commit details and the diff.

**Navigation**

| Key | Action |
|---|---|
| `j`/`k`, `↓`/`↑` | Down / up |
| `ctrl-d`/`ctrl-u` | Half page down / up |
| `PgDn`/`PgUp`, `ctrl-f`/`ctrl-b` | Page down / up |
| `g`/`G` | Top / bottom |
| `h`/`l` | Left / right 4 columns (diff) |
| `H`/`L` | Left / right half a page (diff) |
| `home`/`end` | Line start / end (diff) |
| `←`/`→` | Previous / next pane: commits, commit, files, diff |
| `enter` | Open: the files, then a file's diff |

**Search**

| Key | Action |
|---|---|
| `/` | Search (regex, smart case; whole message, author, email, hash, refs) |
| `n`/`N` | Next / previous match |
| `esc` | Hide search highlights, then back to the commits |

**Commits**

| Key | Action |
|---|---|
| `H` | Jump to HEAD |
| `p`/`c` | First parent / child |
| `m` | Merged parent (repeat to cycle through every parent of an octopus merge) |
| `b` | Branch, tag & stash picker (type to filter, `enter` to jump) |
| `D` | Diff of the whole commit |
| `o` | Checkout the branch at the commit, or detach HEAD there (asks first; not for stashes) |
| `y` | Copy the commit hash (OSC 52) |

**Diff** (shown in place of the commits)

| Key | Action |
|---|---|
| `tab` / `shift-tab` | Next / previous file (wrapping around) |
| `z` | Maximize / restore |
| `esc` | Back to the commits |

**Input box** (the `/` box and the branch picker)

| Key | Action |
|---|---|
| `←`/`→`, `home`/`end`, `ctrl-a`/`ctrl-e` | Move the cursor |
| `ctrl-u` / `ctrl-w` | Delete to start / previous word |
| `↑`/`↓` | History (branch picker: select) |
| `enter` | Submit |
| `esc`, `ctrl-c` | Cancel (`backspace` on an empty box too) |

**Mouse**

| Key | Action |
|---|---|
| wheel | Scroll (`ctrl`: faster) |
| click | Select a commit, file or pane |
| drag | Select text; copied on release (OSC 52) |
| shift + drag | The terminal's own selection |

**General**

| Key | Action |
|---|---|
| `r` | Refresh (reload the repository) |
| `?` | Help |
| `q`, `ctrl-c` | Quit |

### Shared key guideline

`track`, `monitor` and `git-graph` follow one key guideline, so the same
keys do the same thing in all three:

| Key | Meaning |
|---|---|
| `j`/`k`, `↓`/`↑` | Down / up |
| `ctrl-d`/`ctrl-u` | Half page down / up |
| `PgDn`/`PgUp`, `ctrl-f`/`ctrl-b` | Page down / up |
| `g`/`G` | Top / bottom |
| `h`/`l` | Left / right 4 columns |
| `H`/`L` | Left / right half a page |
| `home`/`end` | Line start / end (not top / bottom) |
| `/`, `n`/`N` | Search (regex, smart case), next / previous match |
| `esc` | Hide search highlights, then go back |
| `w` | Toggle wrap (track, monitor) |
| `r` | Refresh: redraw the screen, and run the command again (monitor) or reload the repository (git-graph) |
| `?` | Help, in the same window everywhere (`j`/`k` or the wheel scroll it; any other key closes it) |
| `q`, `ctrl-c` | Quit (in an input box, `ctrl-c` cancels it like `esc`) |
| mouse wheel | Scroll (`ctrl` + wheel: faster) |
| mouse drag | Select text and copy it with OSC 52 on release (up to 512 KB) |
| shift + drag | The terminal's own selection |

Other keys are specific to one tool, and no key just repeats another one.
In `git-graph`'s commit list, which doesn't scroll sideways, `H` jumps to
HEAD.

### Shared search

`/` opens the same small input box near the top of the screen in all three
(in the style of snacks.nvim's input):

```
          ╭───────────────────── Search ─────────────────────╮
          │  error█                               12 matches │
          ╰──── enter search · ↑/↓ history · esc cancel ─────╯
```

- Patterns are regular expressions with smart case (case-insensitive unless
  the pattern has an uppercase letter; the letter of an escape such as `\W`
  or `\S` doesn't count); an invalid regex is matched literally.
- While you type, matches are highlighted, the view moves to the first match
  from where you started, and the box shows how many there are.
- `enter` keeps the search; `esc` cancels it and goes back to where you were;
  an empty search clears it.
- Editing: `←`/`→`, `home`/`end`, `ctrl-a`/`ctrl-e` move the cursor,
  `ctrl-u`/`ctrl-w` delete to the start / the previous word, `↑`/`↓` browse
  earlier searches, and `backspace` on an empty box closes it.
- `n`/`N` move to the next / previous match. The current match's line is
  highlighted (in `git-graph` it is the selected commit), and the view only
  scrolls when the match is off screen, keeping 3 lines of context. Scroll
  the current match out of view and `n`/`N` start from the screen instead.
- The main panel's title shows the search and the current match, e.g.
  `/error  3/12`.
- Matches are highlighted in black on yellow. `esc` hides the highlights but
  keeps the search (the title then says `hidden`), like vim's `:noh`; `n`/`N`
  show them again and jump to the next / previous match.

### Shared screen layout

All three draw the same frame:

```
 tool  what it shows  state · flags that are on              tool-specific info
╭ Panel  details ─────────────────────────────────────── /search  current/total ╮
│ content                                                                       █
│                                                                               ║
╰──────────────────────────────────────────────────────────────────────────────╯
 j/k down/up  g/G top/bottom  / search  n/N next/prev  tool keys…  esc back  ? help  q quit
```

- The header starts with the tool's name on a cyan badge, then what it shows
  (cut short with `…` when space runs out), its state, and the flags that
  are on (e.g. `wrap`, `diff · bell · wrap`). The tool-specific info on the
  right appears when everything fits.
- Panels have rounded borders; the focused one is cyan.
- A panel whose content doesn't fit has a scrollbar on its right border, in
  the border's color (cyan when the panel is focused). Nothing is drawn over
  the bottom border.
- The footer lists keys in the same order everywhere: movement, search, the
  tool's most used keys (`?` lists them all), `esc` back where it applies,
  then `? help` and `q quit`.
  Keys are cyan and their descriptions gray; on a narrow terminal the tool's
  own keys are dropped first, then `? help`, and a hint is never cut in
  half. A status message (yellow) replaces the hints until the next key.
- The mouse is captured, so the wheel scrolls and a drag selects text.
  `--no-mouse` leaves it to the terminal and asks for alternate scroll mode,
  so the wheel still scrolls where the terminal supports it.
- However the tool ends (`q`, `ctrl-c`, a termination signal from outside
  such as `kill` or closing the terminal or tmux pane, or a crash), the
  terminal is restored: normal screen, cursor, keyboard and mouse.
- Double-width characters (Chinese, Japanese, Korean) stay whole, including
  under the edge of a popup and in terminals that don't blank the other half
  of a half-overwritten one, such as Neovim's terminal (toggleterm.nvim).

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
cargo fmt
cargo clippy --all-targets
cargo test     # graph layout, merge-message parsing, selection, search and editor tests
```

| File | Contents |
|---|---|
| `src/graph.rs` | Branch tracing, column assignment, connector rows and drawing (pure, unit-tested) |
| `src/repo.rs` | Loading history, refs and branch specs with git2; commit details, diffs, checkout |
| `src/app.rs` | Application state and key/mouse handling |
| `src/ui.rs` | Rendering with ratatui |
| `src/selection.rs` | Mouse text selection and copying |
| `src/input.rs` | Search box line editor and history (shared) |
| `src/matcher.rs` | Smart-case regex search patterns (shared) |
| `src/main.rs` | Command-line options, the event loop, `--print` |
| `src/terminal.rs` | Entering and restoring the terminal, panics, OSC 52 copying (shared) |
| `src/signals.rs` | Quitting cleanly on termination signals (shared) |

`git-graph --print` is handy for checking layout changes: it prints the same
graph the TUI shows, including connector rows.

### Code style

track, monitor and git-graph share one code style, so a change can be
carried between them:

- `rustfmt.toml` is the same in all three; run `cargo fmt` before committing.
- `cargo clippy --all-targets` must report nothing, and `cargo test` must pass.
- `src/input.rs` (line editor and history) and `src/matcher.rs` (smart-case
  regex search) are identical copies; change them in all three together.
- The same `[profile.release]` (LTO, one codegen unit, stripped) and
  `rust-version = "1.88"`; dependencies are listed alphabetically, with the
  same versions where they overlap.
