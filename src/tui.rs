use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io::{self, Stdout};
use std::time::Duration;
use tokio::sync::mpsc;

/// Messages from the actor to the TUI for display
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// Streaming text content
    TextDelta(String),
    /// Complete text block finished
    TextComplete(String),
    /// Thinking block started
    ThinkingStart,
    /// Thinking content delta
    ThinkingDelta(String),
    /// Thinking complete
    ThinkingComplete(String),
    /// Tool use started
    ToolStart { name: String, id: String },
    /// Tool result received
    ToolResult { name: String, result: String },
    /// Processing complete
    Done,
    /// Error occurred
    Error(String),
}

/// A message block in the conversation
#[derive(Debug, Clone)]
pub enum MessageBlock {
    User(String),
    Assistant(String),
    Thinking(String),
    ToolUse { name: String, result: Option<String> },
}

/// Application state for the TUI
pub struct App {
    /// Current input buffer
    input: String,
    /// Cursor position in input
    cursor_pos: usize,
    /// Conversation history
    messages: Vec<MessageBlock>,
    /// Current streaming buffer
    current_stream: Option<String>,
    /// Current thinking buffer
    current_thinking: Option<String>,
    /// Is processing a request
    is_processing: bool,
    /// Scroll offset for messages
    scroll_offset: u16,
    /// Sender to actor
    actor_tx: mpsc::UnboundedSender<String>,
    /// Receiver from actor
    ui_rx: mpsc::UnboundedReceiver<UiEvent>,
    /// Should quit
    should_quit: bool,
}

impl App {
    pub fn new(
        actor_tx: mpsc::UnboundedSender<String>,
        ui_rx: mpsc::UnboundedReceiver<UiEvent>,
    ) -> Self {
        Self {
            input: String::new(),
            cursor_pos: 0,
            messages: Vec::new(),
            current_stream: None,
            current_thinking: None,
            is_processing: false,
            scroll_offset: 0,
            actor_tx,
            ui_rx,
            should_quit: false,
        }
    }

    /// Handle a UI event from the actor
    pub fn handle_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::TextDelta(text) => {
                if let Some(ref mut stream) = self.current_stream {
                    stream.push_str(&text);
                } else {
                    self.current_stream = Some(text);
                }
            }
            UiEvent::TextComplete(text) => {
                self.messages.push(MessageBlock::Assistant(text));
                self.current_stream = None;
            }
            UiEvent::ThinkingStart => {
                self.current_thinking = Some(String::new());
            }
            UiEvent::ThinkingDelta(text) => {
                if let Some(ref mut thinking) = self.current_thinking {
                    thinking.push_str(&text);
                } else {
                    self.current_thinking = Some(text);
                }
            }
            UiEvent::ThinkingComplete(text) => {
                self.messages.push(MessageBlock::Thinking(text));
                self.current_thinking = None;
            }
            UiEvent::ToolStart { name, id: _ } => {
                self.messages.push(MessageBlock::ToolUse {
                    name,
                    result: None,
                });
            }
            UiEvent::ToolResult { name, result } => {
                // Update the last tool use with result
                if let Some(&mut MessageBlock::ToolUse {
                    name: ref tool_name,
                    result: ref mut tool_result,
                }) = self.messages.last_mut()
                {
                    if tool_name == &name {
                        *tool_result = Some(result);
                    }
                }
            }
            UiEvent::Done => {
                // Finalize any pending stream
                if let Some(stream) = self.current_stream.take() {
                    if !stream.is_empty() {
                        self.messages.push(MessageBlock::Assistant(stream));
                    }
                }
                if let Some(thinking) = self.current_thinking.take() {
                    if !thinking.is_empty() {
                        self.messages.push(MessageBlock::Thinking(thinking));
                    }
                }
                self.is_processing = false;
            }
            UiEvent::Error(err) => {
                self.messages
                    .push(MessageBlock::Assistant(format!("⚠ Error: {}", err)));
                self.is_processing = false;
            }
        }
    }

    /// Submit current input to the actor
    pub fn submit(&mut self) {
        if !self.input.is_empty() && !self.is_processing {
            let prompt = self.input.clone();
            self.messages.push(MessageBlock::User(prompt.clone()));
            self.input.clear();
            self.cursor_pos = 0;
            self.is_processing = true;
            let _ = self.actor_tx.send(prompt);
        }
    }

    /// Handle keyboard input
    pub fn handle_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Enter => self.submit(),
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
                }
            }
            KeyCode::Home => self.cursor_pos = 0,
            KeyCode::End => self.cursor_pos = self.input.len(),
            KeyCode::Up => {
                self.scroll_offset = self.scroll_offset.saturating_add(3);
            }
            KeyCode::Down => {
                self.scroll_offset = self.scroll_offset.saturating_sub(3);
            }
            KeyCode::Esc => self.should_quit = true,
            _ => {}
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }
}

