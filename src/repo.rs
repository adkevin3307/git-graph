use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use git2::{BranchType, Delta, DiffFindOptions, DiffOptions, Oid, Repository, Sort};

use crate::graph::{self, Node, Row};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Head,
    Local,
    Remote,
    Tag,
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
    pub author: String,
    pub email: String,
    pub time: i64,
    pub offset_min: i32,
    /// Branch this commit is attributed to (from branch tips or merge messages).
    pub branch: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub status: char,
    pub path: String,
    pub old_path: Option<String>,
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
    pub rows: Vec<Row>,
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
}

pub fn open(path: &Path) -> Result<Repository> {
    Repository::discover(path).with_context(|| format!("not a git repository: {}", path.display()))
}

pub fn load(repo: &Repository, opts: &LoadOptions) -> Result<Snapshot> {
    let mut refs: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
    let mut branches = Vec::new();
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
            refs.entry(commit.id()).or_default().push(RefLabel {
                name,
                kind: RefKind::Tag,
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
            summary: commit.summary().ok().flatten().unwrap_or("").to_string(),
            author: author.name().unwrap_or("?").to_string(),
            email: author.email().unwrap_or("").to_string(),
            time: author.when().seconds(),
            offset_min: author.when().offset_minutes(),
            branch: None,
        });
    }
    let index: HashMap<Oid, usize> = commits.iter().enumerate().map(|(i, c)| (c.id, i)).collect();

    // Primary branch: its first-parent chain is pinned to lane 0.
    let (primary_name, primary_set) = primary_chain(repo, opts.primary.as_deref(), &index)?;

    // Attribute commits to branches so each branch keeps one color, stable
    // across merges, rebases and restarts (the color is a hash of the name).
    let remotes: Vec<String> = repo
        .remotes()?
        .iter()
        .flatten()
        .flatten()
        .map(str::to_string)
        .collect();
    let primary_norm = primary_name.as_deref().map(|p| normalize(p, &remotes));
    let names = assign_branches(
        &commits,
        &index,
        primary_norm.as_deref(),
        &primary_set,
        &branches,
        &remotes,
    );
    for (c, name) in commits.iter_mut().zip(names) {
        c.branch = name;
    }

    let rows = graph::layout(
        commits.iter().map(|c| Node {
            id: c.id,
            parents: &c.parents,
            color: c
                .branch
                .as_deref()
                .map(|b| name_color(b, primary_norm.as_deref())),
        }),
        &primary_set,
        opts.compact,
    );

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
        rows,
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

fn assign_branches(
    commits: &[CommitInfo],
    index: &HashMap<Oid, usize>,
    primary: Option<&str>,
    primary_set: &HashSet<Oid>,
    branches: &[BranchEntry],
    remotes: &[String],
) -> Vec<Option<String>> {
    let mut names: Vec<Option<String>> = vec![None; commits.len()];
    // Claim the first-parent chain from `start` until an attributed commit.
    let walk = |names: &mut Vec<Option<String>>, start: Oid, name: &str| {
        let mut cur = start;
        while let Some(&i) = index.get(&cur) {
            if names[i].is_some() {
                break;
            }
            names[i] = Some(name.to_string());
            match commits[i].parents.first() {
                Some(&p) => cur = p,
                None => break,
            }
        }
    };

    if let Some(p) = primary {
        for &id in primary_set {
            names[index[&id]] = Some(p.to_string());
        }
    }
    // Branch tips: checked-out branch first, then local, then remote.
    for b in branches {
        walk(&mut names, b.target, &normalize(&b.label.name, remotes));
    }
    // Merged (possibly deleted) branches, named by merge messages, top-down.
    for c in commits {
        if c.parents.len() < 2 {
            continue;
        }
        let (into, merged) = parse_merge(&c.summary);
        if let Some(into) = into {
            walk(&mut names, c.id, &normalize(&into, remotes));
        }
        for (k, name) in merged.iter().enumerate() {
            if let Some(&p) = c.parents.get(k + 1) {
                walk(&mut names, p, &normalize(name, remotes));
            }
        }
    }
    names
}

fn primary_chain(
    repo: &Repository,
    explicit: Option<&str>,
    index: &HashMap<Oid, usize>,
) -> Result<(Option<String>, HashSet<Oid>)> {
    let candidates: Vec<String> = match explicit {
        Some(name) => vec![name.to_string()],
        None => [
            "main",
            "master",
            "trunk",
            "develop",
            "origin/main",
            "origin/master",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect(),
    };
    let mut found = None;
    for name in &candidates {
        if let Ok(obj) = repo.revparse_single(name)
            && let Ok(c) = obj.peel_to_commit()
        {
            found = Some((name.clone(), c));
            break;
        }
    }
    let Some((name, mut commit)) = found else {
        if let Some(name) = explicit {
            return Err(anyhow!("primary branch '{name}' not found"));
        }
        return Ok((None, HashSet::new()));
    };
    let mut set = HashSet::new();
    loop {
        if !index.contains_key(&commit.id()) || !set.insert(commit.id()) {
            break;
        }
        match commit.parent(0) {
            Ok(p) => commit = p,
            Err(_) => break,
        }
    }
    Ok((Some(name), set))
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
        let path = delta
            .new_file()
            .path()
            .or(delta.old_file().path())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let old_path = (status == 'R' || status == 'C')
            .then(|| delta.old_file().path().map(|p| p.display().to_string()))
            .flatten();
        let (added, removed) = match git2::Patch::from_diff(&diff, i) {
            Ok(Some(p)) => p.line_stats().map(|(_, a, d)| (a, d)).unwrap_or((0, 0)),
            _ => (0, 0),
        };
        files.push(FileChange {
            status,
            path,
            old_path,
            added,
            removed,
        });
    }
    let committer = commit.committer();
    Ok(CommitDetail {
        id,
        message: commit.message().unwrap_or("").trim_end().to_string(),
        committer: format!(
            "{} <{}>",
            committer.name().unwrap_or("?"),
            committer.email().unwrap_or("")
        ),
        commit_time: committer.when().seconds(),
        commit_offset: committer.when().offset_minutes(),
        files,
    })
}

/// Diff of a single file (or the whole commit when `path` is None).
pub fn file_diff(repo: &Repository, id: Oid, path: Option<&FileChange>) -> Result<Vec<DiffLine>> {
    let diff = match path {
        Some(f) => {
            // Include both sides so renames show their content diff.
            let mut opts = DiffOptions::new();
            opts.pathspec(&f.path).disable_pathspec_match(true);
            if let Some(old) = &f.old_path {
                opts.pathspec(old);
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
                        text: l.to_string(),
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
/// branch if given, otherwise detaches HEAD at the commit.
pub fn checkout(repo: &Repository, id: Oid, branch: Option<&str>) -> Result<String> {
    let commit = repo.find_commit(id)?;
    let mut cb = git2::build::CheckoutBuilder::new();
    cb.safe();
    repo.checkout_tree(commit.as_object(), Some(&mut cb))
        .context("checkout failed (uncommitted changes would be overwritten?)")?;
    match branch {
        Some(b) => {
            repo.set_head(&format!("refs/heads/{b}"))?;
            Ok(format!("Switched to branch '{b}'"))
        }
        None => {
            repo.set_head_detached(id)?;
            Ok(format!("HEAD detached at {}", &id.to_string()[..8]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
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
