use tuirealm::event::{Key, KeyEvent, KeyModifiers};
use tuirealm::props::Color;
use tuirealm::ratatui::layout::{Constraint, Layout, Rect};
use tuirealm::ratatui::style::{Modifier, Style};
use tuirealm::ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use tuirealm::{AttrValue, Attribute, Component, Event, Frame, MockComponent, Props, State};

use crate::components::{ContactsPaneData, SessionBadge, payload_content};
use crate::events::{CoreEvent, Msg};

#[derive(Debug, Clone)]
enum InlineInputMode {
    Add,
    Rename(String),
}

#[derive(Debug, Clone)]
enum RowItem {
    Header,
    Divider,
    Contact {
        id: String,
        nickname: String,
        badge: &'static str,
    },
    Room(String),
}

#[derive(Default)]
struct OwnStates {
    selected: usize,
    inline_input: Option<InlineInputMode>,
    inline_buffer: String,
}

#[derive(Default)]
struct ContactsPaneInner {
    props: Props,
    state: OwnStates,
}

impl ContactsPaneInner {
    fn rows_from_data(data: &ContactsPaneData) -> Vec<RowItem> {
        let mut rows = Vec::new();
        rows.push(RowItem::Header);
        for contact in &data.contacts {
            let badge = match data.sessions.get(&contact.id).unwrap_or(&SessionBadge::Inactive) {
                SessionBadge::Connecting => "●",
                SessionBadge::ConnectedDirect => "◉",
                SessionBadge::ConnectedRelayed => "◎",
                SessionBadge::Inactive => "○",
            };
            rows.push(RowItem::Contact {
                id: contact.id.clone(),
                nickname: contact.nickname.clone(),
                badge,
            });
        }
        rows.push(RowItem::Divider);
        rows.push(RowItem::Header);
        for room in &data.rooms {
            rows.push(RowItem::Room(room.clone()));
        }
        rows
    }

    fn selected_row(rows: &[RowItem], selected: usize) -> Option<&RowItem> {
        rows.get(selected)
    }
}

impl MockComponent for ContactsPaneInner {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let data = payload_content::<ContactsPaneData>(&self.props).unwrap_or(ContactsPaneData {
            contacts: Vec::new(),
            rooms: Vec::new(),
            sessions: std::collections::HashMap::new(),
            call_active: false,
        });
        let rows = Self::rows_from_data(&data);
        if self.state.selected >= rows.len() {
            self.state.selected = rows.len().saturating_sub(1);
        }

        let focused = self
            .props
            .get_or(Attribute::Focus, AttrValue::Flag(false))
            .unwrap_flag();
        let border_color = if focused { Color::Cyan } else { Color::Gray };
        let block = Block::default()
            .title("Contacts")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let [list_area, input_area] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
            .areas(area);

        let mut items = Vec::with_capacity(rows.len());
        for row in &rows {
            let line = match row {
                RowItem::Header => {
                    if items.is_empty() {
                        "Contacts".to_string()
                    } else {
                        "Rooms".to_string()
                    }
                }
                RowItem::Divider => "──────".to_string(),
                RowItem::Contact { nickname, badge, .. } => format!("{badge} {nickname}"),
                RowItem::Room(name) => format!("# {name}"),
            };
            items.push(ListItem::new(line));
        }

        let mut list_state = ListState::default();
        if !rows.is_empty() {
            list_state.select(Some(self.state.selected));
        }
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_stateful_widget(list, list_area, &mut list_state);

