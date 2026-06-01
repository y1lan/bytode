pub mod app_shell;
pub mod events;
pub mod overlays;
pub mod panels;
pub mod render;
pub mod statusbar;
pub mod window_manager;

use crossterm::{
    execute,
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use std::io::{self, Stdout};

pub struct Terminal {
    terminal: ratatui::Terminal<CrosstermBackend<Stdout>>,
}

impl Terminal {
    pub fn init() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = ratatui::Terminal::new(backend)?;
        Ok(Terminal { terminal })
    }

    pub fn draw<F>(&mut self, f: F) -> io::Result<ratatui::CompletedFrame<'_>>
    where
        F: FnOnce(&mut Frame),
    {
        self.terminal.draw(f)
    }

    pub fn restore() -> io::Result<()> {
        disable_raw_mode()?;
        execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
        Ok(())
    }
}
