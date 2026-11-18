mod app;
mod graph;
mod repo;
mod ui;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;

/// Interactive terminal git graph viewer.
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
    /// Trunk branch: leftmost column and color 0 (default: main, master, trunk, develop).
    #[arg(short, long)]
    main: Option<String>,
    /// Give every branch connector its own row instead of sharing free
    /// commit rows (like git-igitt's --sparse).
    #[arg(long)]
    no_compact: bool,
    /// Disable mouse support (keeps terminal text selection working).
    #[arg(long)]
    no_mouse: bool,
    /// Print the graph to stdout instead of starting the TUI.
    #[arg(long)]
    print: bool,
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

    if cli.print {
        let snap = repo::load(&repo, &opts)?;
        for r in &snap.rows {
            let g: String = r.cells.iter().map(graph::Cell::ch).collect();
            let Some(i) = r.commit else {
                println!("{}", g.trim_end());
                continue;
            };
            let c = &snap.commits[i];
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
        return Ok(());
    }

    let mut app = app::App::new(repo, opts)?;
    let mut terminal = ratatui::init();
    if !cli.no_mouse {
        execute!(std::io::stdout(), EnableMouseCapture)?;
    }
    let result = run(&mut terminal, &mut app);
    if !cli.no_mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> Result<()> {
    while !app.quit {
        terminal.draw(|f| ui::draw(f, app))?;
        if !event::poll(Duration::from_millis(500))? {
            continue;
        }
        match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => app.on_key(k),
            Event::Mouse(m) => app.on_mouse(m),
            _ => {}
        }
    }
    Ok(())
}
