use super::*;
use futures::StreamExt;
use ratatui::{TerminalOptions, Viewport, backend::TestBackend};

struct RenderedUi {
    terminal: Terminal<TestBackend>,
    frames: Vec<String>,
}

impl RenderedUi {
    fn new(width: u16, height: u16) -> Self {
        Self {
            terminal: Terminal::with_options(
                TestBackend::new(width, height),
                TerminalOptions {
                    viewport: Viewport::Inline(height),
                },
            )
            .unwrap(),
            frames: Vec::new(),
        }
    }

    fn render(&mut self, app: &mut TUIApp) -> Result<()> {
        if app.do_clear_terminal {
            self.terminal.backend_mut().clear()?;
            self.terminal.clear()?;
        }
        app.render_frame(&mut self.terminal)?;
        self.frames.push(
            self.terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect(),
        );
        Ok(())
    }

    fn output(&self) -> String {
        let backend = self.terminal.backend();
        backend
            .scrollback()
            .content
            .iter()
            .chain(backend.buffer().content.iter())
            .map(|cell| cell.symbol())
            .collect()
    }
}

fn root(packet: ActorToTuiPacket) -> ActorToTui {
    ActorToTui {
        actor_id: 0,
        packet,
    }
}

fn key_event(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

#[tokio::test(start_paused = true)]
async fn coalesced_stream_and_logout_preserve_every_output_line() {
    let mut fixture = Fixture::new().await;
    let mut rendered = RenderedUi::new(100, 30);
    let (tx, rx) = flume::unbounded();
    tx.send(root(ActorToTuiPacket::StateChanged(State::MessageStart)))
        .unwrap();
    for index in 0..100 {
        tx.send(root(ActorToTuiPacket::Data(format!("part-{index:03}\n"))))
            .unwrap();
    }
    tx.send(root(ActorToTuiPacket::StateChanged(State::MessageStop)))
        .unwrap();
    tx.send(root(ActorToTuiPacket::CommandResult(
        Command::Logout,
        "Logout complete".into(),
    )))
    .unwrap();
    fixture
        .app
        .run_events(&mut futures::stream::pending(), &rx, |app| {
            rendered.render(app)
        })
        .await
        .unwrap();
    assert_eq!(rendered.frames.len(), 2);
    let output = rendered.output();
    for index in 0..100 {
        assert_eq!(output.matches(&format!("part-{index:03}")).count(), 1);
    }
    assert!(rendered.frames.last().unwrap().contains("Logout complete"));
    fixture.stop().await;
}

#[tokio::test(start_paused = true)]
async fn channel_closure_draws_the_last_partial_stream() {
    let mut fixture = Fixture::new().await;
    let mut rendered = RenderedUi::new(100, 30);
    let (tx, rx) = flume::unbounded();
    tx.send(root(ActorToTuiPacket::StateChanged(State::MessageStart)))
        .unwrap();
    tx.send(root(ActorToTuiPacket::Data("Final partial output".into())))
        .unwrap();
    drop(tx);
    fixture
        .app
        .run_events(&mut futures::stream::pending(), &rx, |app| {
            rendered.render(app)
        })
        .await
        .unwrap();
    assert_eq!(rendered.frames.len(), 2);
    assert!(
        rendered
            .frames
            .last()
            .unwrap()
            .contains("Final partial output")
    );
    fixture.stop().await;
}

#[tokio::test(start_paused = true)]
async fn coalesced_input_preserves_paste_vim_modes_and_submission() {
    let mut fixture = Fixture::new().await;
    let mut rendered = RenderedUi::new(100, 30);
    let (_tx, rx) = flume::unbounded();
    let events = futures::stream::iter([
        key_event(KeyCode::Char('i')),
        Event::Paste("Draft λ".into()),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
        key_event(KeyCode::Char('z')),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('x'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        )),
        key_event(KeyCode::Esc),
        Event::Key(KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        )),
        key_event(KeyCode::Enter),
    ])
    .map(Ok)
    .chain(futures::stream::once(async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(key_event(KeyCode::Char('q')))
    }));
    let mut events = Box::pin(events);
    fixture
        .app
        .run_events(&mut events, &rx, |app| rendered.render(app))
        .await
        .unwrap();
    let message = fixture.messages.try_recv().unwrap();
    assert!(matches!(message, Message::StartWork(Some(text)) if text == "Draft λ\nz"));
    assert!(fixture.messages.is_empty());
    assert!(fixture.app.input_box.is_empty());
    assert!(matches!(
        fixture.app.input_mode,
        InputMode::HomeMenu(HomeMenu::Normal)
    ));
    assert!(rendered.output().contains("Draft λ"));
    assert_eq!(rendered.frames.len(), 3);
    fixture.stop().await;
}

