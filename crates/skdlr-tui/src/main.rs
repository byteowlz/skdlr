//! skdlr-tui - Interactive TUI for schedule management.
//!
//! This is a placeholder implementation. Full TUI implementation is tracked in:
//! https://github.com/byteowlz/skdlr/issues/skdlr-er3

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use skdlr_core::paths::AppPaths;
use skdlr_core::{SkdlrConfig, Storage};

fn main() {
    if let Err(err) = try_main() {
        let _ = writeln!(io::stderr(), "error: {err:#}");
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
    let cli = Cli::parse();
    let paths = AppPaths::discover(cli.config)?;
    let _config = SkdlrConfig::load(&paths, false)?;
    let storage = Storage::open(&paths.db_path)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(storage)?;
    let result = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

#[derive(Debug, Parser)]
#[command(
    name = "skdlr-tui",
    about = "Interactive TUI for skdlr schedule management"
)]
struct Cli {
    /// Override the config file path
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
}

struct App {
    schedules: Vec<String>,
    selected: usize,
    should_quit: bool,
}

impl App {
    fn new(storage: Storage) -> Result<Self> {
        let schedules = storage
            .list_schedules()?
            .into_iter()
            .map(|s| format!("{} - {} - {}", s.name, s.status, s.cron_expr))
            .collect();

        Ok(Self {
            schedules,
            selected: 0,
            should_quit: false,
        })
    }

    fn next(&mut self) {
        if !self.schedules.is_empty() {
            self.selected = (self.selected + 1) % self.schedules.len();
        }
    }

    fn previous(&mut self) {
        if !self.schedules.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.schedules.len() - 1);
        }
    }
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => app.should_quit = true,
                KeyCode::Down | KeyCode::Char('j') => app.next(),
                KeyCode::Up | KeyCode::Char('k') => app.previous(),
                _ => {}
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Header
    let header = Paragraph::new("skdlr - Schedule Manager")
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    // Schedule list
    let items: Vec<ListItem> = app
        .schedules
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let style = if i == app.selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(s.clone(), style)))
        })
        .collect();

    let list = List::new(items).block(Block::default().title("Schedules").borders(Borders::ALL));
    f.render_widget(list, chunks[1]);

    // Footer
    let footer = Paragraph::new("q: Quit | j/k: Navigate | Enter: Select")
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}
