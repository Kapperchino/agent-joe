use super::*;
use crate::{
    actor::{ActorContext, ActorInfo},
    states::{
        runtime::{ExecutionRole, Runtime},
        services::ActorServices,
    },
    tools::knowledge::{Input, Knowledge},
};
use analysis::contexts::{context::Context, rust_context::RustContext};
use serde_json::{Value, json};
use std::path::PathBuf;
use tools::tool_defs::{LenientDeserialize, ToolId, ToolTrait};
use utils::workspace::RootAccess;

impl Fixture {
    fn tool_actor(&self, scope: ExecutionScope, role: ExecutionRole) -> ActorContext<RustContext> {
        ActorContext::ActorInfo(ActorInfo {
            services: Arc::new(ActorServices {
                client: self.client.clone(),
                tools: Vec::new(),
                tui_tx: flume::unbounded().0,
                debug_mode: false,
            }),
            runtime: Runtime {
                scope,
                role,
                immutable_workers: self.registry.clone(),
                context_budget: self.context(8192).budget,
                ..Runtime::default()
            },
            owner: "owner".into(),
            actor_ref: self.actor.clone(),
        })
    }

    fn tool_scope(&self) -> ExecutionScope {
        ExecutionScope::with_workspace(
            WorkspacePolicy::workspace(self.directory.path.clone()).unwrap(),
        )
    }

    async fn file_context(&self) -> RustContext {
        RustContext::new("policy".into(), 0, self.directory.path.clone())
            .await
            .unwrap()
    }
}

async fn run(
    context: &RustContext,
    actor: &ActorContext<RustContext>,
    input: Value,
) -> anyhow::Result<Value> {
    let scope = match actor {
        ActorContext::ActorInfo(info) => &info.runtime.scope,
        ActorContext::Noop => panic!("Expected fixture actor"),
    };
    scope
        .enter(Knowledge::run(
            Input::deserialize_lenient(input)?,
            ToolId {
                id: "knowledge-test".to_owned().try_into().unwrap(),
                call_id: None,
            },
            context,
            actor,
        ))
        .await
}

