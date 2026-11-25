use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use git2::{BranchType, Delta, DiffFindOptions, DiffOptions, Oid, Repository, Sort};

use crate::graph::{self, BranchSpec, Node, Row};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Head,
    Local,
    Remote,
    Tag,
    Stash,
}

#[derive(Clone, Debug)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
}

#[derive(Clone, Debug)]
pub struct CommitInfo {
    pub id: Oid,
    pub short: String,
    pub parents: Vec<Oid>,
    pub summary: String,
    /// The rest of the message (searched too).
    pub body: String,
    pub author: String,
    pub email: String,
    pub time: i64,
    pub offset_min: i32,
    /// Branch this commit is attributed to (from branch tips or merge messages).
    pub branch: Option<String>,
    /// Palette index of that branch.
    pub color: usize,
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub status: char,
    /// For display (invalid UTF-8 replaced).
    pub path: String,
    pub old_path: Option<String>,
    /// The paths as stored, to find the file's diff even when they aren't UTF-8.
    pub raw_path: Vec<u8>,
    pub raw_old_path: Option<Vec<u8>>,
    pub added: usize,
    pub removed: usize,
}

#[derive(Clone, Debug)]
pub struct CommitDetail {
    pub id: Oid,
    pub message: String,
    pub committer: String,
    pub commit_time: i64,
    pub commit_offset: i32,
    pub files: Vec<FileChange>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Header,
    Hunk,
    Add,
    Del,
    Context,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

pub struct BranchEntry {
    pub label: RefLabel,
    pub target: Oid,
}

pub struct Snapshot {
    pub workdir: String,
    pub head_name: String,
    pub head: Option<Oid>,
    pub primary: Option<String>,
    pub commits: Vec<CommitInfo>,
    /// Graph rows: one per commit plus connector-only rows.
    pub rows: Vec<Row>,
    /// Row index of each commit.
    pub commit_row: Vec<usize>,
    /// Graph width in cells.
    pub graph_width: usize,
    pub refs: HashMap<Oid, Vec<RefLabel>>,
    pub branches: Vec<BranchEntry>,
    pub index: HashMap<Oid, usize>,
    pub truncated: bool,
    remotes: Vec<String>,
}

impl Snapshot {
    /// Palette index for a ref label, matching the color of its branch in the
    /// graph. `None` for tags and a detached HEAD.
    pub fn label_color(&self, label: &RefLabel) -> Option<usize> {
        match label.kind {
            RefKind::Tag => None,
            RefKind::Stash => Some(name_color("stash", self.primary_norm().as_deref())),
            RefKind::Head if label.name == "HEAD" => None,
            _ => Some(name_color(
                &normalize(&label.name, &self.remotes),
                self.primary_norm().as_deref(),
            )),
        }
    }

    fn primary_norm(&self) -> Option<String> {
        self.primary.as_deref().map(|p| normalize(p, &self.remotes))
    }
}

pub struct LoadOptions {
    pub all: bool,
    pub limit: usize,
    pub primary: Option<String>,
    pub compact: bool,
    /// Show stash entries in the graph.
    pub stash: bool,
}

pub fn open(path: &Path) -> Result<Repository> {
    Repository::discover(path).with_context(|| format!("not a git repository: {}", path.display()))
}

/// Text that may not be UTF-8 (old commits in another encoding): shown with
/// replacement characters rather than dropped.
fn lossy(bytes: Option<&[u8]>) -> String {
    bytes
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default()
}

pub fn load(repo: &Repository, opts: &LoadOptions) -> Result<Snapshot> {
    let mut refs: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
    let mut branches = Vec::new();
    // Branch tips in git's ref order, for tracing the graph.
    let mut tips: Vec<(String, Oid)> = Vec::new();
    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;

    let head = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id());
    let head_name = match repo.head() {
        Ok(h) if h.is_branch() => h.shorthand().unwrap_or("HEAD").to_string(),
        Ok(_) => "HEAD (detached)".to_string(),
        Err(_) => "(no commits)".to_string(),
    };
    if let Some(h) = head {
        walk.push(h)?;
        if repo.head_detached().unwrap_or(false) {
            refs.entry(h).or_default().push(RefLabel {
                name: "HEAD".into(),
                kind: RefKind::Head,
            });
        }
    }

