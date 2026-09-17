use super::*;
use clients::{ClaudeAuthConfig, ClaudeConfig, ClaudeEffort, ClaudeKeyConfig, config::Config};
use commands::command::{Answer, QuestionAnswer};
use common_models::interaction::{Choice, InteractionView, Question, QuestionInput};
use common_models::tui_models::{Lifecycle, RequestContext, SessionSummary};
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

    fn questions(&mut self, questions: Vec<Question>) {
        self.packet(ActorToTuiPacket::InteractionUpdated(InteractionView {
            planning: self.app.interaction.planning.clone(),
            questions,
        }));
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
    fixture.key(KeyCode::Esc);
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
    let root_turn = common_models::runtime_ids::TurnId::new();
    fixture.packet(ActorToTuiPacket::TurnChanged {
        turn_id: root_turn,
        state: Lifecycle::Running,
        detail: None,
    });
    let worker_turn = common_models::runtime_ids::TurnId::new();
    fixture.app.handle_actor_msg(ActorToTui {
        actor_id: 1,
        packet: ActorToTuiPacket::TurnChanged {
            turn_id: worker_turn,
            state: Lifecycle::Failed,
            detail: Some("Worker tool-call budget exhausted (128 calls)".into()),
        },
    });
    assert!(
        fixture
            .render()
            .contains(&format!("Worker 1 turn {worker_turn}: Failed"))
    );
    assert!(fixture.app.root_busy);
    assert!(matches!(
        fixture.app.progress,
        Progress::Turn(Lifecycle::Running)
    ));
    fixture.stop().await;
}

#[tokio::test]
async fn welcome_gives_way_to_conversation_and_returns_after_clear() {
    let mut fixture = Fixture::new().await;
    let ferris = crate::branding::ferris()[1].to_string();
    let welcome = fixture.render();
    assert!(welcome.contains(crate::branding::TAGLINE));
    assert!(welcome.contains(ferris.trim()));
    fixture.key(KeyCode::Char('i'));
    fixture.app.input_box.paste("Explain this crate");
    fixture.key(KeyCode::Enter);
    let conversation = fixture.render();
    assert!(conversation.contains("Explain this crate"));
    assert!(!conversation.contains(crate::branding::TAGLINE));
    assert!(!conversation.contains(ferris.trim()));
    assert!(!conversation.contains(crate::branding::TITLE));
    fixture.app.clear_messages_and_terminal();
    let welcome = fixture.render();
    assert!(welcome.contains(crate::branding::TAGLINE));
    assert!(welcome.contains(ferris.trim()));
    fixture.stop().await;
}

#[tokio::test]
async fn footer_preserves_model_context_and_shortcuts_at_compact_and_full_widths() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut fixture = Fixture::new().await;
    fixture.packet(ActorToTuiPacket::ContextUpdated(RequestContext {
        estimated_tokens: 12_000,
        ceiling: 100_000,
        response_reserve: 4_000,
    }));
    let effort = fixture.app.config_context.get_config().get_effort();
    for width in [32, 63, 64, 100, 160] {
        let mut terminal = Terminal::new(TestBackend::new(width, 2)).unwrap();
        terminal
            .draw(|frame| fixture.app.draw_footer(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let status: String = (0..width).map(|x| buffer[(x, 0)].symbol()).collect();
        let hints: String = (0..width).map(|x| buffer[(x, 1)].symbol()).collect();
        let model_label = match width {
            0..64 => " fixture ".to_string(),
            _ => format!(" fixture  ·  {effort} "),
        };
        assert!(status.ends_with(&model_label), "{status:?}");
        assert!(status.starts_with(" context ~12.0k/100.0k "), "{status:?}");
        assert_eq!(status.contains(&effort), width >= 64);
        assert!(!status.contains(crate::branding::TITLE));
        assert!(hints.contains("write"));
        assert!(!hints.contains("fixture"));
        assert_eq!(buffer[(width - 2, 0)].fg, theme::MUTED);
    }
    fixture.stop().await;
}

#[tokio::test]
async fn conversation_starts_at_the_top_with_model_information_only_in_the_footer() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut fixture = Fixture::new().await;
    fixture
        .app
        .message_box
        .append(Msg::Message("Conversation starts here".into()));
    for width in [32, 100] {
        let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
        terminal.draw(|frame| fixture.app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = (0..12)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        assert!(rows[0].contains("Conversation starts here"));
        assert!(rows[10].contains("fixture"));
        assert!(rows[11].contains("write"));
        assert!(rows[..10].iter().all(|row| !row.contains("fixture")));
        assert!(rows.iter().all(|row| !row.contains(crate::branding::TITLE)));
        assert!(rows.iter().all(|row| !row.contains("THE RUST WORKSPACE")));
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
    let mut pending = question("long-question", true);
    pending.prompt = "A long question with Unicode: 日本語\n".repeat(20);
    pending.choices[0].label = "A long choice with Unicode: 日本語 ".repeat(6);
    fixture.questions(vec![pending]);
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
            InputMode::CommandMenu(CommandMenu::QuestionSelector),
        ] {
            fixture.app.update_input_mode(mode);
            let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
            terminal.draw(|frame| fixture.app.draw(frame)).unwrap();
            assert!(area.contains(terminal.get_cursor_position().unwrap()));
        }
    }
    fixture.stop().await;
}

