use commands::command::ResumeTarget;
use common_models::tui_models::{SessionSummary, SessionTranscript};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Paragraph, Row, StatefulWidget, Table, TableState, Widget, Wrap},
};
use std::time::SystemTime;

pub(crate) struct SessionBox;

#[derive(Default)]
pub(crate) enum SessionPickerState {
    #[default]
    Closed,
    Loading,
    Selecting(SessionSelection),
    Resuming,
    Failed(String),
}

pub(crate) struct SessionSelection {
    sessions: Vec<SessionSummary>,
    query: String,
    matches: Vec<usize>,
    table: TableState,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PickerAction {
    Stay,
    Cancel,
    Resume { id: String },
}

impl SessionPickerState {
    pub fn new(target: &ResumeTarget) -> Self {
        match target {
            ResumeTarget::Picker => Self::Loading,
            ResumeTarget::Session { .. } => Self::Resuming,
        }
    }

    pub fn load(&mut self, result: Result<Vec<SessionSummary>, String>) {
        match (&*self, result) {
            (Self::Loading, Ok(sessions)) => {
                *self = Self::Selecting(SessionSelection::new(sessions));
            }
            (Self::Loading, Err(error)) => *self = Self::Failed(error),
            _ => {}
        }
    }

    pub fn resumed(
        &mut self,
        result: Result<SessionTranscript, String>,
    ) -> Option<SessionTranscript> {
        match (&*self, result) {
            (Self::Resuming, Ok(transcript)) => {
                *self = Self::Closed;
                Some(transcript)
            }
            (Self::Resuming, Err(error)) => {
                *self = Self::Failed(error);
                None
            }
            _ => None,
        }
    }

    pub fn paste(&mut self, text: &str) {
        if let Self::Selecting(selection) = self {
            selection
                .query
                .push_str(&text.split_whitespace().collect::<Vec<_>>().join(" "));
            selection.filter();
        }
    }

    pub fn key(&mut self, key: &KeyEvent) -> PickerAction {
        let action = match (&mut *self, key.code) {
            (_, _) if key.kind == KeyEventKind::Release => PickerAction::Stay,
            (Self::Closed | Self::Resuming, _) => PickerAction::Stay,
            (_, KeyCode::Esc) => PickerAction::Cancel,
            (_, KeyCode::Char('c')) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                PickerAction::Cancel
            }
            (Self::Selecting(selection), _) => selection.key(key),
            _ => PickerAction::Stay,
        };
        match action {
            PickerAction::Stay => {}
            PickerAction::Cancel => *self = Self::Closed,
            PickerAction::Resume { .. } => *self = Self::Resuming,
        }
        action
    }
}

impl SessionSelection {
    fn new(sessions: Vec<SessionSummary>) -> Self {
        let matches = (0..sessions.len()).collect();
        let table = TableState::default().with_selected(sessions.first().map(|_| 0));
        Self {
            sessions,
            query: String::new(),
            matches,
            table,
        }
    }

    fn filter(&mut self) {
        let query = self.query.to_lowercase();
        self.matches = self
            .sessions
            .iter()
            .enumerate()
            .filter_map(|(index, session)| {
                let searchable = format!("{} {}", session.title, session.id).to_lowercase();
                query
                    .split_whitespace()
                    .all(|word| searchable.contains(word))
                    .then_some(index)
            })
            .collect();
        self.table = TableState::default().with_selected((!self.matches.is_empty()).then_some(0));
    }

    fn selected(&self) -> Option<&SessionSummary> {
        self.table
            .selected()
            .and_then(|selected| self.matches.get(selected))
            .and_then(|index| self.sessions.get(*index))
    }

    fn move_selection(&mut self, amount: isize) {
        let selected = self.matches.len().checked_sub(1).map(|last| {
            self.table
                .selected()
                .unwrap_or(0)
                .saturating_add_signed(amount)
                .min(last)
        });
        self.table.select(selected);
    }

