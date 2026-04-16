use ratatui::Frame;
use ratatui::layout::Alignment;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::state::AppState;

pub fn render_placeholder(frame: &mut Frame<'_>, state: &AppState) {
    let block = Block::default()
        .title("Telepathy TUI")
        .borders(Borders::ALL);

    let text = format!("{}\n\nPress q to quit.", state.placeholder_message);
    let paragraph = Paragraph::new(text)
        .block(block)
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, frame.area());
}
