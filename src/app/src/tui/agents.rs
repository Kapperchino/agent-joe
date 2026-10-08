use crate::theme;
use crate::widgets::message_box::message_box::{MessageBoxState, Msg, ToolDisplay};
use common_models::tui_models::{ActorToTuiPacket, AgentProgress, Lifecycle, State};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use utils::utils::FnvHashMap;

pub(super) struct AgentThreads {
    threads: FnvHashMap<u64, AgentThread>,
    view: AgentView,
    tool_display: ToolDisplay,
    width: u16,
    height: u16,
}

#[derive(Default, Clone, Copy)]
enum AgentView {
    #[default]
    Main,
    Picker {
        selected: usize,
    },
    Thread(u64),
}

#[derive(Clone, Copy)]
enum Attention {
    Read,
    Unread,
}

struct AgentThread {
    worker_id: String,
    objective: String,
    state: Lifecycle,
    attention: Attention,
    messages: MessageBoxState,
    offset: usize,
}

impl AgentThread {
    fn new(tool_display: ToolDisplay, width: u16, height: u16) -> Self {
        let mut messages = MessageBoxState::with_tool_display(tool_display);
        messages.update_width_height(width, height);
        Self {
            worker_id: String::new(),
            objective: "Delegated task".into(),
            state: Lifecycle::Running,
            attention: Attention::Unread,
            messages,
            offset: 0,
        }
    }

    fn lifecycle(&mut self, state: Lifecycle, detail: Option<String>) {
        self.state = state;
        if state.terminal() {
            self.messages.finish_stream_message(true);
            self.messages.actor_state = State::Stopped;
        }
        if let Some(detail) = detail {
            self.messages.append(Msg::Message(detail));
        }
    }

    fn packet(&mut self, packet: ActorToTuiPacket) {
        match packet {
            ActorToTuiPacket::AgentUpdated(AgentProgress {
                worker_id,
                objective,
                state,
                detail,
            }) => {
                self.worker_id = worker_id;
                self.objective = objective;
                self.lifecycle(state, detail);
            }
            ActorToTuiPacket::TurnChanged { state, detail, .. } => self.lifecycle(state, detail),
            ActorToTuiPacket::StateChanged(state) => {
                match state {
                    State::ThinkingStart => self.messages.start_stream_message(false),
                    State::MessageStart => self.messages.start_stream_message(true),
                    State::ThinkingStop => self.messages.finish_stream_message(false),
                    State::MessageStop | State::Stopped => {
                        self.messages.finish_stream_message(true)
                    }
                    _ => {}
                }
                self.messages.actor_state = state;
            }
            ActorToTuiPacket::Data(data) => {
                if matches!(
                    self.messages.actor_state,
                    State::ThinkingStart | State::MessageStart
                ) {
                    self.messages.push_stream_message(&data);
                }
            }
            ActorToTuiPacket::ToolUse(lines) => {
                lines
                    .into_iter()
                    .for_each(|line| self.messages.append(Msg::Tool(line)));
            }
            ActorToTuiPacket::ContextNotice(text)
            | ActorToTuiPacket::SessionError(text)
            | ActorToTuiPacket::CommandResult(_, text) => self.messages.append(Msg::Message(text)),
            ActorToTuiPacket::OperationChanged {
                state: Lifecycle::Failed,
                detail,
                ..
            } => {
                self.messages.append(Msg::Message(detail));
            }
            ActorToTuiPacket::ValidationUpdated(validation) => {
                self.messages.append(Msg::Message(format!(
                    "Validation · {} {:?}",
                    validation.operation, validation.state
                )))
            }
            _ => {}
        }
    }
}

impl AgentThreads {
    pub(super) fn new(tool_display: ToolDisplay) -> Self {
        Self {
            threads: FnvHashMap::default(),
            view: AgentView::Main,
            tool_display,
            width: 80,
            height: 20,
        }
    }

    pub(super) fn is_main(&self) -> bool {
        matches!(self.view, AgentView::Main)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }

    pub(super) fn active_count(&self) -> usize {
        self.threads
            .values()
            .filter(|thread| !thread.state.terminal())
            .count()
    }

    #[cfg(test)]
    pub(super) fn state(&self, actor_id: u64) -> Option<Lifecycle> {
        self.threads.get(&actor_id).map(|thread| thread.state)
    }

    pub(super) fn clear(&mut self) {
        self.threads.clear();
        self.show_main();
    }

    pub(super) fn show_main(&mut self) {
        self.view = AgentView::Main;
    }

