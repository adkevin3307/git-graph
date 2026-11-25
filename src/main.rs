mod app;
mod graph;
mod input;
mod matcher;
mod repo;
mod selection;
mod signals;
mod terminal;
mod ui;

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use clap::builder::styling::{AnsiColor, Styles};
use ratatui::backend::Backend;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use signals::Signals;

#[derive(Parser)]
#[command(
    name = "git-graph",
    version,
    about = "A git history viewer: commit graph, details, files and diffs, with search and a branch picker.",
    long_about = None,
    styles = Styles::styled()
        .header(AnsiColor::Yellow.on_default())
        .usage(AnsiColor::Yellow.on_default())
        .literal(AnsiColor::Green.on_default()),
)]
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
    /// Don't show stash entries.
    #[arg(long)]
    no_stash: bool,
    /// Don't capture the mouse: drag uses the terminal's own selection, and the wheel
    /// scrolls only where the terminal supports alternate scroll mode.
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
        stash: !cli.no_stash,
    };

    if cli.print {
        let snap = repo::load(&repo, &opts)?;
        // A reader that stops early (`| head`) is not an error
        return match print_graph(&snap) {
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            other => Ok(other?),
        };
    }

    let mut app = app::App::new(repo, opts)?;
    // A panic anywhere restores the terminal; termination signals end through the
    // same cleanup as q.
    terminal::on_panic(|| {});
    let guard = terminal::enter(stdout, !cli.no_mouse)?;
    let signals = Signals::catch(|| {})?;
    let mut terminal = terminal::tui(io::stdout())?;
    let result = run(&mut terminal, &mut app, &signals);
    drop(terminal);
    drop(guard);
    if let Some(sig) = result? {
        std::process::exit(signals::exit_status(sig));
    }
    Ok(())
}

fn stdout() -> Box<dyn Write> {
    Box::new(io::stdout())
}

/// `--print`: the graph as plain text.
fn print_graph(snap: &repo::Snapshot) -> io::Result<()> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    {
        for r in &snap.rows {
            let g: String = r.cells.iter().map(graph::Cell::ch).collect();
            let Some(i) = r.commit else {
                writeln!(out, "{}", g.trim_end())?;
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
            writeln!(out, "{g:<16} {} {refs}{}", c.short, c.summary)?;
        }
    }
    out.flush()
}

/// Returns the signal that ended the program, if one did.
fn run(
    terminal: &mut terminal::Tui<io::Stdout>,
    app: &mut app::App,
    signals: &Signals,
) -> Result<Option<i32>> {
    while !app.quit {
        if let Some(sig) = signals.received() {
            return Ok(Some(sig));
        }
        if std::mem::take(&mut app.redraw) {
            refresh_screen(terminal)?;
        }
        terminal.draw(|f| ui::draw(f, app))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        // Handle every queued event before the next draw, like track and monitor
        loop {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => app.on_key(k),
                Event::Mouse(m) => app.on_mouse(m),
                _ => {}
            }
            if app.quit || !event::poll(Duration::ZERO)? {
                break;
            }
        }
    }
    Ok(None)
}

/// Redraws the whole screen (r: refresh), the same in track, monitor and git-graph.
/// Unlike `Terminal::clear`, this doesn't ask the terminal for the cursor position
/// (`\e[6n`), so a terminal that doesn't answer can't make it fail.
fn refresh_screen(terminal: &mut terminal::Tui<io::Stdout>) -> std::io::Result<()> {
    // Through the backend, so it also forgets what the screen showed
    Backend::clear(terminal.backend_mut())?;
    // An empty frame first, so the next frame repaints every cell.
    terminal.draw(|_| {})?;
    Ok(())
}
