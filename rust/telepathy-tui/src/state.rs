use crate::events::AppEvent;

#[derive(Debug, Clone)]
pub struct AppState {
    pub running: bool,
    pub placeholder_message: String,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            running: true,
            placeholder_message: String::from("telepathy-tui scaffold initialized"),
        }
    }
}

impl AppState {
    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Tick => {}
            AppEvent::QuitRequested => self.running = false,
        }
    }
}
