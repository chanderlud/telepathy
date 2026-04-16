#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AppEvent {
    Tick,
    QuitRequested,
}
