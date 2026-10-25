mod app;
mod editor;
mod markdown;
mod notes;
mod theme;
mod ui;

use std::io::{self, stdout, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use ratatui::crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind},
    execute,
    terminal::SetTitle,
};
use ratatui::DefaultTerminal;
use serde::Deserialize;

use crate::app::App;
use crate::notes::Store;
use crate::theme::ThemeSource;

/// How often to look for outside changes to the notes folder and the theme.
const WATCH_INTERVAL: Duration = Duration::from_secs(1);
const POLL_TIMEOUT: Duration = Duration::from_millis(250);

/// Quick notes for the terminal.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Notes folder [default: ~/Notes]
    #[arg(short, long, value_name = "PATH", global = true)]
    dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open straight into a new note; closing it quits
    New {
        /// Text to start the note with
        text: Vec<String>,
    },
    /// Save a note without opening the interface (reads stdin when no text is given)
    Add { text: Vec<String> },
    /// Print the notes folder
    Dir,
}

/// `~/.config/jot/config.toml`
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    /// Notes folder.
    dir: Option<String>,
    /// Program for `e`; falls back to $VISUAL, $EDITOR, then nvim.
    editor: Option<String>,
    /// "auto" follows the Omarchy theme, "terminal" uses the terminal colors.
    theme: Option<String>,
}

impl Config {
    fn load() -> Result<Self> {
        let Some(path) = dirs::config_dir().map(|d| d.join("jot/config.toml")) else {
            return Ok(Self::default());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("invalid config {}", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("could not read {}", path.display())),
        }
    }
}

fn expand(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("jot: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;
    let dir = cli
        .dir
        .or_else(|| std::env::var_os("JOT_DIR").map(|d| expand(&d.to_string_lossy())))
        .or_else(|| config.dir.as_deref().map(expand))
        .or_else(|| dirs::home_dir().map(|h| h.join("Notes")))
        .context("could not find the home directory, pass --dir")?;
    let mut store = Store::open(dir)?;

    let mut capture = None;
    match cli.command {
        None => {}
        Some(Cmd::Dir) => {
            println!("{}", store.dir().display());
            return Ok(());
        }
        Some(Cmd::Add { text }) => {
            let mut text = text.join(" ");
            if text.is_empty() && !io::stdin().is_terminal() {
                io::stdin().read_to_string(&mut text)?;
            }
            if text.trim().is_empty() {
                bail!("nothing to save");
            }
            let path = store.save(None, text.trim_end())?;
            println!("{}", path.display());
            return Ok(());
        }
        Some(Cmd::New { text }) => capture = Some(text.join(" ")),
    }

    let theme_enabled = config.theme.as_deref() != Some("terminal");
    let mut theme = ThemeSource::new(theme_enabled, dirs::home_dir().as_deref());
    let mut app = App::new(store)?;
    if let Some(text) = capture {
        app.capture = true;
        app.new_note(&text);
    }
    let editor = config
        .editor
        .or_else(|| std::env::var("VISUAL").ok())
        .or_else(|| std::env::var("EDITOR").ok())
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| "nvim".into());

    let mut terminal = ratatui::init();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableBracketedPaste);
        hook(info);
    }));
    let _ = execute!(stdout(), EnableBracketedPaste, SetTitle("jot"));

    let result = event_loop(&mut terminal, &mut app, &mut theme, &editor);

    // Never leave without writing what was typed.
    app.finish();
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    theme: &mut ThemeSource,
    editor: &str,
) -> Result<()> {
    let mut last_watch = Instant::now();
    loop {
        let palette = theme.palette();
        terminal.draw(|f| ui::draw(f, app, &palette))?;

        if event::poll(POLL_TIMEOUT)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
                Event::Paste(text) => app.handle_paste(&text),
                _ => {}
            }
        }
        if app.should_quit {
            return Ok(());
        }

        if let Some(path) = app.external_edit.take() {
            if let Err(e) = open_external(terminal, editor, &path) {
                app.error(format!("{e:#}"));
            }
            app.reload(Some(path));
        }

        if last_watch.elapsed() >= WATCH_INTERVAL {
            last_watch = Instant::now();
            if app.editing.is_none() && app.store.changed() {
                app.reload(None);
            }
            theme.poll();
        }
        app.tick();
    }
}

/// Hand the terminal to the external editor and take it back afterwards.
fn open_external(terminal: &mut DefaultTerminal, editor: &str, path: &Path) -> Result<()> {
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("nvim");
    let status = Command::new(program).args(parts).arg(path).status();
    *terminal = ratatui::init();
    let _ = execute!(stdout(), EnableBracketedPaste, SetTitle("jot"));
    terminal.clear()?;
    status.with_context(|| format!("could not start {program}"))?;
    Ok(())
}
