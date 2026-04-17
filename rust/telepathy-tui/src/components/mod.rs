//! Component scaffolding for telepathy-tui.
//!
//! T4 only ships an empty placeholder component used to satisfy mounts in
//! [`crate::app::model::Model`]; the real components for the contacts pane,
//! call controls, chat pane, status bar and overlays land in T5/T6.

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::ratatui::layout::Rect;
use tuirealm::ratatui::widgets::Paragraph;
use tuirealm::{
    AttrValue, Attribute, Component, Event, Frame, MockComponent, Props, State as TuiState,
};

use crate::events::{CoreEvent, Id, Msg};

/// Empty component used by [`Model`](crate::app::model::Model) for every
/// [`Id`] until the real components are implemented.
#[derive(Default)]
pub struct PlaceholderComponent {
    props: Props,
}

impl MockComponent for PlaceholderComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if self.props.get_or(Attribute::Display, AttrValue::Flag(true)) == AttrValue::Flag(true) {
            frame.render_widget(Paragraph::new(""), area);
        }
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.props.get(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        self.props.set(attr, value);
    }

    fn state(&self) -> TuiState {
        TuiState::None
    }

    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::None
    }
}

impl Component<Msg, CoreEvent> for PlaceholderComponent {
    fn on(&mut self, _ev: Event<CoreEvent>) -> Option<Msg> {
        None
    }
}

/// Bridges core user events from the event loop into model messages.
#[derive(Default)]
pub struct CoreEventBridgeComponent;

impl MockComponent for CoreEventBridgeComponent {
    fn view(&mut self, _frame: &mut Frame, _area: Rect) {}

    fn query(&self, _attr: Attribute) -> Option<AttrValue> {
        None
    }

    fn attr(&mut self, _attr: Attribute, _value: AttrValue) {}

    fn state(&self) -> TuiState {
        TuiState::None
    }

    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::None
    }
}

impl Component<Msg, CoreEvent> for CoreEventBridgeComponent {
    fn on(&mut self, ev: Event<CoreEvent>) -> Option<Msg> {
        match ev {
            Event::User(core_event) => Some(Msg::CoreEvent(core_event)),
            _ => None,
        }
    }
}

/// All component identifiers that [`Model::new`](crate::app::model::Model::new)
/// mounts at startup with [`PlaceholderComponent`]. T5 will replace these
/// stubs one identifier at a time.
pub fn placeholder_ids() -> &'static [Id] {
    &[
        Id::ContactsPane,
        Id::CallControlsPane,
        Id::ChatPane,
        Id::StatusBar,
    ]
}
