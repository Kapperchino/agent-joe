use super::*;
use crate::openai::prompt_cache::RequestFingerprint;

enum CacheExpectation {
    Automatic,
    Breakpoints,
}

struct CacheRoute {
    auth: OpenAIAuthConfig,
    cache: CacheExpectation,
}

fn cached_request(config: &OpenAIConfig, input: Vec<InputItem>) -> ResponseRequest {
    let mut request = ClientRequest::new(input).with_instructions("Stable operating policy".into());
    request.prompt_cache_key = Some("session-1".into());
    ResponseRequest::new(config, request, ResponseMode::Streaming)
}

#[test]
fn cache_boundaries_follow_model_and_provider_support() {
    for route in [
        CacheRoute {
            auth: codex_auth(),
            cache: CacheExpectation::Automatic,
        },
        CacheRoute {
            auth: OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
                api_key: "fixture".into(),
                url: None,
            }),
            cache: CacheExpectation::Breakpoints,
        },
        CacheRoute {
            auth: OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
                api_key: "fixture".into(),
                url: Some("https://api.openai.com/v1/".into()),
            }),
            cache: CacheExpectation::Breakpoints,
        },
        CacheRoute {
            auth: OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
                api_key: "fixture".into(),
                url: Some("https://compatible.invalid/v1".into()),
            }),
            cache: CacheExpectation::Automatic,
        },
        CacheRoute {
            auth: OpenAIAuthConfig::Local(LocalOpenAIConfig {
                api_key: None,
                url: "http://localhost:1234/v1".into(),
            }),
            cache: CacheExpectation::Automatic,
        },
        CacheRoute {
            auth: OpenAIAuthConfig::OpenRouter(OpenRouterConfig {
                api_key: "fixture".into(),
                url: None,
            }),
            cache: CacheExpectation::Automatic,
        },
    ] {
        let config = config(route.auth);
        for model in [
            "gpt-6-astra",
            "gpt-5.6",
            "gpt-5.6-sol",
            "gpt-5.6-terra",
            "gpt-5.6-luna",
            "gpt-6-astra-2026-09-01",
        ] {
            let body = serde_json::to_value(ResponseRequest::new(
                &config,
                ClientRequest::new(vec![InputItem::user("task".into())]).with_model(model.into()),
                ResponseMode::Streaming,
            ))
            .unwrap();
            assert_eq!(body["model"], model);
            match route.cache {
                CacheExpectation::Breakpoints => {
                    assert_eq!(body["prompt_cache_options"], json!({"mode":"implicit"}));
                    assert_eq!(body["input"][0]["content"][0]["text"], "task");
                    assert_eq!(
                        body["input"][0]["content"][0]["prompt_cache_breakpoint"],
                        json!({"mode":"explicit"})
                    );
                }
                CacheExpectation::Automatic => {
                    assert!(body.get("prompt_cache_options").is_none());
                    assert_eq!(body["input"][0]["content"], "task");
                }
            }
        }
        for model in ["gpt-5.5", "gpt-5.4", "gpt-6-astral", "gpt-5.6-unknown"] {
            let mut config = config.clone();
            config.model = model.into();
            let body = wire_request(&config, ResponseMode::Streaming);
            assert!(body.get("prompt_cache_options").is_none());
            assert_eq!(body["input"][0]["content"], "task");
        }
    }
}

#[test]
fn artifact_cycles_keep_every_cached_prefix_identical_across_replay() {
    for route in [
        CacheRoute {
            auth: codex_auth(),
            cache: CacheExpectation::Automatic,
        },
        CacheRoute {
            auth: OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
                api_key: "fixture".into(),
                url: None,
            }),
            cache: CacheExpectation::Breakpoints,
        },
    ] {
        artifact_cycles(config(route.auth), route.cache);
    }
}