pub fn init_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}
pub fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    Ok(())
}

pub fn render(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(10),   // Messages
            Constraint::Length(3), // Input
            Constraint::Length(1), // Status
        ])
        .split(frame.area());

    render_header(frame, chunks[0]);
    render_messages(frame, chunks[1], app);
    render_input(frame, chunks[2], app);
    render_status(frame, chunks[3], app);
}

fn render_header(frame: &mut Frame, area: Rect) {
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            " TURBO",
            Style::default()
                .fg(Color::Rgb(255, 107, 107))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "-CODE ",
            Style::default()
                .fg(Color::Rgb(78, 205, 196))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("│ ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            "AI Coding Assistant",
            Style::default().fg(Color::Rgb(150, 150, 150)),
        ),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(60, 60, 60)))
            .style(Style::default().bg(Color::Rgb(25, 25, 30))),
    );
    frame.render_widget(header, area);
}

fn render_messages(frame: &mut Frame, area: Rect, app: &App) {
    let mut items: Vec<ListItem> = Vec::new();

    // Render existing messages
    for msg in &app.messages {
        let item = match msg {
            MessageBlock::User(text) => {
                let lines = vec![
                    Line::from(Span::styled(
                        "▸ You",
                        Style::default()
                            .fg(Color::Rgb(78, 205, 196))
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(
                        format!("  {}", text),
                        Style::default().fg(Color::White),
                    )),
                    Line::from(""),
                ];
                ListItem::new(lines)
            }
            MessageBlock::Assistant(text) => {
                let mut lines = vec![Line::from(Span::styled(
                    "◆ Assistant",
                    Style::default()
                        .fg(Color::Rgb(255, 107, 107))
                        .add_modifier(Modifier::BOLD),
                ))];
                for line in text.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("  {}", line),
                        Style::default().fg(Color::Rgb(200, 200, 200)),
                    )));
                }
                lines.push(Line::from(""));
                ListItem::new(lines)
            }
            MessageBlock::Thinking(text) => {
                let preview = if text.len() > 100 {
                    format!("{}...", &text[..100])
                } else {
                    text.clone()
                };
                let lines = vec![
                    Line::from(Span::styled(
                        "💭 Thinking",
                        Style::default()
                            .fg(Color::Rgb(150, 120, 200))
                            .add_modifier(Modifier::ITALIC),
                    )),
                    Line::from(Span::styled(
                        format!("  {}", preview),
                        Style::default().fg(Color::Rgb(120, 120, 130)),
                    )),
                    Line::from(""),
                ];
                ListItem::new(lines)
            }
            MessageBlock::ToolUse { name, result } => {
                let mut lines = vec![Line::from(Span::styled(
                    format!("🔧 Tool: {}", name),
                    Style::default()
                        .fg(Color::Rgb(255, 200, 100))
                        .add_modifier(Modifier::BOLD),
                ))];
                if let Some(res) = result {
                    let preview = if res.len() > 200 {
                        format!("{}...", &res[..200])
                    } else {
                        res.clone()
                    };
                    lines.push(Line::from(Span::styled(
                        format!("  Result: {}", preview),
                        Style::default().fg(Color::Rgb(150, 150, 150)),
                    )));
                }
                lines.push(Line::from(""));
                ListItem::new(lines)
            }
        };
        items.push(item);
    }

    // Render current streaming content
    if let Some(ref stream) = app.current_stream {
        let mut lines = vec![Line::from(Span::styled(
            "◆ Assistant",
            Style::default()
                .fg(Color::Rgb(255, 107, 107))
                .add_modifier(Modifier::BOLD),
        ))];
        for line in stream.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {}", line),
                Style::default().fg(Color::Rgb(200, 200, 200)),
            )));
        }
        lines.push(Line::from(Span::styled(
            "▋",
            Style::default().fg(Color::Rgb(255, 107, 107)),
        )));
        items.push(ListItem::new(lines));
    }

    // Render current thinking
    if let Some(ref thinking) = app.current_thinking {
        let preview = if thinking.len() > 150 {
            format!("{}...", &thinking[thinking.len().saturating_sub(150)..])
        } else {
            thinking.clone()
        };
        let lines = vec![
            Line::from(Span::styled(
                "💭 Thinking...",
                Style::default()
                    .fg(Color::Rgb(150, 120, 200))
                    .add_modifier(Modifier::ITALIC),
            )),
            Line::from(Span::styled(
                format!("  {}", preview),
                Style::default().fg(Color::Rgb(120, 120, 130)),
            )),
        ];
        items.push(ListItem::new(lines));
    }

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Rgb(60, 60, 60)))
            .title(Span::styled(
                " Conversation ",
                Style::default()
                    .fg(Color::Rgb(150, 150, 150))
                    .add_modifier(Modifier::BOLD),
            ))
            .style(Style::default().bg(Color::Rgb(20, 20, 25))),
    );

    frame.render_widget(list, area);
}