    for b in repo.branches(None)? {
        let (b, bt) = b?;
        let Some(name) = b.name()?.map(str::to_string) else {
            continue;
        };
        let Ok(commit) = b.get().peel_to_commit() else {
            continue;
        };
        if bt == BranchType::Remote && name.ends_with("/HEAD") {
            continue;
        }
        let kind = if bt == BranchType::Local {
            RefKind::Local
        } else {
            RefKind::Remote
        };
        let is_head = bt == BranchType::Local && b.is_head();
        let label = RefLabel {
            name,
            kind: if is_head { RefKind::Head } else { kind },
        };
        refs.entry(commit.id()).or_default().push(label.clone());
        tips.push((label.name.clone(), commit.id()));
        branches.push(BranchEntry {
            label,
            target: commit.id(),
        });
        if opts.all {
            walk.push(commit.id())?;
        }
    }

    repo.tag_foreach(|oid, name| {
        let name = String::from_utf8_lossy(name);
        let name = name.trim_start_matches("refs/tags/").to_string();
        if let Ok(commit) = repo.find_object(oid, None).and_then(|o| o.peel_to_commit()) {
            let label = RefLabel {
                name,
                kind: RefKind::Tag,
            };
            refs.entry(commit.id()).or_default().push(label.clone());
            // Tags are in the picker too (b: branch, tag & stash picker)
            branches.push(BranchEntry {
                label,
                target: commit.id(),
            });
            if opts.all {
                let _ = walk.push(commit.id());
            }
        }
        true
    })?;
    for labels in refs.values_mut() {
        labels.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    }
    branches.sort_by(|a, b| {
        a.label
            .kind
            .cmp(&b.label.kind)
            .then_with(|| a.label.name.cmp(&b.label.name))
    });

    let mut commits = Vec::new();
    let mut truncated = false;
    for oid in walk {
        if commits.len() >= opts.limit {
            truncated = true;
            break;
        }
        let commit = repo.find_commit(oid?)?;
        let author = commit.author();
        commits.push(CommitInfo {
            id: commit.id(),
            short: commit
                .as_object()
                .short_id()?
                .as_str()
                .unwrap_or("")
                .to_string(),
            parents: commit.parent_ids().collect(),
            summary: lossy(commit.summary_bytes()),
            body: lossy(commit.body_bytes()),
            author: lossy(Some(author.name_bytes())),
            email: lossy(Some(author.email_bytes())),
            time: author.when().seconds(),
            offset_min: author.when().offset_minutes(),
            branch: None,
            color: 0,
        });
    }
    let stashes = if opts.stash {
        add_stashes(repo, &mut commits)?
    } else {
        Vec::new()
    };
    for (n, &id) in stashes.iter().enumerate() {
        let label = RefLabel {
            name: format!("stash@{{{n}}}"),
            kind: RefKind::Stash,
        };
        refs.entry(id).or_default().push(label.clone());
        branches.push(BranchEntry { label, target: id });
    }
    let index: HashMap<Oid, usize> = commits.iter().enumerate().map(|(i, c)| (c.id, i)).collect();

    let primary_name = primary_branch(repo, opts.primary.as_deref())?;
    let remotes: Vec<String> = repo
        .remotes()?
        .iter()
        .flatten()
        .flatten()
        .map(str::to_string)
        .collect();
    let primary_norm = primary_name.as_deref().map(|p| normalize(p, &remotes));
    let primary = primary_norm.as_deref();

