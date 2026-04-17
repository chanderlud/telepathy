use tuirealm::event::{Key, KeyEvent, KeyModifiers};
use tuirealm::ratatui::layout::{Constraint, Layout, Rect};
use tuirealm::ratatui::style::{Modifier, Style};
use tuirealm::ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use tuirealm::{AttrValue, Attribute, Component, Event, Frame, MockComponent, Props, State};

use crate::components::{ChatPaneData, payload_content};
use crate::events::{CoreEvent, Msg};

#[derive(Default)]
struct OwnStates {
    input: String,
    scroll_offset: usize,
    input_focused: bool,
}

#[derive(Default)]
struct ChatPaneInner {
    props: Props,
    state: OwnStates,
}

impl MockComponent for ChatPaneInner {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let data = payload_content::<ChatPaneData>(&self.props).unwrap_or(ChatPaneData {
            entries: Vec::new(),
            active_peer: None,
            call_active: false,
        });

        let [history_area, input_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).areas(area);

        let mut lines = Vec::new();
        for entry in &data.entries {
            lines.push(format!("[--:--] <{}>: {}", entry.peer_id, entry.text));
        }
        let message_text = lines.join("\n");
        let mut history = Paragraph::new(message_text)
            .block(Block::default().title("Chat").borders(Borders::ALL))
            .wrap(Wrap { trim: false });
        history = history.scroll((self.state.scroll_offset as u16, 0));

        let mut pane_style = Style::default();
        if !data.call_active {
            pane_style = pane_style.add_modifier(Modifier::DIM);
        }
        frame.render_widget(history.style(pane_style), history_area);

        let input_text = if data.call_active {
            self.state.input.clone()
        } else {
            "(call a contact to chat)".to_string()
        };
        let input_border_style = if self.state.input_focused {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        frame.render_widget(
            Paragraph::new(input_text)
                .block(
                    Block::default()
                        .title("Input")
                        .borders(Borders::ALL)
                        .border_style(input_border_style),
                )
                .style(pane_style),
            input_area,
        );
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
pub struct ChatPane {
    component: ChatPaneInner,
}

impl MockComponent for ChatPane {
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

impl Component<Msg, CoreEvent> for ChatPane {
    fn on(&mut self, ev: Event<CoreEvent>) -> Option<Msg> {
        let enabled = payload_content::<ChatPaneData>(&self.component.props)
            .map(|p| p.call_active)
            .unwrap_or(false);
        match ev {
            Event::Keyboard(KeyEvent {
                code: Key::Char(ch),
                modifiers: KeyModifiers::NONE,
            }) if enabled => {
                self.component.state.input.push(ch);
                Some(Msg::None)
            }
            Event::Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) if enabled => {
                self.component.state.input.pop();
                Some(Msg::None)
            }
            Event::Keyboard(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::NONE,
            }) if enabled => {
                if self.component.state.input.trim().is_empty() {
                    return Some(Msg::None);
                }
                let text = std::mem::take(&mut self.component.state.input);
                Some(Msg::SendMessage(text))
            }
            Event::Keyboard(KeyEvent {
                code: Key::Up,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.state.scroll_offset = self.component.state.scroll_offset.saturating_sub(1);
                Some(Msg::None)
            }
            Event::Keyboard(KeyEvent {
                code: Key::Down,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.state.scroll_offset = self.component.state.scroll_offset.saturating_add(1);
                Some(Msg::None)
            }
            Event::Keyboard(KeyEvent {
                code: Key::Tab,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::FocusContacts),
            Event::Keyboard(KeyEvent {
                code: Key::BackTab,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::FocusCallControls),
            _ => None,
        }
    }
}
