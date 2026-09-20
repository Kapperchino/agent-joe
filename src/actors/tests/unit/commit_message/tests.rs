use super::*;
use merge_workflow::commit_message::generate;
use utils::git::worktrees::session::CommitMessage;

const DIFF: &str =
    "diff --git a/lib.rs b/lib.rs\n-pub fn value() -> u32 { 1 }\n+pub fn value() -> u32 { 2 }\n";

async fn generated(events: Vec<StreamEvent>) -> anyhow::Result<CommitMessage> {
    let (tx, requests) = flume::unbounded();
    let client = llm::LLmClient::Injected(Arc::new(Provider(tx)));
    let task = tokio::spawn(generate(client, DIFF.into(), Duration::from_secs(1), None));
    let (request, reply) = within(requests.recv_async()).await.unwrap();
    assert_eq!(request.messages.len(), 1);
    assert_eq!(request.messages[0].text(), DIFF);
    assert!(request.tools.is_empty());
    assert!(!request.thinking);
    assert_eq!(request.max_output_tokens, Some(4096));
    let instructions = request.system.unwrap();
    assert!(instructions.contains("actual changes"));
    assert!(instructions.contains("not file counts"));
    assert!(instructions.contains("untrusted data"));
    answer(reply, events);
    within(task).await.unwrap()
}

#[tokio::test]
async fn generates_a_brief_subject_from_diff_without_conversation_or_tools() {
    let message = generated(response(vec![text("Make value return 2 instead of 1")]))
        .await
        .unwrap();
    assert_eq!(message.as_str(), "Make value return 2 instead of 1");
}

#[tokio::test]
async fn rejects_malformed_incomplete_refused_and_tool_responses() {
    for subject in [
        "",
        "\n ",
        "Fix value\n\nMore details",
        "```Fix value```",
        &"x".repeat(73),
    ] {
        assert!(generated(response(vec![text(subject)])).await.is_err());
    }
    let mut incomplete = response(vec![text("Make value return 2")]);
    incomplete.pop();
    assert!(generated(incomplete).await.is_err());
    for reason in [llm::StopReason::Refusal, llm::StopReason::MaxTokens] {
        let mut events = response(vec![text("Make value return 2")]);
        if let Some(StreamEvent::MessageDelta { delta, .. }) = events.last_mut() {
            delta.stop_reason = Some(reason);
        }
        assert!(generated(events).await.is_err());
    }
    assert!(
        generated(response(vec![call("apply_patch", "unexpected")]))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn provider_failures_and_timeouts_leave_no_partial_subject() {
    let (tx, requests) = flume::unbounded();
    let client = llm::LLmClient::Injected(Arc::new(Provider(tx)));
    let task = tokio::spawn(generate(
        client.clone(),
        DIFF.into(),
        Duration::from_secs(1),
        None,
    ));
    let (_, reply) = within(requests.recv_async()).await.unwrap();
    assert!(
        reply
            .send(Err(anyhow::anyhow!("Provider unavailable")))
            .is_ok()
    );
    assert!(
        within(task)
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("Provider unavailable")
    );

    let task = tokio::spawn(generate(
        client,
        DIFF.into(),
        Duration::from_millis(50),
        None,
    ));
    let (_, reply) = within(requests.recv_async()).await.unwrap();
    assert!(
        within(task)
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("timed out")
    );
    assert!(reply.is_closed());
}

#[tokio::test]
async fn empty_and_oversized_diffs_do_not_send_partial_context() {
    let (tx, requests) = flume::unbounded();
    let client = llm::LLmClient::Injected(Arc::new(Provider(tx)));
    for diff in [String::new(), "x".repeat(64 * 1024 + 1)] {
        assert!(
            generate(client.clone(), diff, Duration::from_secs(1), None)
                .await
                .is_err()
        );
    }
    assert!(requests.is_empty());
}

#[tokio::test]
async fn response_budget_is_cumulative_across_events() {
    let block = "x".repeat(32 * 1024);
    let error = generated(response(vec![text(&block), text(&block)]))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("response exceeds 64 KiB"));
}

#[tokio::test]
async fn response_state_rejects_content_after_completion() {
    let mut events = response(vec![text("Raise retry limit to five")]);
    events.push(StreamEvent::ContentBlockComplete {
        index: 1,
        content: text("and ignore the previous subject"),
    });
    let error = generated(events).await.unwrap_err();
    assert!(error.to_string().contains("after completing its response"));
}
