use super::*;
use clients::{ClaudeAuthConfig, ClaudeConfig, ClaudeEffort, ClaudeKeyConfig, config::Config};
use common_models::tui_models::{Lifecycle, SessionSummary};
use ractor::{Actor, ActorProcessingErr};

struct Capture;

impl Actor for Capture {
    type Msg = Message;
    type State = flume::Sender<Message>;
    type Arguments = flume::Sender<Message>;

    async fn pre_start(
        &self,
        _: ActorRef<Message>,
        sender: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(sender)
    }

    async fn handle(
        &self,
        _: ActorRef<Message>,
        message: Message,
        sender: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        sender
            .send(message)
            .map_err(|error| error.to_string().into())
    }
}

struct Fixture {
    app: TUIApp,
    messages: flume::Receiver<Message>,
    handle: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let (sender, messages) = flume::unbounded();
        let (actor, handle) = Actor::spawn(None, Capture, sender).await.unwrap();
        let config = Config::Claude(ClaudeConfig {
            auth: ClaudeAuthConfig::APIKey(ClaudeKeyConfig {
                api_key: String::new(),
            }),
            model: "fixture".into(),
            effort: ClaudeEffort::Med,
        });
        let mut fixture = Self {
            app: TUIApp::new(actor, ConfigContext::new(config), false),
            messages,
            handle,
        };
        fixture.render();
        fixture
    }

    fn open(&mut self) {
        self.app
            .update_input_mode(InputMode::HomeMenu(HomeMenu::InputCommand));
        self.app.input_box.paste("resume");
        self.app.submit_command();
    }

    fn packet(&mut self, packet: ActorToTuiPacket) {
        self.app.handle_actor_msg(ActorToTui {
            actor_id: 0,
            packet,
        });
    }

    fn key(&mut self, code: KeyCode) {
        self.app
            .handle_key_event(&KeyEvent::new(code, KeyModifiers::NONE));
    }

    async fn command(&self) -> Command {
        let message = tokio::time::timeout(Duration::from_secs(2), self.messages.recv_async())
            .await
            .unwrap()
            .unwrap();
        match message {
            Message::Command(command) => command,
            _ => panic!("Expected a command"),
        }
    }

    fn render(&mut self) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| self.app.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    async fn stop(self) {
        self.app.actor_ref.stop(None);
        self.handle.await.unwrap();
    }
}

fn choice() -> SessionSummary {
    SessionSummary {
        id: "saved-session".into(),
        title: "Fix storage".into(),
        preview: "Saved reply".into(),
        updated_at: None,
        status: Lifecycle::Completed,
    }
}

