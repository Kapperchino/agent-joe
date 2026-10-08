use super::*;
use common_models::tui_models::ActorToTuiPacket;
use futures::FutureExt;

fn packet(text: &str) -> ActorToTui {
    ActorToTui {
        actor_id: 0,
        packet: ActorToTuiPacket::Data(text.into()),
    }
}

#[tokio::test(start_paused = true)]
async fn idle_ui_has_no_periodic_redraws_and_wakes_on_updates() {
    let mut schedule = RenderSchedule::new();
    let (tx, rx) = flume::unbounded();
    let mut events = futures::stream::pending();
    assert!(matches!(
        schedule.next_event(&mut events, &rx).await.unwrap(),
        UiEvent::Render
    ));
    schedule.rendered(None);
    for _ in 0..60 {
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(
            schedule
                .next_event(&mut events, &rx)
                .now_or_never()
                .is_none()
        );
    }
    tx.send(packet("update")).unwrap();
    assert!(matches!(
        schedule.next_event(&mut events, &rx).await.unwrap(),
        UiEvent::Actor(_)
    ));
    schedule.request();
    assert!(matches!(
        schedule.next_event(&mut events, &rx).await.unwrap(),
        UiEvent::Render
    ));
}

#[tokio::test(start_paused = true)]
async fn packet_bursts_are_lossless_and_coalesce_without_postponing_frames() {
    let mut schedule = RenderSchedule::new();
    let (tx, rx) = flume::unbounded();
    let mut events = futures::stream::pending();
    schedule.next_event(&mut events, &rx).await.unwrap();
    schedule.rendered(None);
    for _ in 0..3 {
        for index in 0..100 {
            tx.send(packet(&index.to_string())).unwrap();
        }
        for index in 0..100 {
            let event = schedule.next_event(&mut events, &rx).await.unwrap();
            match event {
                UiEvent::Actor(message) => {
                    assert!(
                        matches!(message.packet, ActorToTuiPacket::Data(data) if data == index.to_string())
                    );
                }
                _ => panic!("Expected queued data before the frame deadline"),
            }
            schedule.request();
        }
        assert!(
            schedule
                .next_event(&mut events, &rx)
                .now_or_never()
                .is_none()
        );
        tokio::time::advance(FRAME_INTERVAL - Duration::from_millis(1)).await;
        schedule.request();
        assert!(
            schedule
                .next_event(&mut events, &rx)
                .now_or_never()
                .is_none()
        );
        tokio::time::advance(Duration::from_millis(1)).await;
        tx.send(packet("still streaming")).unwrap();
        assert!(matches!(
            schedule.next_event(&mut events, &rx).await.unwrap(),
            UiEvent::Render
        ));
        schedule.rendered(None);
        assert!(matches!(
            schedule.next_event(&mut events, &rx).await.unwrap(),
            UiEvent::Actor(_)
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn animation_deadlines_survive_updates_and_do_not_catch_up_in_bursts() {
    let mut schedule = RenderSchedule::new();
    let (_tx, rx) = flume::unbounded();
    let mut events = futures::stream::pending();
    let deadline = Instant::now() + Duration::from_millis(300);
    schedule.rendered(Some(deadline));
    tokio::time::advance(Duration::from_millis(100)).await;
    schedule.request();
    assert!(matches!(
        schedule.next_event(&mut events, &rx).await.unwrap(),
        UiEvent::Render
    ));
    schedule.rendered(Some(deadline));
    tokio::time::advance(Duration::from_millis(199)).await;
    assert!(
        schedule
            .next_event(&mut events, &rx)
            .now_or_never()
            .is_none()
    );
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(matches!(
        schedule.next_event(&mut events, &rx).await.unwrap(),
        UiEvent::Render
    ));
    schedule.rendered(Some(Instant::now() + Duration::from_millis(300)));
    assert!(
        schedule
            .next_event(&mut events, &rx)
            .now_or_never()
            .is_none()
    );
    schedule.rendered(None);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert!(
        schedule
            .next_event(&mut events, &rx)
            .now_or_never()
            .is_none()
    );
}

#[tokio::test(start_paused = true)]
async fn terminal_events_wake_idle_ui_and_channel_closure_exits() {
    let mut schedule = RenderSchedule::new();
    let (tx, rx) = flume::unbounded();
    let (event_tx, mut events) = futures::channel::mpsc::unbounded();
    schedule.rendered(None);
    event_tx.unbounded_send(Ok(Event::Resize(120, 40))).unwrap();
    assert!(matches!(
        schedule.next_event(&mut events, &rx).await.unwrap(),
        UiEvent::Terminal(Event::Resize(120, 40))
    ));
    schedule.request();
    tokio::time::advance(FRAME_INTERVAL).await;
    assert!(matches!(
        schedule.next_event(&mut events, &rx).await.unwrap(),
        UiEvent::Render
    ));
    schedule.rendered(None);
    drop(tx);
    assert!(matches!(
        schedule.next_event(&mut events, &rx).await.unwrap(),
        UiEvent::Closed
    ));
    let (_tx, rx) = flume::unbounded();
    drop(event_tx);
    assert!(matches!(
        schedule.next_event(&mut events, &rx).await.unwrap(),
        UiEvent::Closed
    ));
}

#[tokio::test]
async fn terminal_errors_are_propagated_instead_of_spinning() {
    let mut schedule = RenderSchedule::new();
    let (_tx, rx) = flume::unbounded();
    let mut events = futures::stream::iter([Err(std::io::Error::other("terminal failed"))]);
    schedule.rendered(None);
    let error = schedule.next_event(&mut events, &rx).await.err().unwrap();
    assert!(error.to_string().contains("terminal failed"));
}

#[tokio::test(start_paused = true)]
async fn continuous_terminal_events_do_not_starve_actor_packets() {
    let mut schedule = RenderSchedule::new();
    schedule.rendered(None);
    let (tx, rx) = flume::unbounded();
    for _ in 0..256 {
        tx.send(packet("update")).unwrap();
    }
    let mut events = futures::stream::repeat_with(|| Ok(Event::FocusGained));
    let mut actor_updates = 0;
    let mut terminal_updates = 0;
    for _ in 0..256 {
        match schedule.next_event(&mut events, &rx).await.unwrap() {
            UiEvent::Actor(_) => actor_updates += 1,
            UiEvent::Terminal(_) => terminal_updates += 1,
            _ => panic!("Expected an input event"),
        }
    }
    assert!(actor_updates > 0);
    assert!(terminal_updates > 0);
}