    // Branches to trace, as git-igitt does: branch tips, plus branches named
    // by merge messages (one per merged parent). Colors hash the branch name,
    // so a branch keeps its color across merges, rebases and restarts.
    let spec = |name: Option<String>, start: Oid, merge: Option<usize>| {
        let norm = name.as_deref().map(|n| normalize(n, &remotes));
        let (persistence, group, color) = match norm.as_deref() {
            Some(n) => (
                persistence(n, primary),
                column_group(n, primary),
                name_color(n, primary),
            ),
            None => (
                graph::PERS_OTHER,
                GROUPS,
                name_color(&start.to_string(), primary),
            ),
        };
        let fork_color = name_color(&format!("fork/{}", norm.as_deref().unwrap_or("")), primary);
        BranchSpec {
            name: norm,
            start,
            merge,
            persistence,
            group,
            color,
            fork_color,
        }
    };
    let mut specs: Vec<BranchSpec> = tips
        .iter()
        .map(|(n, id)| spec(Some(n.clone()), *id, None))
        .collect();
    for (i, c) in commits.iter().enumerate() {
        if c.parents.len() > 1 {
            let (_, names) = parse_merge(&c.summary);
            for (k, &p) in c.parents.iter().enumerate().skip(1) {
                specs.push(spec(names.get(k - 1).cloned(), p, Some(i)));
            }
        }
    }
    // Stashes go last so real branches claim shared commits first. They share
    // one color.
    let stash_color = name_color("stash", primary);
    for (n, &id) in stashes.iter().enumerate() {
        specs.push(BranchSpec {
            name: Some(format!("stash@{{{n}}}")),
            start: id,
            merge: None,
            persistence: graph::PERS_OTHER,
            // A group after all branches: stashes hang off to the right.
            group: GROUPS + 1,
            color: stash_color,
            fork_color: stash_color,
        });
    }
    let nodes: Vec<Node> = commits
        .iter()
        .map(|c| Node {
            id: c.id,
            parents: &c.parents,
        })
        .collect();
    let g = graph::layout(&nodes, specs, GROUPS, opts.compact);
    for (i, c) in commits.iter_mut().enumerate() {
        c.branch = g.commit_branch[i].clone();
        c.color = g.commit_color[i];
    }

    Ok(Snapshot {
        workdir: repo
            .workdir()
            .unwrap_or_else(|| repo.path())
            .display()
            .to_string()
            .trim_end_matches('/')
            .to_string(),
        head_name,
        head,
        primary: primary_name,
        commits,
        rows: g.rows,
        commit_row: g.commit_row,
        graph_width: g.width,
        refs,
        branches,
        index,
        truncated,
        remotes,
    })
}

/// Strip a remote prefix: `origin/feature` -> `feature`.
fn normalize(name: &str, remotes: &[String]) -> String {
    let name = name.strip_prefix("refs/heads/").unwrap_or(name);
    if let Some((first, rest)) = name.split_once('/')
        && remotes.iter().any(|r| r == first)
        && !rest.is_empty()
    {
        return rest.to_string();
    }
    name.to_string()
}

/// Stable palette index for a branch name; the primary branch is always 0.
pub fn name_color(name: &str, primary: Option<&str>) -> usize {
    if primary == Some(name) {
        return 0;
    }
    // FNV-1a: deterministic across runs and Rust versions.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    1 + (h % (graph::PALETTE_SIZE as u64 - 1)) as usize
}

/// Branch names a merge commit's message mentions:
/// (`into` = branch merged into, `merged` = names for parents 2..).
fn parse_merge(summary: &str) -> (Option<String>, Vec<String>) {
    // GitHub: "Merge pull request #12 from owner/branch"
    if let Some(rest) = summary.strip_prefix("Merge pull request #")
        && let Some((_, from)) = rest.split_once(" from ")
    {
        let src = from.split_whitespace().next().unwrap_or("");
        let branch = src.split_once('/').map_or(src, |(_, b)| b);
        return (
            None,
            if branch.is_empty() {
                vec![]
            } else {
                vec![branch.to_string()]
            },
        );
    }
    // Bitbucket: "Merged in feature/x (pull request #3)"
    if let Some(rest) = summary.strip_prefix("Merged in ") {
        let branch = rest.split_whitespace().next().unwrap_or("");
        return (
            None,
            if branch.is_empty() {
                vec![]
            } else {
                vec![branch.to_string()]
            },
        );
    }
    // git / GitLab: "Merge branch 'a'", "Merge branches 'a', 'b' and 'c'",
    // "Merge remote-tracking branch 'origin/a'", optional " into 'b'" / " into b".
    let Some(rest) = summary
        .strip_prefix("Merge branches ")
        .or_else(|| summary.strip_prefix("Merge branch "))
        .or_else(|| summary.strip_prefix("Merge remote-tracking branches "))
        .or_else(|| summary.strip_prefix("Merge remote-tracking branch "))
    else {
        return (None, vec![]);
    };
    let (head, into) = match rest.rsplit_once(" into ") {
        Some((h, i)) => (
            h,
            Some(i.trim().trim_matches('\'').to_string()).filter(|s| !s.is_empty()),
        ),
        None => (rest, None),
    };
    let merged = head
        .split('\'')
        .skip(1)
        .step_by(2)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    (into, merged)
}