#[tokio::test(start_paused = true)]
async fn coalesced_background_packets_remain_available_in_both_views() {
    let mut fixture = Fixture::new().await;
    fixture.agent_progress(1, Lifecycle::Running);
    fixture.inspect_agent(1);
    let mut rendered = RenderedUi::new(100, 30);
    let (tx, rx) = flume::unbounded();
    for actor_id in [0, 1] {
        for packet in [
            ActorToTuiPacket::StateChanged(State::MessageStart),
            ActorToTuiPacket::Data(format!("Output belonging to actor {actor_id}")),
            ActorToTuiPacket::StateChanged(State::MessageStop),
        ] {
            tx.send(ActorToTui { actor_id, packet }).unwrap();
        }
    }
    drop(tx);
    fixture
        .app
        .run_events(&mut futures::stream::pending(), &rx, |app| {
            rendered.render(app)
        })
        .await
        .unwrap();
    let agent = rendered.frames.last().unwrap();
    assert!(agent.contains("Output belonging to actor 1"));
    assert!(!agent.contains("Output belonging to actor 0"));
    fixture.key(KeyCode::Esc);
    let main = fixture.render();
    assert!(main.contains("Output belonging to actor 0"));
    assert!(!main.contains("Output belonging to actor 1"));
    fixture.stop().await;
}

#[tokio::test]
async fn resize_and_input_growth_flush_overflow_without_another_tick() {
    let mut fixture = Fixture::new().await;
    let mut rendered = RenderedUi::new(100, 30);
    fixture.packet(ActorToTuiPacket::StateChanged(State::MessageStart));
    fixture.packet(ActorToTuiPacket::Data(
        (0..20).map(|index| format!("row-{index:02}\n")).collect(),
    ));
    rendered.render(&mut fixture.app).unwrap();
    assert_eq!(fixture.app.message_box.history_line_count(), 20);
    rendered.terminal.backend_mut().resize(100, 16);
    fixture.app.handle_term_event(&Event::Resize(100, 16));
    rendered.render(&mut fixture.app).unwrap();
    assert!(fixture.app.message_box.history_line_count() <= 10);
    fixture.key(KeyCode::Char('i'));
    fixture
        .app
        .handle_term_event(&Event::Paste("draft\ndraft\ndraft\ndraft\ndraft".into()));
    rendered.render(&mut fixture.app).unwrap();
    assert!(fixture.app.message_box.history_line_count() <= 6);
    let output = rendered.output();
    for index in 0..20 {
        assert_eq!(output.matches(&format!("row-{index:02}")).count(), 1);
    }
    assert!(rendered.frames.last().unwrap().contains("row-19"));
    rendered.render(&mut fixture.app).unwrap();
    assert_eq!(rendered.output(), output);
    fixture.stop().await;
}

#[tokio::test]
async fn restored_session_flushes_history_in_the_clear_frame() {
    let mut fixture = Fixture::new().await;
    let mut rendered = RenderedUi::new(100, 30);
    fixture
        .app
        .message_box
        .append(Msg::Message("Previous conversation".into()));
    rendered.render(&mut fixture.app).unwrap();
    fixture.app.restore_transcript(SessionTranscript {
        id: "saved-session".into(),
        messages: vec![SessionMessage::Assistant(
            (0..80).map(|index| format!("saved-{index:02}\n")).collect(),
        )],
    });
    assert!(fixture.app.do_clear_terminal);
    rendered.render(&mut fixture.app).unwrap();
    assert!(!fixture.app.do_clear_terminal);
    assert!(fixture.app.message_box.history_line_count() <= 24);
    let output = rendered.output();
    assert!(!output.contains("Previous conversation"));
    for index in 0..80 {
        assert_eq!(output.matches(&format!("saved-{index:02}")).count(), 1);
    }
    assert!(rendered.frames.last().unwrap().contains("Resumed session"));
    fixture.stop().await;
}

#[tokio::test(start_paused = true)]
async fn session_age_refreshes_stop_when_the_picker_is_hidden() {
    let mut fixture = Fixture::new().await;
    fixture.open();
    fixture.command().await;
    let mut session = choice();
    session.updated_at = Some(std::time::SystemTime::now() - Duration::from_secs(59));
    fixture.packet(ActorToTuiPacket::SessionChoices(Ok(vec![session])));
    let deadline = fixture.app.redraw_deadline().unwrap();
    assert!(deadline <= tokio::time::Instant::now() + Duration::from_secs(1));
    fixture.app.agents.open();
    assert_eq!(fixture.app.redraw_deadline(), None);
    fixture.app.agents.show_main();
    assert!(fixture.app.redraw_deadline().is_some());
    fixture.key(KeyCode::Esc);
    assert_eq!(fixture.app.redraw_deadline(), None);
    fixture.stop().await;
}