        if self.state.inline_input.is_some() {
            frame.render_widget(Paragraph::new(self.state.inline_buffer.as_str()), input_area);
        }
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
pub struct ContactsPane {
    component: ContactsPaneInner,
}

impl MockComponent for ContactsPane {
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

impl Component<Msg, CoreEvent> for ContactsPane {
    fn on(&mut self, ev: Event<CoreEvent>) -> Option<Msg> {
        let data = payload_content::<ContactsPaneData>(&self.component.props).unwrap_or(ContactsPaneData {
            contacts: Vec::new(),
            rooms: Vec::new(),
            sessions: std::collections::HashMap::new(),
            call_active: false,
        });
        let rows = ContactsPaneInner::rows_from_data(&data);

        if let Some(mode) = self.component.state.inline_input.clone() {
            return match ev {
                Event::Keyboard(KeyEvent {
                    code: Key::Esc,
                    modifiers: KeyModifiers::NONE,
                }) => {
                    self.component.state.inline_input = None;
                    self.component.state.inline_buffer.clear();
                    Some(Msg::None)
                }
                Event::Keyboard(KeyEvent {
                    code: Key::Backspace,
                    modifiers: KeyModifiers::NONE,
                }) => {
                    self.component.state.inline_buffer.pop();
                    Some(Msg::None)
                }
                Event::Keyboard(KeyEvent {
                    code: Key::Char(ch),
                    modifiers: KeyModifiers::NONE,
                }) => {
                    self.component.state.inline_buffer.push(ch);
                    Some(Msg::None)
                }
                Event::Keyboard(KeyEvent {
                    code: Key::Enter,
                    modifiers: KeyModifiers::NONE,
                }) => {
                    let input = self.component.state.inline_buffer.trim().to_string();
                    self.component.state.inline_input = None;
                    self.component.state.inline_buffer.clear();
                    if input.is_empty() {
                        return Some(Msg::None);
                    }
                    match mode {
                        InlineInputMode::Add => Some(Msg::ContactAdd(input, String::new())),
                        InlineInputMode::Rename(contact_id) => {
                            Some(Msg::ContactRename(contact_id, input))
                        }
                    }
                }
                _ => None,
            };
        }

        match ev {
            Event::Keyboard(KeyEvent {
                code: Key::Up,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.state.selected = self.component.state.selected.saturating_sub(1);
                Some(Msg::None)
            }
            Event::Keyboard(KeyEvent {
                code: Key::Down,
                modifiers: KeyModifiers::NONE,
            }) => {
                if self.component.state.selected + 1 < rows.len() {
                    self.component.state.selected += 1;
                }
                Some(Msg::None)
            }
            Event::Keyboard(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::NONE,
            }) => match ContactsPaneInner::selected_row(&rows, self.component.state.selected) {
                Some(RowItem::Contact { id, .. }) => Some(Msg::ContactSelected(id.clone())),
                Some(RowItem::Room(name)) => Some(Msg::RoomSelected(name.clone())),
                _ => Some(Msg::None),
            },
            Event::Keyboard(KeyEvent {
                code: Key::Char('c'),
                modifiers: KeyModifiers::NONE,
            }) => match ContactsPaneInner::selected_row(&rows, self.component.state.selected) {
                Some(RowItem::Contact { .. }) => {
                    if data.call_active {
                        Some(Msg::EndCall)
                    } else {
                        Some(Msg::StartCall)
                    }
                }
                _ => Some(Msg::None),
            },
            Event::Keyboard(KeyEvent {
                code: Key::Char('j'),
                modifiers: KeyModifiers::NONE,
            }) => match ContactsPaneInner::selected_row(&rows, self.component.state.selected) {
                Some(RowItem::Room(name)) => Some(Msg::RoomJoin(name.clone())),
                _ => Some(Msg::None),
            },
            Event::Keyboard(KeyEvent {
                code: Key::Char('a'),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.state.inline_input = Some(InlineInputMode::Add);
                self.component.state.inline_buffer.clear();
                Some(Msg::None)
            }
            Event::Keyboard(KeyEvent {
                code: Key::Char('r'),
                modifiers: KeyModifiers::NONE,
            }) => match ContactsPaneInner::selected_row(&rows, self.component.state.selected) {
                Some(RowItem::Contact { id, nickname, .. }) => {
                    self.component.state.inline_input = Some(InlineInputMode::Rename(id.clone()));
                    self.component.state.inline_buffer = nickname.clone();
                    Some(Msg::None)
                }
                _ => Some(Msg::None),
            },
            Event::Keyboard(KeyEvent {
                code: Key::Char('d'),
                modifiers: KeyModifiers::NONE,
            }) => match ContactsPaneInner::selected_row(&rows, self.component.state.selected) {
                Some(RowItem::Contact { id, .. }) => Some(Msg::ContactDelete(id.clone())),
                Some(RowItem::Room(name)) => Some(Msg::RoomDelete(name.clone())),
                _ => Some(Msg::None),
            },
            Event::Keyboard(KeyEvent {
                code: Key::Tab,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::FocusCallControls),
            _ => None,
        }
    }
}