    pub(super) fn open(&mut self) {
        let selected = match self.view {
            AgentView::Thread(id) => self.threads.keys().filter(|key| **key < id).count() + 1,
            AgentView::Picker { selected } => selected,
            AgentView::Main => 0,
        };
        self.view = AgentView::Picker { selected };
    }

    pub(super) fn handle(&mut self, actor_id: u64, packet: ActorToTuiPacket) {
        match packet {
            ActorToTuiPacket::InteractionUpdated(_)
            | ActorToTuiPacket::SessionChanged
            | ActorToTuiPacket::SessionChoices(_)
            | ActorToTuiPacket::SessionResumed(_)
            | ActorToTuiPacket::InputAccepted { .. }
            | ActorToTuiPacket::ContextUpdated(_)
            | ActorToTuiPacket::TokensUpdated(_)
            | ActorToTuiPacket::Queued { .. } => {}
            packet => {
                let thread = self.threads.entry(actor_id).or_insert_with(|| {
                    AgentThread::new(self.tool_display, self.width, self.height)
                });
                thread.attention = match self.view {
                    AgentView::Thread(id) if id == actor_id => Attention::Read,
                    _ => Attention::Unread,
                };
                thread.packet(packet);
            }
        }
    }

    pub(super) fn advance(&mut self) {
        let thread = match self.view {
            AgentView::Thread(id) => self.threads.get_mut(&id),
            AgentView::Main | AgentView::Picker { .. } => None,
        };
        if let Some(thread) = thread {
            thread.messages.advance_busy_indicator();
        }
    }

    pub(super) fn animation_deadline(&self) -> Option<tokio::time::Instant> {
        match self.view {
            AgentView::Thread(id) => self
                .threads
                .get(&id)
                .and_then(|thread| thread.messages.animation_deadline()),
            AgentView::Main | AgentView::Picker { .. } => None,
        }
    }

    pub(super) fn key(&mut self, key: &KeyEvent) {
        match (self.view, key.code) {
            (_, KeyCode::Char('g')) if key.modifiers.contains(KeyModifiers::CONTROL) => self.open(),
            (AgentView::Thread(id), _) if self.thread_tool_key(id, key) => {}
            (_, KeyCode::Esc | KeyCode::Char('q')) => self.show_main(),
            (AgentView::Picker { selected }, KeyCode::Up | KeyCode::Char('k')) => {
                let count = self.threads.len() + 1;
                self.view = AgentView::Picker {
                    selected: (selected + count - 1) % count,
                };
            }
            (AgentView::Picker { selected }, KeyCode::Down | KeyCode::Char('j')) => {
                self.view = AgentView::Picker {
                    selected: (selected + 1) % (self.threads.len() + 1),
                };
            }
            (AgentView::Picker { selected: 0 }, KeyCode::Enter) => self.show_main(),
            (AgentView::Picker { selected }, KeyCode::Enter) => {
                let mut ids = self.threads.keys().copied().collect::<Vec<_>>();
                ids.sort();
                let id = ids.get(selected - 1).copied();
                if let Some(id) = id {
                    self.view = AgentView::Thread(id);
                    self.threads
                        .entry(id)
                        .and_modify(|thread| thread.attention = Attention::Read);
                }
            }
            (AgentView::Thread(id), code) => {
                if let Some(thread) = self.threads.get_mut(&id) {
                    let capacity = usize::from(self.height).max(1);
                    let max_offset = thread
                        .messages
                        .history_line_count()
                        .saturating_sub(capacity);
                    thread.offset = match code {
                        KeyCode::Up | KeyCode::Char('k') => thread.offset.saturating_add(1),
                        KeyCode::Down | KeyCode::Char('j') => thread.offset.saturating_sub(1),
                        KeyCode::PageUp => thread.offset.saturating_add(capacity),
                        KeyCode::PageDown => thread.offset.saturating_sub(capacity),
                        KeyCode::Home => max_offset,
                        KeyCode::End => 0,
                        _ => thread.offset,
                    }
                    .min(max_offset);
                }
            }
            _ => {}
        }
    }

    fn thread_tool_key(&mut self, id: u64, key: &KeyEvent) -> bool {
        self.threads
            .get_mut(&id)
            .is_some_and(|thread| thread.messages.handle_tool_history_key(key))
    }

