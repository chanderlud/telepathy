//! Components for telepathy-tui.

use tuirealm::command::{Cmd, CmdResult};
use tuirealm::props::{AnyPropBox, PropBoundExt, PropPayload};
use tuirealm::ratatui::layout::Rect;
use tuirealm::ratatui::widgets::Paragraph;
use tuirealm::{
    AttrValue, Attribute, Component, Event, Frame, MockComponent, Props, State as TuiState,
};

use crate::events::{CoreEvent, Id, Msg};
use crate::state::ChatEntry;
use crate::storage::config::ContactMeta;

pub mod call_controls_pane;
pub mod chat_pane;
pub mod confirm_dialog;
pub mod contacts_pane;
pub mod incoming_call_dialog;
pub mod status_bar;

pub use call_controls_pane::CallControlsPane;
pub use chat_pane::ChatPane;
pub use confirm_dialog::ConfirmDialog;
pub use contacts_pane::ContactsPane;
pub use incoming_call_dialog::IncomingCallDialog;
pub use status_bar::StatusBar;

#[derive(Debug, Clone, PartialEq)]
pub enum SessionBadge {
    Connecting,
    ConnectedDirect,
    ConnectedRelayed,
    Inactive,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContactsPaneData {
    pub contacts: Vec<ContactMeta>,
    pub rooms: Vec<String>,
    pub sessions: std::collections::HashMap<String, SessionBadge>,
    pub call_active: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallControlsData {
    pub muted: bool,
    pub deafened: bool,
    pub call_active: bool,
    pub manager_active: bool,
    pub manager_restartable: bool,
    pub output_vol: f32,
    pub input_vol: f32,
    pub sound_vol: f32,
    pub sensitivity: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatPaneData {
    pub entries: Vec<ChatEntry>,
    pub active_peer: Option<String>,
    pub call_active: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IncomingCallDialogData {
    pub request_id: String,
    pub contact_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmDialogData {
    pub message: String,
    pub confirm_msg: Msg,
}

pub fn payload_attr<T>(value: T) -> AttrValue
where
    T: tuirealm::props::PropBound + 'static,
{
    AttrValue::Payload(PropPayload::Any(Box::new(value)))
}

pub fn payload_content<T>(props: &Props) -> Option<T>
where
    T: Clone + 'static,
{
    let value = props.get(Attribute::Content)?;
    payload_from_attr(value)
}

pub fn payload_from_attr<T>(value: AttrValue) -> Option<T>
where
    T: Clone + 'static,
{
    match value {
        AttrValue::Payload(PropPayload::Any(payload)) => payload_to_type::<T>(payload),
        _ => None,
    }
}

fn payload_to_type<T>(payload: AnyPropBox) -> Option<T>
where
    T: Clone + 'static,
{
    payload.as_any().downcast_ref::<T>().cloned()
}

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

pub fn placeholder_ids() -> &'static [Id] {
    &[Id::CoreEventBridge]
}
