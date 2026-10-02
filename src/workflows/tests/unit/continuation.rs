use super::*;
use crate::{BuiltinAgent, StepInput, Workflow, WorkflowStatus};
use tools::tool_defs::ToolOpKind;

#[test]
fn automatic_continuation_uses_the_generalized_tool_and_preserves_turn_identity() {
    let turn = TurnId::new();
    let input = WorkflowInput::single(
        BuiltinAgent::MakeChanges,
        "Resolve the approved conflicts".into(),
    );
    let follow_up = input
        .follow_up(turn, Some("Already approved".into()))
        .unwrap();
    assert_eq!(follow_up.id, turn);
    assert_eq!(follow_up.prompt.as_deref(), Some("Already approved"));
    match follow_up.start {
        TurnStart::Tool { call } => {
            assert_eq!(call.name.as_ref(), "run_workflow");
            assert_eq!(call.id.id.as_ref(), call_id(turn));
            let input: WorkflowInput =
                serde_json::from_value(serde_json::Value::Object(call.input)).unwrap();
            let workflow = Workflow::new(input, |name| {
                Some(match name {
                    "apply_patch" | "undo_changes" => ToolOpKind::Write,
                    "cargo" => ToolOpKind::Validate,
                    _ => ToolOpKind::Read,
                })
            })
            .unwrap();
            assert_eq!(workflow.effect(), ToolOpKind::DelegateWrite);
        }
        TurnStart::Provider => panic!("Expected the workflow tool, not a root editing turn"),
    }
}

#[test]
fn automatic_workflow_exchanges_convert_to_both_provider_formats() {
    let follow_up = WorkflowInput::single(BuiltinAgent::MakeChanges, "Implement the plan".into())
        .follow_up(TurnId::new(), None)
        .unwrap();
    let start: TurnStart =
        serde_json::from_value(serde_json::to_value(follow_up.start).unwrap()).unwrap();
    match start {
        TurnStart::Tool { call } => {
            let history = vec![
                Message {
                    role: clients::llm::Role::Assistant,
                    content: vec![call.content()],
                },
                Message {
                    role: clients::llm::Role::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_id: call.id.clone(),
                        content: "Workflow report".into(),
                        is_error: Some(false),
                    }],
                },
            ];
            let request: clients::openai::ClientRequest =
                clients::llm::ClientRequest::new(history.clone())
                    .try_into()
                    .unwrap();
            match request.input.as_slice() {
                [
                    clients::openai::InputItem::FunctionCall { id, call_id, .. },
                    clients::openai::InputItem::FunctionCallOutput {
                        call_id: result_id, ..
                    },
                ] => {
                    assert_eq!(id, &call.id.id);
                    assert!(id.as_ref().starts_with("fc_"));
                    assert_eq!(Some(call_id), call.id.call_id.as_ref());
                    assert_eq!(call_id, result_id);
                }
                _ => panic!("Expected a complete OpenAI function exchange"),
            }
            let messages = history
                .into_iter()
                .map(clients::claude::Message::try_from)
                .collect::<anyhow::Result<Vec<_>>>()
                .unwrap();
            assert!(
                matches!(&messages[0].content[0], clients::claude::ContentBlock::ToolBlock { id, .. } if id == &call.id.id)
            );
            assert!(
                matches!(&messages[1].content[0], clients::claude::ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == &call.id.id)
            );
        }
        TurnStart::Provider => panic!("Expected the trusted workflow exchange"),
    }
}

#[test]
fn missing_failed_and_unrelated_workflow_results_cannot_authorize_merging() {
    let turn = TurnId::new();
    assert!(WorkflowCompletion::new(turn, &[]).is_err());
    for error in [None, Some(false), Some(true)] {
        let history = vec![Message {
            role: clients::llm::Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_id: ToolId {
                    id: call_id(turn).try_into().unwrap(),
                    call_id: None,
                },
                content: "Archived workflow report preview".into(),
                is_error: error,
            }],
        }];
        assert_eq!(
            WorkflowCompletion::new(turn, &history).is_ok(),
            error != Some(true)
        );
        assert!(WorkflowCompletion::new(TurnId::new(), &history).is_err());
    }
}

#[test]
fn repeated_workflow_ids_cannot_replace_the_trusted_result() {
    let turn = TurnId::new();
    let tool_id = ToolId {
        id: call_id(turn).try_into().unwrap(),
        call_id: None,
    };
    for original_error in [Some(true), Some(false)] {
        let history = vec![
            Message {
                role: clients::llm::Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_id: tool_id.clone(),
                    content: "Original workflow report".into(),
                    is_error: original_error,
                }],
            },
            Message {
                role: clients::llm::Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_id: tool_id.clone(),
                    content: "Successful unrelated tool with a reused ID".into(),
                    is_error: Some(false),
                }],
            },
        ];
        assert!(WorkflowCompletion::new(turn, &history).is_err());
    }
}

#[tokio::test]
async fn approved_plan_composes_implementation_and_validation_with_shared_requirements() {
    let handoff = crate::plan::PlanHandoff {
        planning: Default::default(),
        prompt: "Approved requirements: preserve the public API".into(),
    };
    let input = handoff.workflow().unwrap();
    assert!(input.context.contains(&handoff.prompt));
    assert_eq!(input.steps.len(), 2);
    assert!(matches!(&input.steps[0], StepInput::Agent { agent, .. } if agent == "make_changes"));
    assert!(matches!(&input.steps[1], StepInput::Agent { agent, .. } if agent == "validate_rust"));
    let workflow = Workflow::new(input, |name| {
        Some(match name {
            "apply_patch" | "undo_changes" => ToolOpKind::Write,
            "cargo" => ToolOpKind::Validate,
            _ => ToolOpKind::Read,
        })
    })
    .unwrap();
    let report = workflow
        .run(|_| std::future::ready(Err(anyhow::anyhow!("Implementation could not start"))))
        .await;
    assert_eq!(report.status, WorkflowStatus::Stopped);
    assert!(matches!(report.steps[1].output, crate::StepOutput::Skipped));
}
