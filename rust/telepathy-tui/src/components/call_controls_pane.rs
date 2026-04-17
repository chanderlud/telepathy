use std::time::Instant;

use tuirealm::event::{Key, KeyEvent, KeyModifiers};
use tuirealm::ratatui::layout::Rect;
use tuirealm::ratatui::style::{Modifier, Style};
use tuirealm::ratatui::widgets::{Block, Borders, Paragraph};
use tuirealm::{AttrValue, Attribute, Component, Event, Frame, MockComponent, Props, State};

use crate::components::{CallControlsData, payload_content};
use crate::events::{CoreEvent, Msg, VolumeKind};

#[derive(Debug, Clone, Copy)]
enum FocusedControl {
    Output,
    Input,
    Sensitivity,
    Sound,
}

#[derive(Default)]
struct OwnStates {
    call_start: Option<Instant>,
    output_vol: f32,
    input_vol: f32,
    sound_vol: f32,
    sensitivity: f32,
    focused: usize,
}

#[derive(Default)]
struct CallControlsPaneInner {
    props: Props,
    state: OwnStates,
}

impl CallControlsPaneInner {
    fn focused_kind(&self) -> FocusedControl {
        match self.state.focused {
            0 => FocusedControl::Output,
            1 => FocusedControl::Input,
            2 => FocusedControl::Sensitivity,
            _ => FocusedControl::Sound,
        }
    }

    fn focused_volume_kind(&self) -> VolumeKind {
        match self.focused_kind() {
            FocusedControl::Output => VolumeKind::Output,
            FocusedControl::Input => VolumeKind::Input,
            FocusedControl::Sensitivity => VolumeKind::InputSensitivity,
            FocusedControl::Sound => VolumeKind::Sound,
        }
    }

    fn focused_value(&self) -> f32 {
        match self.focused_kind() {
            FocusedControl::Output => self.state.output_vol,
            FocusedControl::Input => self.state.input_vol,
            FocusedControl::Sensitivity => self.state.sensitivity,
            FocusedControl::Sound => self.state.sound_vol,
        }
    }

    fn to_bar(value: f32) -> String {
        let normalized = ((value + 60.0) / 60.0).clamp(0.0, 1.0);
        let filled = (normalized * 8.0).round() as usize;
        let empty = 8usize.saturating_sub(filled);
        format!("{}{}", "█".repeat(filled), "░".repeat(empty))
    }
}

impl MockComponent for CallControlsPaneInner {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let data = payload_content::<CallControlsData>(&self.props).unwrap_or(CallControlsData {
            muted: false,
            deafened: false,
            call_active: false,
            manager_active: false,
            manager_restartable: false,
            output_vol: 0.0,
            input_vol: 0.0,
            sound_vol: 0.0,
            sensitivity: 0.0,
        });
        self.state.output_vol = data.output_vol;
        self.state.input_vol = data.input_vol;
        self.state.sound_vol = data.sound_vol;
        self.state.sensitivity = data.sensitivity;

        if data.call_active {
            if self.state.call_start.is_none() {
                self.state.call_start = Some(Instant::now());
            }
        } else {
            self.state.call_start = None;
        }

        let duration = self
            .state
            .call_start
            .map(|start| Instant::now().saturating_duration_since(start))
            .map(|elapsed| {
                let secs = elapsed.as_secs();
                format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
            })
            .unwrap_or_else(|| "00:00:00".to_string());

        let mut lines = Vec::new();
        lines.push(format!(
            "[m] Mute: {}   [d] Deafen: {}",
            if data.muted { "ON" } else { "OFF" },
            if data.deafened { "ON" } else { "OFF" }
        ));
        lines.push(format!(
            "Output Vol: {} {:>6.1} dB  [←/→] [Shift+←/→]",
            Self::to_bar(self.state.output_vol),
            self.state.output_vol
        ));
        lines.push(format!(
            "Input Vol:  {} {:>6.1} dB",
            Self::to_bar(self.state.input_vol),
            self.state.input_vol
        ));
        lines.push(format!(
            "Sensitivity:{} {:>6.1} dB",
            Self::to_bar(self.state.sensitivity),
            self.state.sensitivity
        ));
        lines.push(format!(
            "Sound Vol:  {} {:>6.1} dB",
            Self::to_bar(self.state.sound_vol),
            self.state.sound_vol
        ));
        lines.push(format!(
            "Manager: {}  [r] Restart",
            if data.manager_active {
                "Active"
            } else {
                "Inactive"
            }
        ));
        lines.push(format!("Duration: {duration}"));
        lines.push("[t] Audio Test".to_string());

        let body = lines.join("\n");
        let mut style = Style::default();
        if !data.call_active {
            style = style.add_modifier(Modifier::DIM);
        }
        frame.render_widget(
            Paragraph::new(body)
                .block(Block::default().title("Call Controls").borders(Borders::ALL))
                .style(style),
            area,
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
pub struct CallControlsPane {
    component: CallControlsPaneInner,
}

impl MockComponent for CallControlsPane {
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

impl Component<Msg, CoreEvent> for CallControlsPane {
    fn on(&mut self, ev: Event<CoreEvent>) -> Option<Msg> {
        match ev {
            Event::Tick => Some(Msg::None),
            Event::Keyboard(KeyEvent {
                code: Key::Up,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.state.focused = self.component.state.focused.saturating_sub(1);
                Some(Msg::None)
            }
            Event::Keyboard(KeyEvent {
                code: Key::Down,
                modifiers: KeyModifiers::NONE,
            }) => {
                if self.component.state.focused < 3 {
                    self.component.state.focused += 1;
                }
                Some(Msg::None)
            }
            Event::Keyboard(KeyEvent {
                code: Key::Char('m'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ToggleMute),
            Event::Keyboard(KeyEvent {
                code: Key::Char('d'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ToggleDeafen),
            Event::Keyboard(KeyEvent {
                code: Key::Left,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::VolumeChanged(
                self.component.focused_volume_kind(),
                self.component.focused_value() - 1.0,
            )),
            Event::Keyboard(KeyEvent {
                code: Key::Right,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::VolumeChanged(
                self.component.focused_volume_kind(),
                self.component.focused_value() + 1.0,
            )),
            Event::Keyboard(KeyEvent {
                code: Key::Left,
                modifiers: KeyModifiers::SHIFT,
            }) => Some(Msg::VolumeChanged(
                self.component.focused_volume_kind(),
                self.component.focused_value() - 0.1,
            )),
            Event::Keyboard(KeyEvent {
                code: Key::Right,
                modifiers: KeyModifiers::SHIFT,
            }) => Some(Msg::VolumeChanged(
                self.component.focused_volume_kind(),
                self.component.focused_value() + 0.1,
            )),
            Event::Keyboard(KeyEvent {
                code: Key::Char('r'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::RestartManager),
            Event::Keyboard(KeyEvent {
                code: Key::Char('t'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::AudioTestToggle),
            Event::Keyboard(KeyEvent {
                code: Key::Tab,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::FocusChat),
            Event::Keyboard(KeyEvent {
                code: Key::BackTab,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::FocusContacts),
            _ => None,
        }
    }
}