    pub(super) fn hints(&self) -> Vec<theme::KeyHint> {
        match self.view {
            AgentView::Main => Vec::new(),
            AgentView::Picker { .. } => vec![
                theme::KeyHint::new("Esc", "main"),
                theme::KeyHint::new("↑/↓", "select"),
                theme::KeyHint::new("Enter", "inspect"),
            ],
            AgentView::Thread(id) => {
                let thread = self.threads.get(&id);
                let mut hints = vec![
                    match thread.is_some_and(|thread| thread.messages.tool_history_expanded()) {
                        true => theme::KeyHint::new("Ctrl+o/Esc", "collapse"),
                        false => theme::KeyHint::new("Esc", "main"),
                    },
                    theme::KeyHint::new("Ctrl+g", "agents"),
                    theme::KeyHint::new("↑/↓", "scroll"),
                    theme::KeyHint::new("PgUp/PgDn", "page"),
                    theme::KeyHint::new("Home/End", "first/last"),
                ];
                if thread.is_some_and(|thread| thread.messages.has_tool_history()) {
                    hints.insert(2, theme::KeyHint::new("Ctrl+o", "tools"));
                }
                hints
            }
        }
    }

    pub(super) fn summary(&self) -> Line<'static> {
        let unread = self
            .threads
            .values()
            .filter(|thread| matches!(thread.attention, Attention::Unread))
            .count();
        Line::from(vec![
            theme::badge("AGENTS", theme::ACCENT),
            theme::muted(format!(
                "  {} running · {} finished · {unread} unread",
                self.active_count(),
                self.threads.len() - self.active_count()
            )),
            Span::styled("  Ctrl+g / /agent", Style::default().fg(theme::ACCENT)),
        ])
    }

    pub(super) fn draw(&mut self, frame: &mut Frame, area: Rect) {
        self.width = area.width;
        self.height = area.height.saturating_sub(3);
        self.threads
            .values_mut()
            .for_each(|thread| thread.messages.update_width_height(self.width, self.height));
        match self.view {
            AgentView::Main => {}
            AgentView::Picker { selected } => {
                let panel = theme::panel("Agent threads · select to inspect", theme::ACCENT);
                let inner = panel.inner(area);
                frame.render_widget(panel, area);
                let mut threads = self.threads.iter().collect::<Vec<_>>();
                threads.sort_by_key(|(id, _)| **id);
                let rows = std::iter::once((
                    "Main conversation".to_owned(),
                    "Your messages and the main agent's response".to_owned(),
                ))
                .chain(threads.into_iter().map(|(id, thread)| {
                    let unread = match thread.attention {
                        Attention::Read => "",
                        Attention::Unread => " · unread",
                    };
                    (
                        format!("Agent {id} · {:?}{unread}", thread.state),
                        thread.objective.clone(),
                    )
                }));
                let capacity = usize::from(inner.height / 2).max(1);
                let start = selected.saturating_sub(capacity - 1);
                let lines = rows
                    .enumerate()
                    .skip(start)
                    .take(capacity)
                    .flat_map(|(index, (label, objective))| {
                        let style = match index == selected {
                            true => theme::base().bg(theme::SELECTION).fg(theme::ACCENT),
                            false => theme::base(),
                        };
                        let marker = match index == selected {
                            true => "›",
                            false => " ",
                        };
                        [
                            Line::styled(format!("{marker} {label}"), style),
                            Line::from(theme::muted(format!(
                                "  {}",
                                objective.split_whitespace().collect::<Vec<_>>().join(" ")
                            ))),
                        ]
                    })
                    .collect::<Vec<_>>();
                frame.render_widget(Paragraph::new(lines), inner);
            }
            AgentView::Thread(id) => {
                if let Some(thread) = self.threads.get_mut(&id) {
                    let [header, transcript] =
                        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);
                    let color = match thread.state {
                        Lifecycle::Failed => theme::RED,
                        Lifecycle::Completed => theme::GREEN,
                        _ => theme::AMBER,
                    };
                    let heading = vec![
                        Line::from(vec![
                            theme::badge(format!("AGENT {id}"), color),
                            theme::muted(format!("  {:?}", thread.state)),
                        ]),
                        Line::from(theme::muted(
                            thread
                                .objective
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" "),
                        )),
                        Line::from(theme::muted(format!("worker {}", thread.worker_id))),
                    ];
                    frame.render_widget(Paragraph::new(heading), header);
                    match (
                        thread.messages.history_line_count(),
                        thread.state.terminal(),
                    ) {
                        (0, terminal) => {
                            let text = match terminal {
                                true => "No output was recorded for this agent.",
                                false => "Waiting for agent output…",
                            };
                            frame.render_widget(Paragraph::new(theme::muted(text)), transcript);
                        }
                        _ => thread.messages.render_history(
                            transcript,
                            frame.buffer_mut(),
                            thread.offset,
                        ),
                    }
                }
            }
        }
    }
}