/// Insert the stash entries (newest first: stash@{0}, stash@{1}, ...) into
/// `commits`. A stash commit's extra parents (the index and untracked-file
/// snapshots) are left out, so each stash shows as one commit on top of the
/// commit it was made on. Each is placed above that commit, at its place in
/// time order. Returns the stash commit ids.
fn add_stashes(repo: &Repository, commits: &mut Vec<CommitInfo>) -> Result<Vec<Oid>> {
    let Ok(reflog) = repo.reflog("refs/stash") else {
        return Ok(Vec::new());
    };
    let mut ids = Vec::new();
    for entry in reflog.iter() {
        let id = entry.id_new();
        let Ok(commit) = repo.find_commit(id) else {
            continue;
        };
        ids.push(id);
        if commits.iter().any(|c| c.id == id) {
            continue;
        }
        let base = commit.parent_id(0).ok();
        let author = commit.author();
        let time = author.when().seconds();
        let base_pos = base.and_then(|b| commits.iter().position(|c| c.id == b));
        let by_time = commits
            .iter()
            .position(|c| c.time <= time)
            .unwrap_or(commits.len());
        let at = base_pos.map_or(by_time, |b| by_time.min(b));
        commits.insert(
            at,
            CommitInfo {
                id,
                short: commit
                    .as_object()
                    .short_id()?
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                parents: base.into_iter().collect(),
                summary: lossy(commit.summary_bytes()),
                body: lossy(commit.body_bytes()),
                author: lossy(Some(author.name_bytes())),
                email: lossy(Some(author.email_bytes())),
                time,
                offset_min: author.when().offset_minutes(),
                branch: None,
                color: 0,
            },
        );
    }
    Ok(ids)
}

/// Number of named column groups (main | hotfix, release | develop); other
/// branches use the next group. Matches git-igitt's default git-flow model.
const GROUPS: usize = 3;

fn is_main(name: &str, primary: Option<&str>) -> bool {
    primary == Some(name) || matches!(name, "master" | "main" | "trunk")
}

/// Claim priority: which branch owns commits shared with another.
fn persistence(name: &str, primary: Option<&str>) -> u8 {
    if is_main(name, primary) {
        0
    } else if matches!(name, "develop" | "dev") {
        1
    } else if name.starts_with("feature") {
        2
    } else if name.starts_with("release") {
        3
    } else if name.starts_with("hotfix") {
        4
    } else if name.starts_with("bugfix") {
        5
    } else {
        graph::PERS_OTHER
    }
}

/// Column group, left to right.
fn column_group(name: &str, primary: Option<&str>) -> usize {
    if is_main(name, primary) {
        0
    } else if name.starts_with("hotfix") || name.starts_with("release") {
        1
    } else if matches!(name, "develop" | "dev") {
        2
    } else {
        GROUPS
    }
}

/// The trunk branch: shown first and always colored 0.
fn primary_branch(repo: &Repository, explicit: Option<&str>) -> Result<Option<String>> {
    let candidates: &[&str] = match explicit {
        Some(name) => &[name],
        None => &[
            "main",
            "master",
            "trunk",
            "develop",
            "origin/main",
            "origin/master",
        ],
    };
    for name in candidates {
        if repo
            .revparse_single(name)
            .and_then(|o| o.peel_to_commit())
            .is_ok()
        {
            return Ok(Some(name.to_string()));
        }
    }
    match explicit {
        Some(name) => Err(anyhow!("primary branch '{name}' not found")),
        None => Ok(None),
    }
}

fn diff_for<'r>(repo: &'r Repository, id: Oid, pathspec: Option<&str>) -> Result<git2::Diff<'r>> {
    let commit = repo.find_commit(id)?;
    let tree = commit.tree()?;
    let parent_tree = commit.parent(0).ok().map(|p| p.tree()).transpose()?;
    let mut opts = DiffOptions::new();
    if let Some(p) = pathspec {
        opts.pathspec(p).disable_pathspec_match(true);
    }
    let mut diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))?;
    if pathspec.is_none() {
        diff.find_similar(Some(DiffFindOptions::new().renames(true)))?;
    }
    Ok(diff)
}