fn artifact_cycles(mut config: OpenAIConfig, cache: CacheExpectation) {
    config.model = "gpt-6-astra".into();
    config.effort = OpenAIEffort::Xhigh;
    let mut input = vec![InputItem::user("Review the complete task changes".into())];
    let mut previous = cached_request(&config, input.clone());
    for index in 0..85 {
        let call_id = NonEmptyString::try_from(format!("call-{index}")).unwrap();
        input.extend([
            InputItem::FunctionCall {
                id: NonEmptyString::try_from(format!("fc-{index}")).unwrap(),
                call_id: call_id.clone(),
                name: NonEmptyString::try_from("read_artifact".to_owned()).unwrap(),
                arguments: json!({"id":"review", "offset":index * 4096, "bytes":4096}).to_string(),
            },
            InputItem::function_call_output(call_id, format!("page {index}: {}", "x".repeat(4096))),
            InputItem::user(format!("Runtime state update: page {index} inspected")),
        ]);
        let request = cached_request(&config, input.clone());
        assert_eq!(
            serde_json::to_vec(&request.input[..previous.input.len()]).unwrap(),
            serde_json::to_vec(&previous.input).unwrap()
        );
        let old_hashes = RequestFingerprint::new(&previous).unwrap();
        let new_hashes = RequestFingerprint::new(&request).unwrap();
        assert_eq!(old_hashes.settings, new_hashes.settings);
        assert_eq!(
            old_hashes.prefixes,
            new_hashes.prefixes[..previous.input.len()]
        );
        let body = serde_json::to_value(&request).unwrap();
        let tool = &body["input"][body["input"].as_array().unwrap().len() - 2];
        let output = format!("page {index}: {}", "x".repeat(4096));
        match cache {
            CacheExpectation::Automatic => {
                assert_eq!(tool["output"], output);
                assert!(body.get("prompt_cache_options").is_none());
            }
            CacheExpectation::Breakpoints => {
                assert_eq!(tool["output"][0]["text"], output);
                assert_eq!(
                    tool["output"][0]["prompt_cache_breakpoint"],
                    json!({"mode":"explicit"})
                );
            }
        }
        assert_eq!(body["reasoning"]["effort"], "xhigh");
        assert_eq!(body["prompt_cache_key"], "session-1");
        previous = request;
    }
    let saved = serde_json::to_vec(&input).unwrap();
    let replay = cached_request(&config, serde_json::from_slice(&saved).unwrap());
    assert_eq!(
        serde_json::to_vec(&previous).unwrap(),
        serde_json::to_vec(&replay).unwrap()
    );
    let annotated = cached_request(&config, previous.input.clone());
    assert_eq!(
        serde_json::to_vec(&previous).unwrap(),
        serde_json::to_vec(&annotated).unwrap()
    );
}

#[test]
fn cache_annotations_preserve_reasoning_native_compaction_and_assistant_items() {
    let mut config = config(OpenAIAuthConfig::APIKey(OpenAIKeyConfig {
        api_key: "fixture".into(),
        url: None,
    }));
    config.model = "gpt-6-astra".into();
    let reasoning = json!({
        "type":"reasoning", "id":"rs-1", "summary":[],
        "encrypted_content":"opaque", "future":{"preserve":true}
    });
    let native = json!({"type":"compaction", "encrypted_content":"compacted", "future":[1,2]});
    let items = vec![
        InputItem::Native(native.clone()),
        serde_json::from_value(reasoning.clone()).unwrap(),
        InputItem::Message {
            role: Role::Assistant,
            content: "Reviewing the changes".to_owned().into(),
            phase: Some(llm::MessagePhase::Commentary),
        },
    ];
    let expected = serde_json::to_value(&items).unwrap();
    let request = cached_request(&config, items);
    assert_eq!(serde_json::to_value(&request.input).unwrap(), expected);
    assert_eq!(serde_json::to_value(&request.input[0]).unwrap(), native);
    assert_eq!(serde_json::to_value(&request.input[1]).unwrap(), reasoning);
}