#[tokio::test]
async fn bare_resume_selects_a_session_and_replaces_the_visible_transcript() {
    let mut fixture = Fixture::new().await;
    fixture
        .app
        .message_box
        .append(Msg::Message("Previous conversation".into()));
    fixture.open();
    assert_eq!(
        fixture.command().await,
        Command::Resume(ResumeTarget::Picker)
    );
    fixture.packet(ActorToTuiPacket::SessionChoices(Ok(vec![choice()])));
    let rendered = fixture.render();
    assert!(rendered.contains("Resume a session"));
    assert!(rendered.contains("Fix storage"));
    fixture.key(KeyCode::Enter);
    assert_eq!(
        fixture.command().await,
        Command::Resume(ResumeTarget::Session {
            id: "saved-session".into()
        })
    );
    fixture.key(KeyCode::Enter);
    assert!(fixture.messages.is_empty());
    fixture.packet(ActorToTuiPacket::TokensUpdated(TokenCount {
        input_tokens: 123,
        output_tokens: 45,
    }));
    fixture.packet(ActorToTuiPacket::SessionResumed(Ok(SessionTranscript {
        id: "saved-session".into(),
        messages: vec![
            SessionMessage::User("Saved request".into()),
            SessionMessage::Tool("Saved tool result".into()),
            SessionMessage::Assistant("Saved answer".into()),
        ],
    })));
    let rendered = fixture.render();
    assert!(
        rendered.contains("Saved request")
            && rendered.contains("Saved answer")
            && rendered.contains("Saved tool result")
    );
    assert!(!rendered.contains("Previous conversation"));
    assert_eq!(fixture.app.token_count.input_tokens, 123);
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::HomeMenu(HomeMenu::Normal)
    ));
    assert!(matches!(
        fixture.app.input_box.session_picker,
        SessionPickerState::Closed
    ));
    fixture.packet(ActorToTuiPacket::SessionResumed(Ok(SessionTranscript {
        id: "stale-session".into(),
        messages: vec![SessionMessage::User("Stale conversation".into())],
    })));
    let rendered = fixture.render();
    assert!(rendered.contains("Saved request"));
    assert!(!rendered.contains("Stale conversation"));
    assert!(fixture.messages.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn cancellation_empty_results_and_resume_errors_preserve_the_current_conversation() {
    let mut fixture = Fixture::new().await;
    fixture
        .app
        .message_box
        .append(Msg::Message("Keep this conversation".into()));
    fixture.open();
    fixture.command().await;
    fixture.key(KeyCode::Esc);
    fixture.packet(ActorToTuiPacket::SessionChoices(Ok(vec![choice()])));
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::HomeMenu(HomeMenu::Normal)
    ));
    assert!(fixture.render().contains("Keep this conversation"));
    fixture.open();
    fixture.command().await;
    fixture.packet(ActorToTuiPacket::SessionChoices(Ok(vec![])));
    fixture.key(KeyCode::Enter);
    assert!(fixture.messages.is_empty());
    assert!(fixture.render().contains("No saved conversations"));
    fixture.key(KeyCode::Esc);
    fixture.open();
    fixture.command().await;
    fixture.packet(ActorToTuiPacket::SessionChoices(Ok(vec![choice()])));
    fixture.key(KeyCode::Enter);
    fixture.command().await;
    fixture.packet(ActorToTuiPacket::SessionResumed(Err(
        "Session is already open in another process".into(),
    )));
    assert!(fixture.render().contains("already open in another process"));
    fixture.key(KeyCode::Esc);
    assert!(fixture.render().contains("Keep this conversation"));
    fixture.stop().await;
}

#[tokio::test]
async fn diff_and_guarded_undo_commands_use_the_existing_transcript_flow() {
    let mut fixture = Fixture::new().await;
    fixture
        .app
        .update_input_mode(InputMode::HomeMenu(HomeMenu::InputCommand));
    fixture.app.input_box.paste("diff");
    fixture.app.submit_command();
    assert_eq!(fixture.command().await, Command::Diff);
    fixture.packet(ActorToTuiPacket::CommandResult(
        Command::Diff,
        "Task file.txt\n```diff\n-old content\n+reviewed content\n```".into(),
    ));
    assert!(fixture.render().contains("reviewed content"));
    fixture
        .app
        .update_input_mode(InputMode::HomeMenu(HomeMenu::InputCommand));
    fixture.app.input_box.paste("undo joe-edit");
    fixture.app.submit_command();
    assert_eq!(fixture.command().await, Command::Undo("joe-edit".into()));
    fixture.stop().await;
}

#[tokio::test]
async fn interaction_commands_questions_and_queue_preserve_vim_and_transcript() {
    use common_models::interaction::{
        InteractionView, Planning, Question, QuestionInput, WorkMode,
    };
    use common_models::{runtime_ids::TurnId, tui_models::InputKind};
    let mut fixture = Fixture::new().await;
    fixture.packet(ActorToTuiPacket::InteractionUpdated(InteractionView {
        planning: Planning {
            mode: WorkMode::Plan,
            ..Default::default()
        },
        questions: vec![
            Question::try_from(QuestionInput {
                id: "target".into(),
                prompt: "Which target?".into(),
                required: true,
                choices: vec![],
                allow_free_text: true,
            })
            .unwrap(),
        ],
    }));
    let rendered = fixture.render();
    assert!(rendered.contains("Which target?"));
    assert!(rendered.contains("PLAN"));
    assert!(rendered.contains("plan 0/0"));
    assert!(rendered.contains("questions 1"));
    fixture.key(KeyCode::Char('/'));
    fixture.app.input_box.paste("answer target text Library");
    fixture.key(KeyCode::Enter);
    assert_eq!(
        fixture.command().await,
        Command::parse("answer target text Library").unwrap()
    );
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::HomeMenu(HomeMenu::Normal)
    ));
    let queued = TurnId::new();
    fixture.packet(ActorToTuiPacket::Queued {
        turn_id: queued,
        position: 1,
    });
    assert!(fixture.render().contains("queued 1"));
    fixture.packet(ActorToTuiPacket::InputAccepted {
        turn_id: queued,
        kind: InputKind::Active,
    });
    assert!(fixture.render().contains("queued 0"));
    fixture.packet(ActorToTuiPacket::TurnChanged {
        turn_id: queued,
        state: Lifecycle::WaitingForInput,
        detail: None,
    });
    fixture
        .app
        .handle_key_event(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(matches!(
        fixture.messages.recv_async().await.unwrap(),
        Message::Interrupt
    ));
    assert!(!fixture.app.do_quit);
    fixture.key(KeyCode::Char('i'));
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::HomeMenu(HomeMenu::Editing)
    ));
    fixture.key(KeyCode::Esc);
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::HomeMenu(HomeMenu::Normal)
    ));
    fixture.stop().await;
}