pub fn detail(repo: &Repository, id: Oid) -> Result<CommitDetail> {
    let commit = repo.find_commit(id)?;
    let diff = diff_for(repo, id, None)?;
    let mut files = Vec::new();
    for (i, delta) in diff.deltas().enumerate() {
        let status = match delta.status() {
            Delta::Added => 'A',
            Delta::Deleted => 'D',
            Delta::Modified => 'M',
            Delta::Renamed => 'R',
            Delta::Copied => 'C',
            Delta::Typechange => 'T',
            _ => '?',
        };
        let raw_path = delta
            .new_file()
            .path_bytes()
            .or(delta.old_file().path_bytes())
            .unwrap_or_default()
            .to_vec();
        let raw_old_path = (status == 'R' || status == 'C')
            .then(|| delta.old_file().path_bytes().map(<[u8]>::to_vec))
            .flatten();
        let path = String::from_utf8_lossy(&raw_path).into_owned();
        let old_path = raw_old_path
            .as_deref()
            .map(|p| String::from_utf8_lossy(p).into_owned());
        let (added, removed) = match git2::Patch::from_diff(&diff, i) {
            Ok(Some(p)) => p.line_stats().map(|(_, a, d)| (a, d)).unwrap_or((0, 0)),
            _ => (0, 0),
        };
        files.push(FileChange {
            status,
            path,
            old_path,
            raw_path,
            raw_old_path,
            added,
            removed,
        });
    }
    let committer = commit.committer();
    Ok(CommitDetail {
        id,
        message: lossy(Some(commit.message_bytes())).trim_end().to_string(),
        committer: format!(
            "{} <{}>",
            lossy(Some(committer.name_bytes())),
            lossy(Some(committer.email_bytes()))
        ),
        commit_time: committer.when().seconds(),
        commit_offset: committer.when().offset_minutes(),
        files,
    })
}

