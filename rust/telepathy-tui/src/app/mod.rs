use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{ExecutableCommand, execute};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use thiserror::Error;

use crate::components;
use crate::events::AppEvent;
use crate::state::AppState;

type AppTerminal = Terminal<CrosstermBackend<Stdout>>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

pub fn run() -> Result<(), AppError> {
    let mut terminal = setup_terminal()?;
    let mut app_state = AppState::default();

    let run_result = run_loop(&mut terminal, &mut app_state);
    let teardown_result = restore_terminal(&mut terminal);

    run_result.and(teardown_result)
}

fn setup_terminal() -> Result<AppTerminal, AppError> {
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut AppTerminal) -> Result<(), AppError> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_loop(terminal: &mut AppTerminal, app_state: &mut AppState) -> Result<(), AppError> {
    while app_state.running {
        terminal.draw(|frame| {
            components::render_placeholder(frame, app_state);
        })?;

        if !event::poll(Duration::from_millis(100))? {
            app_state.handle_event(AppEvent::Tick);
            continue;
        }

        if let CrosstermEvent::Key(key_event) = event::read()?
            && key_event.kind == KeyEventKind::Press
            && matches!(key_event.code, KeyCode::Char('q') | KeyCode::Char('Q'))
        {
            app_state.handle_event(AppEvent::QuitRequested);
        }
    }

    Ok(())
}