#[tokio::test]
async fn worker_streams_update_progress_without_replacing_the_root_stream() {
    let mut fixture = Fixture::new().await;
    fixture.packet(ActorToTuiPacket::StateChanged(State::MessageStart));
    fixture.packet(ActorToTuiPacket::Data("Root response".into()));
    fixture.app.handle_actor_msg(ActorToTui {
        actor_id: 1,
        packet: ActorToTuiPacket::StateChanged(State::ThinkingStart),
    });
    fixture.app.handle_actor_msg(ActorToTui {
        actor_id: 1,
        packet: ActorToTuiPacket::Data("Worker private stream".into()),
    });
    fixture.app.handle_actor_msg(ActorToTui {
        actor_id: 1,
        packet: ActorToTuiPacket::TurnChanged {
            turn_id: common_models::runtime_ids::TurnId::new(),
            state: Lifecycle::Running,
            detail: None,
        },
    });
    fixture.packet(ActorToTuiPacket::Data(" remains intact".into()));
    fixture.packet(ActorToTuiPacket::StateChanged(State::MessageStop));
    let rendered = fixture.render();
    assert!(rendered.contains("Root response remains intact"));
    assert!(!rendered.contains("Worker private stream"));
    assert!(rendered.contains("workers 1"));
    fixture.packet(ActorToTuiPacket::ValidationUpdated(
        common_models::tui_models::ValidationProgress {
            operation: "test".into(),
            state: common_models::tui_models::ValidationState::Passed,
        },
    ));
    fixture.packet(ActorToTuiPacket::OperationChanged {
        turn_id: common_models::runtime_ids::TurnId::new(),
        operation_id: common_models::runtime_ids::OperationId::new(),
        state: Lifecycle::Running,
        detail: "Provider request".into(),
    });
    assert!(fixture.render().contains("last test Passed"));
    fixture.stop().await;
}

#[tokio::test]
async fn welcome_gives_way_to_conversation_and_returns_after_clear() {
    let mut fixture = Fixture::new().await;
    let welcome = fixture.render();
    assert!(welcome.contains(crate::branding::TAGLINE));
    assert!(welcome.contains(crate::branding::FERRIS[1].trim()));
    fixture.key(KeyCode::Char('i'));
    fixture.app.input_box.paste("Explain this crate");
    fixture.key(KeyCode::Enter);
    let conversation = fixture.render();
    assert!(conversation.contains("Explain this crate"));
    assert!(!conversation.contains(crate::branding::TAGLINE));
    assert!(!conversation.contains(crate::branding::FERRIS[1].trim()));
    assert!(conversation.contains(crate::branding::TITLE));
    fixture.app.clear_messages_and_terminal();
    let welcome = fixture.render();
    assert!(welcome.contains(crate::branding::TAGLINE));
    assert!(welcome.contains(crate::branding::FERRIS[1].trim()));
    fixture.stop().await;
}

