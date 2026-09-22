use super::*;
use utils::power::IdleSleep;

async fn assert_idle_sleep(name: &str, expected: IdleSleep) {
    let output = tokio::process::Command::new("/usr/bin/pmset")
        .args(["-g", "assertions"])
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    let assertions = String::from_utf8(output.stdout).unwrap();
    match expected {
        IdleSleep::Prevented => assert!(assertions.contains(name), "{assertions}"),
        IdleSleep::Allowed => assert!(!assertions.contains(name), "{assertions}"),
    }
}

async fn assert_actor_idle_sleep(h: &Harness, expected: IdleSleep) {
    h.history().await;
    let name = format!("Joe actor {} is working", h.actor.get_id());
    assert_idle_sleep(&name, expected).await;
}

#[tokio::test]
async fn idle_sleep_is_prevented_during_requests_retries_and_tools() {
    let (read, entered) = gate("read", ToolOpKind::Read);
    let h = Harness::new(vec![read], Duration::from_secs(10)).await;
    assert_actor_idle_sleep(&h, IdleSleep::Allowed).await;
    h.start("work");
    let (_, reply) = h.request().await;
    assert_actor_idle_sleep(&h, IdleSleep::Prevented).await;
    assert!(
        reply
            .send(Err(Failure::new(
                FailureKind::Transport,
                "connection reset"
            )
            .into()))
            .is_ok()
    );
    h.event(|packet| {
        matches!(packet, ActorToTuiPacket::TurnChanged {
        state: Lifecycle::Running, detail: Some(detail), ..
    } if detail.contains("Retrying"))
    })
    .await;
    assert_actor_idle_sleep(&h, IdleSleep::Prevented).await;
    let (_, reply) = h.request().await;
    answer(reply, response(vec![call("read", "read-1")]));
    let (_, pending) = within(entered.recv_async()).await.unwrap();
    assert_actor_idle_sleep(&h, IdleSleep::Prevented).await;
    pending.send(()).unwrap();
    let (_, reply) = h.request().await;
    answer(reply, response(vec![text("done")]));
    h.terminal(Lifecycle::Completed).await;
    assert_actor_idle_sleep(&h, IdleSleep::Allowed).await;
    h.stop().await;
}

#[tokio::test]
async fn idle_sleep_hold_is_released_on_failure_cancellation_and_actor_shutdown() {
    let h = Harness::new(vec![], Duration::from_secs(10)).await;
    let name = format!("Joe actor {} is working", h.actor.get_id());
    h.start("work");
    let (_, reply) = h.request().await;
    assert_actor_idle_sleep(&h, IdleSleep::Prevented).await;
    assert!(
        reply
            .send(Err(Failure::new(
                FailureKind::Authentication,
                "unauthorized"
            )
            .into()))
            .is_ok()
    );
    h.terminal(Lifecycle::Failed).await;
    assert_actor_idle_sleep(&h, IdleSleep::Allowed).await;

    h.start("retry");
    let (_, reply) = h.request().await;
    assert_actor_idle_sleep(&h, IdleSleep::Prevented).await;
    h.actor.send_message(Message::Interrupt).unwrap();
    h.terminal(Lifecycle::Cancelled).await;
    assert!(reply.is_closed());
    assert_actor_idle_sleep(&h, IdleSleep::Allowed).await;

    h.start("work again");
    let (_, reply) = h.request().await;
    assert_actor_idle_sleep(&h, IdleSleep::Prevented).await;
    h.stop().await;
    assert!(reply.is_closed());
    assert_idle_sleep(&name, IdleSleep::Allowed).await;
}

#[tokio::test]
async fn idle_sleep_is_allowed_while_waiting_for_a_required_answer() {
    let h = Harness::new(vec![], Duration::from_secs(10)).await;
    h.start("work");
    answer(
        h.request().await.1,
        response(vec![ContentBlock::ToolBlock {
            tool_id: ToolId {
                id: "ask".to_owned().try_into().unwrap(),
                call_id: None,
            },
            name: "request_user_input".to_owned().try_into().unwrap(),
            input: json!({"id":"target", "prompt":"Which target?", "required":true})
                .as_object()
                .unwrap()
                .clone(),
        }]),
    );
    h.event(|packet| {
        matches!(
            packet,
            ActorToTuiPacket::TurnChanged {
                state: Lifecycle::WaitingForInput,
                ..
            }
        )
    })
    .await;
    assert_actor_idle_sleep(&h, IdleSleep::Allowed).await;
    h.actor
        .send_message(Message::Command(
            commands::command::Command::parse("answer target text library").unwrap(),
        ))
        .unwrap();
    let (_, reply) = h.request().await;
    assert_actor_idle_sleep(&h, IdleSleep::Prevented).await;
    answer(reply, response(vec![text("done")]));
    h.terminal(Lifecycle::Completed).await;
    assert_actor_idle_sleep(&h, IdleSleep::Allowed).await;
    h.stop().await;
}
