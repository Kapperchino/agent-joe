use super::*;
use commands::command::{Command, ResumeTarget};

#[tokio::test]
async fn tool_cycles_resume_and_forks_keep_runtime_history_and_isolate_cache_keys() {
    let workspace = crate::session::tests::Workspace::new();
    let runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
    let store = runtime.sessions.clone().unwrap();
    let (read, entered) = gate("read", ToolEffect::Read);
    let h = Harness::with_runtime(vec![read], runtime).await;
    h.start("Keep the public API unchanged");
    let (mut previous, mut reply) = h.request().await;
    let id = store.list().unwrap()[0].id.clone();
    assert_eq!(previous.prompt_cache_key.as_deref(), Some(id.as_str()));
    for index in 0..3 {
        answer(
            reply,
            response(vec![call("read", &format!("read-{index}"))]),
        );
        within(entered.recv_async())
            .await
            .unwrap()
            .1
            .send(())
            .unwrap();
        let (next, next_reply) = h.request().await;
        assert_eq!(
            serde_json::to_value(&next.messages[..previous.messages.len()]).unwrap(),
            serde_json::to_value(&previous.messages).unwrap()
        );
        assert_eq!(next.system, previous.system);
        assert_eq!(next.prompt_cache_key, previous.prompt_cache_key);
        assert_eq!(runtime_snapshot(&next.messages).evidence.len(), index + 1);
        crate::context::CompleteHistory::new(&next.messages).unwrap();
        previous = next;
        reply = next_reply;
    }
    answer(reply, response(vec![text("Inspected the files")]));
    h.terminal(Lifecycle::Completed).await;
    let saved = store
        .list()
        .unwrap()
        .into_iter()
        .find(|snapshot| snapshot.id == id)
        .unwrap();
    assert_eq!(
        runtime_snapshot(&saved.history),
        runtime_snapshot(&previous.messages)
    );
    h.stop().await;
    drop(store);
    let h = Harness::with_runtime(
        vec![],
        Runtime::for_workspace(workspace.path.clone()).unwrap(),
    )
    .await;
    h.actor
        .send_message(Message::Command(Command::Resume(ResumeTarget::Session {
            id: id.clone(),
        })))
        .unwrap();
    h.event(|packet| matches!(packet, ActorToTuiPacket::SessionResumed(Ok(_))))
        .await;
    h.start("Continue investigating");
    let (request, reply) = h.request().await;
    assert_eq!(request.prompt_cache_key, previous.prompt_cache_key);
    assert_eq!(
        serde_json::to_value(&request.messages[..previous.messages.len()]).unwrap(),
        serde_json::to_value(&previous.messages).unwrap()
    );
    answer(reply, response(vec![text("Investigation completed")]));
    h.terminal(Lifecycle::Completed).await;
    h.actor
        .send_message(Message::Command(Command::Fork))
        .unwrap();
    h.event(|packet| matches!(packet, ActorToTuiPacket::CommandResult(Command::Fork, _)))
        .await;
    h.start("Continue on the fork");
    let (fork, reply) = h.request().await;
    assert_ne!(fork.prompt_cache_key, previous.prompt_cache_key);
    assert!(fork.prompt_cache_key.is_some());
    assert_eq!(
        runtime_snapshot(&fork.messages),
        runtime_snapshot(&previous.messages)
    );
    answer(reply, response(vec![text("Fork finished")]));
    h.terminal(Lifecycle::Completed).await;
    h.actor
        .send_message(Message::Command(Command::New))
        .unwrap();
    h.event(|packet| matches!(packet, ActorToTuiPacket::CommandResult(Command::New, _)))
        .await;
    h.start("A new task");
    let (new, reply) = h.request().await;
    assert_ne!(new.prompt_cache_key, previous.prompt_cache_key);
    assert_ne!(new.prompt_cache_key, fork.prompt_cache_key);
    answer(reply, response(vec![text("New task finished")]));
    h.terminal(Lifecycle::Completed).await;
    h.stop().await;
}

#[tokio::test]
async fn quota_exhaustion_retains_history_and_cancels_queued_continuation() {
    for code in ["usage_limit_reached", "insufficient_quota"] {
        let workspace = crate::session::tests::Workspace::new();
        let runtime = Runtime::for_workspace(workspace.path.clone()).unwrap();
        let store = runtime.sessions.clone().unwrap();
        let h = Harness::with_runtime(vec![], runtime).await;
        h.start("Keep my unfinished task");
        let (request, reply) = h.request().await;
        let history = serde_json::to_value(h.history().await).unwrap();
        h.start("Queued follow-up");
        h.event(|packet| matches!(packet, ActorToTuiPacket::Queued { .. }))
            .await;
        let body =
            json!({"error":{"type":code,"message":"Usage exhausted","resets_in_seconds":7200}})
                .to_string();
        assert!(reply.send(Err(Failure::http(429, body).into())).is_ok());
        let event = h
            .event(|packet| {
                matches!(
                    packet,
                    ActorToTuiPacket::TurnChanged {
                        state: Lifecycle::Failed,
                        ..
                    }
                )
            })
            .await;
        assert!(
            matches!(event, ActorToTuiPacket::TurnChanged { detail: Some(detail), .. } if detail.contains("7200") && detail.contains("UsageLimit"))
        );
        assert!(h.requests.is_empty());
        assert_eq!(serde_json::to_value(h.history().await).unwrap(), history);
        let snapshot = store
            .list()
            .unwrap()
            .into_iter()
            .find(|snapshot| Some(&snapshot.id) == request.prompt_cache_key.as_ref())
            .unwrap();
        assert_eq!(snapshot.status, Lifecycle::Failed);
        assert!(snapshot.queued.is_empty());
        assert!(
            snapshot
                .history
                .iter()
                .any(|message| message.text() == "Keep my unfinished task")
        );
        h.stop().await;
    }
}

#[tokio::test]
async fn workers_receive_their_own_cache_key() {
    let (delegate, children) = delegate(vec![], false);
    let h = Harness::new(vec![delegate], Duration::from_secs(10)).await;
    h.start("Inspect the task");
    let (parent, reply) = h.request().await;
    answer(reply, response(vec![call("delegate", "child")]));
    let (child, reply) = within(children.recv_async()).await.unwrap();
    assert!(child.prompt_cache_key.is_some());
    assert_ne!(child.prompt_cache_key, parent.prompt_cache_key);
    answer(reply, response(vec![text("Inspection finished")]));
    let (next, reply) = h.request().await;
    assert_eq!(next.prompt_cache_key, parent.prompt_cache_key);
    answer(reply, response(vec![text("Done")]));
    h.terminal(Lifecycle::Completed).await;
    h.stop().await;
}