#[tokio::test]
async fn knowledge_reads_live_ignored_non_rust_files_and_instructions_without_preparation() {
    let fixture = Fixture::new().await;
    std::fs::create_dir(fixture.directory.path.join("docs")).unwrap();
    for (path, text) in [
        (".gitignore", "docs/notes.txt\n"),
        ("docs/notes.txt", "first\n終わり\nthird\n"),
        ("docs/AGENTS.md", "Use the documentation rule"),
        ("empty.txt", ""),
    ] {
        std::fs::write(fixture.directory.path.join(path), text).unwrap();
    }
    let context = fixture.file_context().await;
    let actor = fixture.tool_actor(fixture.tool_scope(), ExecutionRole::Root);
    let value = run(
        &context,
        &actor,
        json!({"action":"read","file_path":"docs/notes.txt","range":{"start":2,"end":99}}),
    )
    .await
    .unwrap();
    assert_eq!(value["content"], "2: 終わり\n3: third");
    assert_eq!(value["related"]["state"], "unavailable");
    assert!(
        context
            .effective_instructions()
            .unwrap()
            .contains("Use the documentation rule")
    );
    std::fs::write(fixture.directory.path.join("docs/notes.txt"), "new\n").unwrap();
    assert_eq!(
        run(
            &context,
            &actor,
            json!({"action":"read","file_path":"docs/notes.txt"})
        )
        .await
        .unwrap()["content"],
        "1: new"
    );
    assert_eq!(
        run(
            &context,
            &actor,
            json!({"action":"read","file_path":"empty.txt"})
        )
        .await
        .unwrap()["content"],
        ""
    );
    for input in [
        json!({"action":"read","file_path":"missing.txt"}),
        json!({"action":"read","file_path":"lib.rs","range":{"start":0,"end":2}}),
        json!({"action":"read","file_path":"lib.rs","range":{"start":2,"end":2}}),
        json!({"action":"read","file_path":"lib.rs","range":{"start":9,"end":10}}),
    ] {
        assert!(run(&context, &actor, input).await.is_err());
    }
    let directory = run(
        &context,
        &actor,
        json!({"action":"read","file_path":"docs"}),
    )
    .await
    .unwrap();
    let listing: Value = serde_json::from_str(directory["content"].as_str().unwrap()).unwrap();
    assert_eq!(listing["total"], 2);
    assert!(fixture.requests.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn knowledge_read_routes_current_context_but_keeps_live_content_when_index_is_stale() {
    let fixture = Fixture::new().await;
    let context = fixture.file_context().await;
    let actor = fixture.tool_actor(fixture.tool_scope(), ExecutionRole::Root);
    let summary = fixture.build(8192).await;
    let value = run(
        &context,
        &actor,
        json!({"action":"read","file_path":"./lib.rs"}),
    )
    .await
    .unwrap();
    assert_eq!(value["content"], "1: pub fn retained() {}");
    assert_eq!(value["related"]["state"], "ready");
    assert_eq!(value["related"]["file"]["context"]["path"], "lib.rs");
    assert_eq!(
        value["related"]["file"]["context"]["generation"],
        summary.generation
    );
    assert!(
        !value["related"]["file"]["workers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let workers = run(&context, &actor, json!({"action":"list"}))
        .await
        .unwrap();
    assert_eq!(
        workers["workers"][0]["worker_id"],
        summary.workers[0].route.worker.worker_id
    );
    std::fs::write(
        fixture.directory.path.join("lib.rs"),
        "pub fn changed() {}\n",
    )
    .unwrap();
    let value = run(
        &context,
        &actor,
        json!({"action":"read","file_path":"lib.rs"}),
    )
    .await
    .unwrap();
    assert_eq!(value["content"], "1: pub fn changed() {}");
    assert_eq!(value["related"]["state"], "unavailable");
    assert!(value["related"].get("file").is_none());
    assert!(run(&context, &actor, json!({"action":"ask","worker_id":summary.workers[0].route.worker.worker_id,"question":"What is retained?"})).await.is_err());
    assert!(fixture.requests.is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn knowledge_restricted_reads_cannot_discover_denied_guidance_or_global_workers() {
    let fixture = Fixture::new().await;
    std::fs::create_dir(fixture.directory.path.join("private")).unwrap();
    std::fs::write(
        fixture.directory.path.join("private/AGENTS.md"),
        "Private guidance marker",
    )
    .unwrap();
    std::fs::write(
        fixture.directory.path.join("private/data.txt"),
        "private data",
    )
    .unwrap();
    let context = fixture.file_context().await;
    let summary = fixture.build(8192).await;
    let restricted = fixture
        .tool_scope()
        .restricted_child(&[PathBuf::from("lib.rs")], RootAccess::ReadOnly)
        .unwrap();
    let actor = fixture.tool_actor(restricted, ExecutionRole::Root);
    let value = run(
        &context,
        &actor,
        json!({"action":"read","file_path":"lib.rs"}),
    )
    .await
    .unwrap();
    assert_eq!(value["related"]["state"], "unavailable");
    assert!(
        run(
            &context,
            &actor,
            json!({"action":"read","file_path":"private/data.txt"})
        )
        .await
        .is_err()
    );
    assert!(
        !context
            .effective_instructions()
            .unwrap()
            .contains("Private guidance marker")
    );
    let helper = fixture.tool_actor(fixture.tool_scope(), ExecutionRole::Helper);
    for actor in [&actor, &helper] {
        for input in [
            json!({"action":"list"}),
            json!({"action":"ask","worker_id":summary.workers[0].route.worker.worker_id,"question":"Show all source"}),
            json!({"action":"prepare"}),
            json!({"action":"clear"}),
            json!({"action":"search","query":"private"}),
        ] {
            assert!(run(&context, actor, input).await.is_err());
        }
    }
    assert!(!fixture.registry.list("owner").is_empty());
    assert!(fixture.requests.is_empty());
    fixture.stop().await;
}