fn question(id: &str, allow_free_text: bool) -> Question {
    Question::try_from(QuestionInput {
        id: id.into(),
        prompt: "Which target should be built?".into(),
        required: true,
        choices: vec![
            Choice {
                id: "library-id".into(),
                label: "Library target".into(),
            },
            Choice {
                id: "binary-id".into(),
                label: "Binary target".into(),
            },
        ],
        allow_free_text,
    })
    .unwrap()
}

#[tokio::test]
async fn question_picker_selects_and_submits_a_choice_only_once() {
    let mut fixture = Fixture::new().await;
    fixture.questions(vec![question("target", false)]);
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::CommandMenu(CommandMenu::QuestionSelector)
    ));
    let rendered = fixture.render();
    assert!(rendered.contains("Question 1/1"));
    assert!(rendered.contains("required"));
    assert!(rendered.contains("Library target"));
    assert!(rendered.contains("Binary target"));
    assert!(!rendered.contains("Other (type an answer)"));
    fixture.key(KeyCode::Down);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| fixture.app.draw(frame)).unwrap();
    assert!(
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .any(|cell| { cell.symbol() == "B" && cell.bg == theme::SELECTION })
    );
    fixture
        .app
        .handle_term_event(&Event::Paste("not an allowed answer".into()));
    fixture.questions(vec![question("target", false)]);
    fixture.app.handle_key_event(&KeyEvent::new_with_kind(
        KeyCode::Enter,
        KeyModifiers::NONE,
        crossterm::event::KeyEventKind::Release,
    ));
    assert!(fixture.messages.is_empty());
    fixture.key(KeyCode::Enter);
    let expected = Command::Answer(QuestionAnswer {
        id: "target".into(),
        answer: Answer::Choice {
            choice_id: "binary-id".into(),
        },
    });
    assert_eq!(fixture.command().await, expected);
    assert!(fixture.render().contains("Submitting answer"));
    fixture.key(KeyCode::Enter);
    fixture.questions(vec![question("target", false)]);
    fixture.key(KeyCode::Enter);
    fixture.key(KeyCode::Esc);
    fixture.key(KeyCode::Char('?'));
    fixture.key(KeyCode::Enter);
    fixture.questions(vec![]);
    fixture.packet(ActorToTuiPacket::CommandResult(
        expected,
        "Answer accepted".into(),
    ));
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::HomeMenu(HomeMenu::Normal)
    ));
    assert!(fixture.app.input_box.question_picker.is_empty());
    assert!(fixture.messages.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn question_picker_preserves_drafts_and_can_be_reopened_without_answering() {
    let mut fixture = Fixture::new().await;
    fixture.key(KeyCode::Char('i'));
    fixture.app.input_box.paste("Keep this message draft");
    fixture.questions(vec![question("target", false)]);
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::HomeMenu(HomeMenu::Editing)
    ));
    fixture.key(KeyCode::Esc);
    fixture.key(KeyCode::Char('?'));
    fixture.key(KeyCode::Char('j'));
    fixture.key(KeyCode::Esc);
    fixture.questions(vec![question("target", false)]);
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::HomeMenu(HomeMenu::Normal)
    ));
    assert_eq!(fixture.app.input_box.get_input(), "Keep this message draft");
    fixture.key(KeyCode::Char('?'));
    fixture.key(KeyCode::Enter);
    assert_eq!(
        fixture.command().await,
        Command::Answer(QuestionAnswer {
            id: "target".into(),
            answer: Answer::Choice {
                choice_id: "binary-id".into()
            }
        })
    );
    fixture.questions(vec![]);
    for kind in [KeyEventKind::Release, KeyEventKind::Repeat] {
        fixture.app.handle_key_event(&KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::NONE,
            kind,
        ));
    }
    assert_eq!(fixture.app.input_box.get_input(), "Keep this message draft");
    assert!(fixture.messages.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn questions_command_reopens_the_picker_and_empty_questions_use_the_actor() {
    let mut fixture = Fixture::new().await;
    fixture.questions(vec![question("target", false)]);
    fixture.key(KeyCode::Esc);
    fixture.key(KeyCode::Char('/'));
    fixture.app.input_box.paste("questions");
    fixture.key(KeyCode::Enter);
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::CommandMenu(CommandMenu::QuestionSelector)
    ));
    assert!(fixture.messages.is_empty());
    fixture.questions(vec![]);
    fixture.key(KeyCode::Char('/'));
    fixture.app.input_box.paste("questions");
    fixture.key(KeyCode::Enter);
    assert_eq!(fixture.command().await, Command::Questions);
    fixture.stop().await;
}