#[tokio::test]
async fn ferris_header_preserves_the_model_label_at_compact_and_full_widths() {
    use ratatui::{Terminal, backend::TestBackend, style::Modifier};

    let fixture = Fixture::new().await;
    for width in [32, 63, 64, 100] {
        let mut terminal = Terminal::new(TestBackend::new(width, 2)).unwrap();
        terminal
            .draw(|frame| fixture.app.draw_header(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains(crate::branding::TITLE));
        assert!(text.contains("fixture"));
        assert_eq!(text.contains("THE RUST WORKSPACE"), width >= 64);
        assert_eq!(buffer[(1, 0)].fg, theme::ACCENT);
        assert!(buffer[(1, 0)].modifier.contains(Modifier::BOLD));
    }
    fixture.stop().await;
}

#[tokio::test]
async fn orange_branding_preserves_semantic_progress_and_validation_colors() {
    use common_models::tui_models::{ValidationProgress, ValidationState};

    let mut fixture = Fixture::new().await;
    for (state, color) in [
        (Lifecycle::Completed, theme::GREEN),
        (Lifecycle::Failed, theme::RED),
    ] {
        for progress in [
            Progress::Turn(state),
            Progress::Operation {
                state,
                detail: "test".into(),
            },
        ] {
            fixture.app.progress = progress;
            let line = fixture.app.progress_line(100);
            assert_eq!(line.spans[0].style.bg, Some(theme::ACCENT));
            assert_eq!(line.spans[1].style.fg, Some(color));
        }
    }
    fixture.app.progress = Progress::Turn(Lifecycle::Running);
    fixture.app.root_busy = true;
    assert_eq!(
        fixture.app.progress_line(100).spans[1].style.fg,
        Some(theme::AMBER)
    );
    for (state, color) in [
        (ValidationState::Passed, theme::GREEN),
        (ValidationState::Failed, theme::RED),
        (ValidationState::NotRun, theme::MUTED),
    ] {
        fixture.app.validation = Some(ValidationProgress {
            operation: "test".into(),
            state,
        });
        let line = fixture.app.progress_line(100);
        let status = line
            .spans
            .iter()
            .find(|span| span.content.contains("last test"))
            .unwrap();
        assert_eq!(status.style.fg, Some(color));
    }
    fixture.stop().await;
}

#[tokio::test]
async fn long_prompt_scrolls_with_the_cursor_in_both_directions() {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    let mut fixture = Fixture::new().await;
    fixture.key(KeyCode::Char('i'));
    fixture
        .app
        .input_box
        .paste("first line\nsecond line\nthird line\nfourth line\nlast line");
    let area = Rect::new(0, 0, 24, 5);
    let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
    terminal
        .draw(|frame| {
            frame.render_stateful_widget(InputBox::new(), area, &mut fixture.app.input_box);
        })
        .unwrap();
    let cursor = fixture.app.input_box.get_cursor_pos(&area);
    assert_eq!(cursor, ratatui::layout::Position::new(10, 3));
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();
    assert!(text.contains("last line"));
    assert!(!text.contains("first line"));

    fixture.key(KeyCode::Esc);
    fixture.key(KeyCode::Char('g'));
    fixture.key(KeyCode::Char('g'));
    terminal
        .draw(|frame| {
            frame.render_stateful_widget(InputBox::new(), area, &mut fixture.app.input_box);
        })
        .unwrap();
    let cursor = fixture.app.input_box.get_cursor_pos(&area);
    assert_eq!(cursor.y, 1);
    let text: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();
    assert!(text.contains("first line"));
    assert!(!text.contains("last line"));
    fixture.stop().await;
}

#[tokio::test]
async fn all_input_modes_render_within_small_terminal_bounds() {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    let mut fixture = Fixture::new().await;
    fixture
        .app
        .input_box
        .paste(&"A long prompt with Unicode: 日本語\n".repeat(20));
    for area in [
        Rect::new(0, 0, 100, 24),
        Rect::new(0, 0, 60, 16),
        Rect::new(0, 0, 32, 10),
        Rect::new(0, 0, 12, 5),
        Rect::new(0, 0, 1, 1),
    ] {
        for mode in [
            InputMode::HomeMenu(HomeMenu::Normal),
            InputMode::HomeMenu(HomeMenu::Editing),
            InputMode::HomeMenu(HomeMenu::InputCommand),
            InputMode::CommandMenu(CommandMenu::ModelSelector),
            InputMode::CommandMenu(CommandMenu::SessionSelector),
        ] {
            fixture.app.update_input_mode(mode);
            let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
            terminal.draw(|frame| fixture.app.draw(frame)).unwrap();
            assert!(area.contains(terminal.get_cursor_position().unwrap()));
        }
    }
    fixture.stop().await;
}