/// A diff header line with its quoted paths made readable. git (and libgit2) quote a
/// path that has non-ASCII bytes, so `檔案.txt` shows as `"a/\346\252\224\346\241\210.txt"`;
/// like `git -c core.quotePath=false`, such a path is shown as UTF-8 without quotes. A path
/// that needs its quotes (a space, a quote, a control character) keeps them.
fn unquote_paths(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let mut bytes = Vec::new();
        let mut chars = after.char_indices();
        let mut end = None;
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    end = Some(i);
                    break;
                }
                '\\' => match chars.next() {
                    Some((_, d @ '0'..='7')) => {
                        let mut v = d.to_digit(8).unwrap_or(0);
                        for _ in 0..2 {
                            match chars.clone().next() {
                                Some((_, o @ '0'..='7')) => {
                                    v = v * 8 + o.to_digit(8).unwrap_or(0);
                                    chars.next();
                                }
                                _ => break,
                            }
                        }
                        bytes.push(v as u8);
                    }
                    Some((_, e)) => bytes.push(match e {
                        'n' => b'\n',
                        't' => b'\t',
                        'r' => b'\r',
                        'a' => 7,
                        'b' => 8,
                        'f' => 12,
                        'v' => 11,
                        other => other as u8,
                    }),
                    None => {}
                },
                c => {
                    let mut buf = [0; 4];
                    bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            }
        }
        let Some(end) = end else {
            // Not a quoted path after all
            out.push_str(&rest[start..]);
            return out;
        };
        let path = String::from_utf8_lossy(&bytes);
        if path
            .chars()
            .any(|c| c.is_control() || c == '"' || c == '\\' || c == ' ')
        {
            out.push('"');
            out.push_str(&after[..end]);
            out.push('"');
        } else {
            out.push_str(&path);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Diff of a single file (or the whole commit when `path` is None).
pub fn file_diff(repo: &Repository, id: Oid, path: Option<&FileChange>) -> Result<Vec<DiffLine>> {
    let diff = match path {
        Some(f) => {
            // Include both sides so renames show their content diff.
            let mut opts = DiffOptions::new();
            opts.pathspec(f.raw_path.as_slice())
                .disable_pathspec_match(true);
            if let Some(old) = &f.raw_old_path {
                opts.pathspec(old.as_slice());
            }
            let commit = repo.find_commit(id)?;
            let parent_tree = commit.parent(0).ok().map(|p| p.tree()).transpose()?;
            let mut d = repo.diff_tree_to_tree(
                parent_tree.as_ref(),
                Some(&commit.tree()?),
                Some(&mut opts),
            )?;
            d.find_similar(Some(DiffFindOptions::new().renames(true)))?;
            d
        }
        None => diff_for(repo, id, None)?,
    };
    let mut lines = Vec::new();
    diff.print(git2::DiffFormat::Patch, |_, _, line| {
        let content = String::from_utf8_lossy(line.content())
            .trim_end_matches(['\n', '\r'])
            .replace('\t', "    ");
        let (kind, text) = match line.origin() {
            '+' => (DiffKind::Add, format!("+{content}")),
            '-' => (DiffKind::Del, format!("-{content}")),
            ' ' => (DiffKind::Context, format!(" {content}")),
            'H' => (DiffKind::Hunk, content),
            'F' => {
                for l in content.lines() {
                    lines.push(DiffLine {
                        kind: DiffKind::Header,
                        text: unquote_paths(l),
                    });
                }
                return true;
            }
            'B' => (DiffKind::Header, content),
            _ => (DiffKind::Context, content),
        };
        lines.push(DiffLine { kind, text });
        true
    })?;
    if lines.is_empty() {
        lines.push(DiffLine {
            kind: DiffKind::Header,
            text: "(no textual changes)".into(),
        });
    }
    Ok(lines)
}

/// Safe checkout (refuses to overwrite local changes). Checks out the local
/// branch if given, otherwise detaches HEAD at the commit. Everything is checked
/// before the working tree is touched.
pub fn checkout(repo: &Repository, id: Oid, branch: Option<&str>) -> Result<String> {
    let commit = repo.find_commit(id)?;
    let target = match branch {
        Some(b) => {
            let found = repo
                .find_branch(b, git2::BranchType::Local)
                .with_context(|| format!("no local branch '{b}'"))?;
            let name = found.get().name()?.to_string();
            if let Some(path) = checked_out_elsewhere(repo, &name)? {
                anyhow::bail!("'{b}' is already checked out at {path}");
            }
            Some(name)
        }
        None => None,
    };
    let mut cb = git2::build::CheckoutBuilder::new();
    cb.safe();
    repo.checkout_tree(commit.as_object(), Some(&mut cb))
        .context("checkout failed (uncommitted changes would be overwritten?)")?;
    match (branch, target) {
        (Some(b), Some(name)) => {
            repo.set_head(&name)?;
            Ok(format!("Switched to branch '{b}'"))
        }
        _ => {
            repo.set_head_detached(id)?;
            Ok(format!("HEAD detached at {}", &id.to_string()[..8]))
        }
    }
}

/// Where another worktree of this repository has `refname` checked out, if one does:
/// git keeps a branch checked out in one worktree at a time.
fn checked_out_elsewhere(repo: &Repository, refname: &str) -> Result<Option<String>> {
    let head_of = |r: &Repository| r.head().ok().and_then(|h| h.name().ok().map(str::to_owned));
    let here = repo.path().canonicalize()?;
    let main = Repository::open(repo.commondir())?;
    // (git dir, checked out ref, where to say it is)
    let mut worktrees = vec![(
        main.path().to_path_buf(),
        head_of(&main),
        main.workdir().map(|p| p.display().to_string()),
    )];
    for name in main.worktrees()?.iter().flatten().flatten() {
        let Ok(wt) = main.find_worktree(name) else {
            continue;
        };
        let Ok(r) = Repository::open_from_worktree(&wt) else {
            continue;
        };
        worktrees.push((
            r.path().to_path_buf(),
            head_of(&r),
            Some(wt.path().display().to_string()),
        ));
    }
    for (gitdir, head, path) in worktrees {
        if gitdir.canonicalize().is_ok_and(|g| g == here) {
            continue;
        }
        if head.as_deref() == Some(refname) {
            return Ok(Some(path.unwrap_or_else(|| gitdir.display().to_string())));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("git-graph-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn commit_file(repo: &Repository, file: &str, msg: &str) -> Oid {
        std::fs::write(repo.workdir().unwrap().join(file), msg).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new(file)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@example.com").unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents)
            .unwrap()
    }

    #[test]
    fn checkout_checks_before_touching_the_working_tree() {
        let (dir, wt_dir) = (scratch_dir("main"), scratch_dir("wt"));
        let repo = Repository::init(&dir).unwrap();
        let first = commit_file(&repo, "a.txt", "first");
        let main_branch = repo.head().unwrap().name().unwrap().to_string();
        // `feature`, one commit ahead, is checked out in a linked worktree
        let branch = repo
            .branch("feature", &repo.find_commit(first).unwrap(), false)
            .unwrap();
        let mut add = git2::WorktreeAddOptions::new();
        add.reference(Some(branch.get()));
        let wt = repo.worktree("wt", &wt_dir, Some(&add)).unwrap();
        let wt_repo = Repository::open_from_worktree(&wt).unwrap();
        let second = commit_file(&wt_repo, "b.txt", "second");

        let err = checkout(&repo, second, Some("feature")).unwrap_err();
        assert!(err.to_string().contains("already checked out"), "{err:#}");
        // Nothing changed here: same branch, no new file, nothing staged
        assert_eq!(repo.head().unwrap().name().unwrap(), main_branch);
        assert!(!dir.join("b.txt").exists());
        assert!(repo.statuses(None).unwrap().is_empty());

        // A detached HEAD's "HEAD" label is not a branch to switch to
        assert!(checkout(&repo, first, Some("HEAD")).is_err());
        assert_eq!(repo.head().unwrap().name().unwrap(), main_branch);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&wt_dir);
    }

    #[test]
    fn quoted_paths_become_readable() {
        assert_eq!(
            unquote_paths(
                r#"diff --git "a/\346\252\224\346\241\210.txt" "b/\346\252\224\346\241\210.txt""#
            ),
            "diff --git a/檔案.txt b/檔案.txt"
        );
        assert_eq!(
            unquote_paths(r#"--- "a/\346\252\224 x.txt""#),
            r#"--- "a/\346\252\224 x.txt""#
        );
        assert_eq!(unquote_paths("+++ b/plain.txt"), "+++ b/plain.txt");
        assert_eq!(
            unquote_paths(r#"rename from "\344\270\255""#),
            "rename from 中"
        );
    }

    #[test]
    fn parses_merge_messages() {
        assert_eq!(
            parse_merge("Merge branch 'feature/x'"),
            (None, v(&["feature/x"]))
        );
        assert_eq!(
            parse_merge("Merge branch 'fix' into develop"),
            (Some("develop".into()), v(&["fix"]))
        );
        assert_eq!(
            parse_merge("Merge branch 'a' into 'main'"),
            (Some("main".into()), v(&["a"]))
        );
        assert_eq!(
            parse_merge("Merge branches 'a', 'b' and 'c'"),
            (None, v(&["a", "b", "c"]))
        );
        assert_eq!(
            parse_merge("Merge remote-tracking branch 'origin/dev'"),
            (None, v(&["origin/dev"]))
        );
        assert_eq!(
            parse_merge("Merge branch 'x' of github.com:o/r"),
            (None, v(&["x"]))
        );
        assert_eq!(
            parse_merge("Merge pull request #81 from git-bahn/bugfix/cargo-install"),
            (None, v(&["bugfix/cargo-install"]))
        );
        assert_eq!(
            parse_merge("Merged in feature/y (pull request #3)"),
            (None, v(&["feature/y"]))
        );
        assert_eq!(parse_merge("Merge tag 'v1.0'"), (None, vec![]));
        assert_eq!(parse_merge("Fix merge bug"), (None, vec![]));
    }

    #[test]
    fn normalizes_remote_names() {
        let remotes = v(&["origin", "upstream"]);
        assert_eq!(normalize("origin/feature/x", &remotes), "feature/x");
        assert_eq!(normalize("feature/x", &remotes), "feature/x");
        assert_eq!(normalize("upstream/main", &remotes), "main");
    }

    #[test]
    fn name_color_is_stable_and_reserves_primary() {
        assert_eq!(name_color("main", Some("main")), 0);
        let c = name_color("feature/x", Some("main"));
        assert_eq!(c, name_color("feature/x", None));
        assert!((1..graph::PALETTE_SIZE).contains(&c));
    }
}