#[test]
fn codex_requests_preserve_cache_keys_without_public_api_cache_fields() {
    let mut config = config(codex_auth());
    config.model = "gpt-6-astra".into();
    config.effort = OpenAIEffort::Xhigh;
    let input = json!([
        {"type":"message","role":"user","content":"Review the task changes"},
        {"type":"function_call","id":"fc-1","call_id":"call-1","name":"review_changes","arguments":"{}"},
        {"type":"function_call_output","call_id":"call-1","output":"Complete task diff"},
        {"type":"message","role":"user","content":"Runtime state update"}
    ]);
    for purpose in [
        llm::RequestPurpose::Conversation,
        llm::RequestPurpose::Worker,
        llm::RequestPurpose::Compaction,
        llm::RequestPurpose::Commit,
    ] {
        for mode in [ResponseMode::Complete, ResponseMode::Streaming] {
            let request = ClientRequest {
                prompt_cache_key: Some("session-1".into()),
                purpose,
                ..ClientRequest::new(serde_json::from_value(input.clone()).unwrap())
            };
            let body = serde_json::to_value(ResponseRequest::new(&config, request, mode)).unwrap();
            assert!(body.get("prompt_cache_options").is_none());
            assert_eq!(body["input"], input);
            assert_eq!(body["prompt_cache_key"], "session-1");
            assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
            assert_eq!(body["model"], "gpt-6-astra");
            assert_eq!(body["reasoning"]["effort"], "xhigh");
        }
    }
}

#[test]
fn fingerprints_distinguish_changed_settings_from_changed_history() {
    let mut config = config(codex_auth());
    config.model = "gpt-6-astra".into();
    let input = vec![
        InputItem::user("first".into()),
        InputItem::user("second".into()),
    ];
    let mut request = cached_request(&config, input);
    let original = RequestFingerprint::new(&request).unwrap();
    request.input[1] = InputItem::user("changed".into());
    let changed_history = RequestFingerprint::new(&request).unwrap();
    assert_eq!(original.settings, changed_history.settings);
    assert_eq!(original.prefixes[0], changed_history.prefixes[0]);
    assert_ne!(original.prefixes[1], changed_history.prefixes[1]);
    request.instructions.push_str("changed instructions");
    let changed_settings = RequestFingerprint::new(&request).unwrap();
    assert_ne!(original.settings, changed_settings.settings);
    assert_ne!(original.prefixes[0], changed_settings.prefixes[0]);
}

#[test]
fn tool_schema_serialization_uses_stable_property_order() {
    use tools::tool_defs::{ToolDefinition, ToolProperty};
    let names = ["path", "offset", "bytes", "pattern", "include", "exclude"];
    let tools = [names.to_vec(), names.into_iter().rev().collect()]
        .into_iter()
        .map(|names| {
            let tool: Tool = ToolDefinition::Client {
                name: "inspect".into(),
                description: "Inspect an artifact".into(),
                properties: names
                    .into_iter()
                    .map(|name| (name.into(), ToolProperty::Schema(json!({"type":"string"}))))
                    .collect(),
                required: vec!["path".into()],
            }
            .into();
            serde_json::to_string(&tool).unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(tools[0], tools[1]);
    assert!(tools[0].contains(
        r#""properties":{"bytes":{"type":"string"},"exclude":{"type":"string"},"include":{"type":"string"},"offset":{"type":"string"},"path":{"type":"string"},"pattern":{"type":"string"}}"#
    ));
}

#[test]
fn cache_metrics_handle_write_tokens_and_old_responses_without_double_counting() {
    let usage: Usage = serde_json::from_value(json!({
        "input_tokens":1000, "output_tokens":200,
        "input_tokens_details":{"cached_tokens":800,"cache_write_tokens":150}
    }))
    .unwrap();
    assert_eq!(usage.cache_hit_percent(), 80.0);
    assert_eq!(usage.input_tokens_details.cache_write_tokens, 150);
    let delta = llm::UsageDelta::from(usage);
    assert_eq!(delta.input_tokens + delta.output_tokens, 1200);
    let old: Usage = serde_json::from_value(json!({"input_tokens":1000})).unwrap();
    assert_eq!(old.cache_hit_percent(), 0.0);
    assert_eq!(old.input_tokens_details.cache_write_tokens, 0);
    assert_eq!(Usage::default().cache_hit_percent(), 0.0);
}