fn render_input(frame: &mut Frame, area: Rect, app: &App) {
    let input_style = if app.is_processing {
        Style::default().fg(Color::Rgb(100, 100, 100))
    } else {
        Style::default().fg(Color::White)
    };

    let input_text = if app.input.is_empty() && !app.is_processing {
        Span::styled(
            "Type your prompt here...",
            Style::default().fg(Color::Rgb(80, 80, 80)),
        )
    } else {
        Span::styled(&app.input, input_style)
    };

    let input = Paragraph::new(input_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(if app.is_processing {
                    Color::Rgb(60, 60, 60)
                } else {
                    Color::Rgb(78, 205, 196)
                }))
                .title(Span::styled(
                    " Input ",
                    Style::default()
                        .fg(Color::Rgb(78, 205, 196))
                        .add_modifier(Modifier::BOLD),
                ))
                .style(Style::default().bg(Color::Rgb(25, 25, 30))),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(input, area);

    // Show cursor
    if !app.is_processing {
        frame.set_cursor_position((
            area.x + app.cursor_pos as u16 + 1,
            area.y + 1,
        ));
    }
}

fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    let status = if app.is_processing {
        Line::from(vec![
            Span::styled("⟳ ", Style::default().fg(Color::Rgb(255, 200, 100))),
            Span::styled(
                "Processing...",
                Style::default().fg(Color::Rgb(150, 150, 150)),
            ),
            Span::styled(" │ ", Style::default().fg(Color::Rgb(60, 60, 60))),
            Span::styled(
                "ESC to quit",
                Style::default().fg(Color::Rgb(100, 100, 100)),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                "ENTER to send",
                Style::default().fg(Color::Rgb(78, 205, 196)),
            ),
            Span::styled(" │ ", Style::default().fg(Color::Rgb(60, 60, 60))),
            Span::styled(
                "↑↓ scroll",
                Style::default().fg(Color::Rgb(100, 100, 100)),
            ),
            Span::styled(" │ ", Style::default().fg(Color::Rgb(60, 60, 60))),
            Span::styled(
                "ESC to quit",
                Style::default().fg(Color::Rgb(100, 100, 100)),
            ),
        ])
    };

    let status_bar =
        Paragraph::new(status).style(Style::default().bg(Color::Rgb(30, 30, 35)));

    frame.render_widget(status_bar, area);
}

/// Main TUI event loop
pub async fn run_tui(
    actor_tx: mpsc::UnboundedSender<String>,
    ui_rx: mpsc::UnboundedReceiver<UiEvent>,
) -> io::Result<()> {
    let mut terminal = init_terminal()?;
    let mut app = App::new(actor_tx, ui_rx);

    loop {
        terminal.draw(|f| render(f, &app))?;

        // Poll for UI events from actor (non-blocking)
        while let Ok(event) = app.ui_rx.try_recv() {
            app.handle_ui_event(event);
        }

        // Poll for keyboard events with timeout
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.handle_key(key.code);
                }
            }
        }

        if app.should_quit() {
            break;
        }
    }

    restore_terminal(&mut terminal)?;
    Ok(())
}

