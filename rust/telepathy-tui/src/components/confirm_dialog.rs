use tuirealm::event::{Key, KeyEvent, KeyModifiers};
use tuirealm::ratatui::layout::{Constraint, Layout, Rect};
use tuirealm::ratatui::widgets::{Block, Borders, Clear, Paragraph};
use tuirealm::{AttrValue, Attribute, Component, Event, Frame, MockComponent, Props, State};

use crate::components::{ConfirmDialogData, payload_content};
use crate::events::{CoreEvent, Msg};

#[derive(Default)]
struct ConfirmDialogInner {
    props: Props,
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let [_, middle, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .areas(area);
    let [_, center, _] = Layout::horizontal([
        Constraint::Fill((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Fill((100 - percent_x) / 2),
    ])
    .areas(middle);
    center
}

impl MockComponent for ConfirmDialogInner {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let data = payload_content::<ConfirmDialogData>(&self.props).unwrap_or(ConfirmDialogData {
            message: String::new(),
            confirm_msg: Msg::None,
        });
        let rect = centered_rect(50, 6, area);
        frame.render_widget(Clear, rect);
        let text = format!("  {}\n  [y] Yes  [n] No", data.message);
        frame.render_widget(
            Paragraph::new(text).block(Block::default().title("Confirm").borders(Borders::ALL)),
            rect,
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
pub struct ConfirmDialog {
    component: ConfirmDialogInner,
}

impl MockComponent for ConfirmDialog {
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

impl Component<Msg, CoreEvent> for ConfirmDialog {
    fn on(&mut self, ev: Event<CoreEvent>) -> Option<Msg> {
        let data = payload_content::<ConfirmDialogData>(&self.component.props)?;
        match ev {
            Event::Keyboard(KeyEvent {
                code: Key::Char('y'),
                modifiers: KeyModifiers::NONE,
            }) => Some(data.confirm_msg),
            Event::Keyboard(KeyEvent {
                code: Key::Char('n'),
                modifiers: KeyModifiers::NONE,
            })
            | Event::Keyboard(KeyEvent {
                code: Key::Esc,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::None),
            _ => None,
        }
    }
}
