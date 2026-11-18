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
        });
    }
    let index: HashMap<Oid, usize> = commits.iter().enumerate().map(|(i, c)| (c.id, i)).collect();

    // Primary branch: its first-parent chain is pinned to lane 0.
    let (primary_name, primary_set) = primary_chain(repo, opts.primary.as_deref(), &index)?;
    let rows = graph::layout(
        commits.iter().map(|c| Node {
            id: c.id,
            parents: &c.parents,
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
    })
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
