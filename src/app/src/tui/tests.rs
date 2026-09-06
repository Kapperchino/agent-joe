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
