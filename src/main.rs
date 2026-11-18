mod graph;
mod repo;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

/// Terminal git graph viewer.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Path inside the repository.
    #[arg(default_value = ".")]
    path: PathBuf,
    /// Only show history reachable from HEAD (default: all branches and tags).
    #[arg(long)]
    head_only: bool,
    /// Maximum number of commits to load.
    #[arg(short = 'n', long, default_value_t = 20_000)]
    limit: usize,
    /// Branch pinned to the leftmost lane (default: main, master, trunk, develop).
    #[arg(short, long)]
    main: Option<String>,
    /// Keep lanes in place instead of shifting them left into free columns.
    #[arg(long)]
    no_compact: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo = repo::open(&cli.path)?;
    let opts = repo::LoadOptions {
        all: !cli.head_only,
        limit: cli.limit,
        primary: cli.main.clone(),
        compact: !cli.no_compact,
    };

    let snap = repo::load(&repo, &opts)?;
    for (c, r) in snap.commits.iter().zip(&snap.rows) {
        let g: String = r.cells.iter().map(graph::Cell::ch).collect();
        let refs = snap
            .refs
            .get(&c.id)
            .map(|ls| {
                ls.iter()
                    .map(|l| format!("({}) ", l.name))
                    .collect::<String>()
            })
            .unwrap_or_default();
        println!("{g:<16} {} {refs}{}", c.short, c.summary);
    }
    Ok(())
}