#[tokio::test]
async fn question_picker_validates_and_sends_free_text_without_command_syntax() {
    let mut fixture = Fixture::new().await;
    fixture.questions(vec![question("target", true)]);
    fixture.key(KeyCode::Up);
    fixture.key(KeyCode::Enter);
    assert!(fixture.render().contains("Your answer:"));
    fixture.key(KeyCode::Enter);
    assert!(fixture.messages.is_empty());
    assert!(fixture.render().contains("nonempty permitted text"));
    fixture
        .app
        .handle_term_event(&Event::Paste("Custom 日本語 🦀".into()));
    fixture.key(KeyCode::Backspace);
    fixture.key(KeyCode::Char('k'));
    fixture.key(KeyCode::Esc);
    fixture.key(KeyCode::Enter);
    fixture.questions(vec![question("target", true)]);
    assert!(
        fixture
            .render()
            .split_whitespace()
            .collect::<String>()
            .contains("Custom日本語k")
    );
    fixture
        .app
        .handle_key_event(&KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    fixture
        .app
        .handle_term_event(&Event::Paste("/literal answer".into()));
    fixture.key(KeyCode::Enter);
    assert_eq!(
        fixture.command().await,
        Command::Answer(QuestionAnswer {
            id: "target".into(),
            answer: Answer::Text("Custom 日本語 k\n/literal answer".into())
        })
    );
    assert!(fixture.app.input_box.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn question_picker_handles_text_only_questions_and_enforces_the_answer_limit() {
    let mut fixture = Fixture::new().await;
    let mut pending = question("details", true);
    pending.choices.clear();
    fixture.questions(vec![pending]);
    assert!(fixture.render().contains("Your answer:"));
    fixture.app.handle_term_event(&Event::Paste(" ".into()));
    fixture.key(KeyCode::Enter);
    fixture
        .app
        .handle_term_event(&Event::Paste("a".repeat(8192)));
    fixture.key(KeyCode::Enter);
    assert!(fixture.messages.is_empty());
    fixture
        .app
        .handle_key_event(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    fixture
        .app
        .handle_term_event(&Event::Paste("Use the workspace".into()));
    fixture.key(KeyCode::Enter);
    assert_eq!(
        fixture.command().await,
        Command::Answer(QuestionAnswer {
            id: "details".into(),
            answer: Answer::Text("Use the workspace".into())
        })
    );
    fixture.stop().await;
}

#[tokio::test]
async fn question_picker_navigates_pending_questions_and_recovers_from_rejected_answers() {
    let mut fixture = Fixture::new().await;
    let first = question("first", false);
    let mut second = question("second", false);
    second.required = false;
    fixture.questions(vec![first.clone(), second.clone()]);
    fixture.key(KeyCode::Tab);
    assert!(fixture.render().contains("Question 2/2"));
    assert!(fixture.render().contains("optional"));
    fixture.key(KeyCode::BackTab);
    assert!(fixture.render().contains("Question 1/2"));
    fixture.key(KeyCode::Tab);
    fixture.key(KeyCode::Up);
    fixture.questions(vec![first.clone(), second.clone()]);
    fixture.key(KeyCode::Enter);
    let submitted = Command::Answer(QuestionAnswer {
        id: "second".into(),
        answer: Answer::Choice {
            choice_id: "binary-id".into(),
        },
    });
    assert_eq!(fixture.command().await, submitted);
    fixture.packet(ActorToTuiPacket::CommandResult(
        submitted.clone(),
        "Could not save answer".into(),
    ));
    assert!(fixture.render().contains("Could not save answer"));
    fixture.key(KeyCode::Enter);
    assert_eq!(fixture.command().await, submitted);
    fixture.questions(vec![first]);
    fixture.packet(ActorToTuiPacket::CommandResult(
        submitted,
        "Answer accepted".into(),
    ));
    assert!(fixture.render().contains("Question 1/1 · first"));
    fixture.key(KeyCode::Enter);
    assert_eq!(
        fixture.command().await,
        Command::Answer(QuestionAnswer {
            id: "first".into(),
            answer: Answer::Choice {
                choice_id: "library-id".into()
            }
        })
    );
    fixture.questions(vec![]);
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::HomeMenu(HomeMenu::Normal)
    ));
    fixture.stop().await;
}

#[tokio::test]
async fn question_picker_reopens_restored_questions_and_clears_stale_session_state() {
    let mut fixture = Fixture::new().await;
    fixture.questions(vec![question("target", true)]);
    fixture.key(KeyCode::Enter);
    fixture.command().await;
    fixture.key(KeyCode::Esc);
    fixture
        .app
        .resume(ResumeTarget::Session { id: "saved".into() });
    fixture.command().await;
    fixture.questions(vec![question("target", true)]);
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::CommandMenu(CommandMenu::SessionSelector)
    ));
    fixture.packet(ActorToTuiPacket::SessionResumed(Ok(SessionTranscript {
        id: "saved".into(),
        messages: vec![],
    })));
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::CommandMenu(CommandMenu::QuestionSelector)
    ));
    assert!(fixture.render().contains("Library target"));
    fixture.packet(ActorToTuiPacket::SessionChanged);
    assert!(fixture.app.input_box.question_picker.is_empty());
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::HomeMenu(HomeMenu::Normal)
    ));
    fixture.key(KeyCode::Char('?'));
    assert!(fixture.messages.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn question_picker_ignores_worker_interactions_and_answer_results() {
    let mut fixture = Fixture::new().await;
    fixture.app.handle_actor_msg(ActorToTui {
        actor_id: 1,
        packet: ActorToTuiPacket::InteractionUpdated(InteractionView {
            planning: Default::default(),
            questions: vec![question("worker", false)],
        }),
    });
    assert!(fixture.app.input_box.question_picker.is_empty());
    fixture.questions(vec![question("root", false)]);
    fixture.key(KeyCode::Enter);
    let submitted = fixture.command().await;
    fixture.app.handle_actor_msg(ActorToTui {
        actor_id: 1,
        packet: ActorToTuiPacket::CommandResult(submitted, "Worker result".into()),
    });
    fixture.key(KeyCode::Enter);
    fixture.questions(vec![]);
    fixture.key(KeyCode::Enter);
    assert!(fixture.messages.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn question_picker_scrolls_long_prompts_and_keeps_the_selected_choice_visible() {
    let mut fixture = Fixture::new().await;
    let mut pending = question("long", false);
    pending.prompt = (0..24)
        .map(|index| format!("Prompt line {index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    pending.choices = (0..6)
        .map(|index| Choice {
            id: format!("choice-{index}"),
            label: format!("Answer {index}"),
        })
        .collect();
    fixture.questions(vec![pending]);
    let render = |app: &mut TUIApp| {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 12)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_stateful_widget(InputBox::new(), frame.area(), &mut app.input_box);
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };
    let initial = render(&mut fixture.app);
    assert!(initial.contains("Prompt line 00"));
    assert!(!initial.contains("Prompt line 23"));
    fixture.key(KeyCode::Up);
    for _ in 0..8 {
        fixture.key(KeyCode::PageDown);
        render(&mut fixture.app);
    }
    let scrolled = render(&mut fixture.app);
    assert!(scrolled.contains("Prompt line 23"));
    assert!(!scrolled.contains("Prompt line 00"));
    assert!(scrolled.contains("Answer 5"));
    for _ in 0..8 {
        fixture.key(KeyCode::PageUp);
        render(&mut fixture.app);
    }
    assert!(render(&mut fixture.app).contains("Prompt line 00"));
    fixture.key(KeyCode::Enter);
    assert_eq!(
        fixture.command().await,
        Command::Answer(QuestionAnswer {
            id: "long".into(),
            answer: Answer::Choice {
                choice_id: "choice-5".into(),
            },
        })
    );
    fixture.stop().await;
}