    fn key(&mut self, key: &KeyEvent) -> PickerAction {
        match key.code {
            KeyCode::Enter => self
                .selected()
                .map(|session| PickerAction::Resume {
                    id: session.id.clone(),
                })
                .unwrap_or(PickerAction::Stay),
            KeyCode::Up => {
                self.move_selection(-1);
                PickerAction::Stay
            }
            KeyCode::Down => {
                self.move_selection(1);
                PickerAction::Stay
            }
            KeyCode::PageUp => {
                self.move_selection(-10);
                PickerAction::Stay
            }
            KeyCode::PageDown => {
                self.move_selection(10);
                PickerAction::Stay
            }
            KeyCode::Home => {
                self.table.select((!self.matches.is_empty()).then_some(0));
                PickerAction::Stay
            }
            KeyCode::End => {
                self.table.select(self.matches.len().checked_sub(1));
                PickerAction::Stay
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_selection(-1);
                PickerAction::Stay
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_selection(1);
                PickerAction::Stay
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.clear();
                self.filter();
                PickerAction::Stay
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.filter();
                PickerAction::Stay
            }
            KeyCode::Char(character)
                if !character.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.query.push(character);
                self.filter();
                PickerAction::Stay
            }
            _ => PickerAction::Stay,
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let [search, list, preview] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(2),
            Constraint::Length(4),
        ])
        .areas(area);
        Paragraph::new(format!("Search: {}▏", self.query)).render(search, buf);
        match (self.sessions.as_slice(), self.matches.as_slice()) {
            ([], _) => Paragraph::new("No saved conversations in this project.")
                .wrap(Wrap { trim: false })
                .render(list, buf),
            (_, []) => Paragraph::new("No matching conversations.")
                .wrap(Wrap { trim: false })
                .render(list, buf),
            (sessions, matches) => {
                let now = SystemTime::now();
                let rows = matches.iter().map(|index| {
                    let session = &sessions[*index];
                    Row::new(vec![
                        updated_label(session.updated_at, now),
                        format!("{:?}", session.status),
                        session.title.clone(),
                    ])
                });
                let table = Table::new(
                    rows,
                    [
                        Constraint::Length(12),
                        Constraint::Length(14),
                        Constraint::Min(10),
                    ],
                )
                .header(
                    Row::new(["Updated", "Status", "Conversation"])
                        .style(Style::default().add_modifier(Modifier::BOLD)),
                )
                .row_highlight_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("> ");
                StatefulWidget::render(table, list, buf, &mut self.table);
            }
        }
        if let Some(session) = self.selected() {
            Paragraph::new(
                std::iter::once(
                    Line::from(session.id.as_str()).style(Style::default().fg(Color::DarkGray)),
                )
                .chain(session.preview.lines().map(Line::from))
                .collect::<Vec<_>>(),
            )
            .wrap(Wrap { trim: false })
            .render(preview, buf);
        }
    }
}

impl StatefulWidget for SessionBox {
    type State = SessionPickerState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let block = Block::bordered()
            .title("Resume a session")
            .title_bottom(" ↑/↓ select · Enter resume · Esc cancel ");
        let inner = block.inner(area);
        block.render(area, buf);
        match state {
            SessionPickerState::Closed => {}
            SessionPickerState::Loading => {
                Paragraph::new("Loading saved sessions…").render(inner, buf)
            }
            SessionPickerState::Resuming => {
                Paragraph::new("Resuming conversation…").render(inner, buf)
            }
            SessionPickerState::Failed(error) => Paragraph::new(error.as_str())
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: false })
                .render(inner, buf),
            SessionPickerState::Selecting(selection) => selection.render(inner, buf),
        }
    }
}

fn updated_label(updated: Option<SystemTime>, now: SystemTime) -> String {
    match updated.map(|time| now.duration_since(time).unwrap_or_default().as_secs()) {
        None => "Unknown".into(),
        Some(0..60) => "Just now".into(),
        Some(seconds @ 60..3600) => format!("{}m ago", seconds / 60),
        Some(seconds @ 3600..86400) => format!("{}h ago", seconds / 3600),
        Some(seconds) => format!("{}d ago", seconds / 86400),
    }
}

#[cfg(test)]
mod tests;
