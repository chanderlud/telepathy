use tuirealm::event::{Key, KeyEvent, KeyModifiers};
use tuirealm::ratatui::layout::Rect;
use tuirealm::ratatui::widgets::Paragraph;
use tuirealm::{AttrValue, Attribute, Component, Event, Frame, MockComponent, Props, State};

use crate::events::{CoreEvent, Msg};

#[derive(Default)]
struct StatusBarInner {
    props: Props,
}

impl MockComponent for StatusBarInner {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let text = self
            .props
            .get_or(Attribute::Text, AttrValue::String(String::new()))
            .unwrap_string();
        frame.render_widget(Paragraph::new(text), area);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.props.get(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        self.props.set(attr, value);
    }

    fn state(&self) -> State {
        State::None
    }

    fn perform(&mut self, _cmd: tuirealm::command::Cmd) -> tuirealm::command::CmdResult {
        tuirealm::command::CmdResult::None
    }
}

#[derive(Default)]
pub struct StatusBar {
    component: StatusBarInner,
}

impl MockComponent for StatusBar {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.component.view(frame, area);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.component.query(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        self.component.attr(attr, value);
    }

    fn state(&self) -> State {
        self.component.state()
    }

    fn perform(&mut self, cmd: tuirealm::command::Cmd) -> tuirealm::command::CmdResult {
        self.component.perform(cmd)
    }
}

impl Component<Msg, CoreEvent> for StatusBar {
    fn on(&mut self, ev: Event<CoreEvent>) -> Option<Msg> {
        match ev {
            Event::Keyboard(KeyEvent {
                code: Key::Char('s'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::OpenSettings),
            Event::Keyboard(KeyEvent {
                code: Key::Char('l'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::OpenLogs),
            Event::Keyboard(KeyEvent {
                code: Key::Char('q'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::Quit),
            Event::Keyboard(KeyEvent {
                code: Key::Tab,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::FocusContacts),
            _ => None,
        }
    }
}
