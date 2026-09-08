use super::*;

#[test]
fn selectors_cannot_inject_options() {
    for selector in [
        "--config=net.offline=false",
        "-p",
        "",
        "test\n--release",
        "../outside",
        "pkg *",
    ] {
        assert!(CargoSelector::new(selector).is_err(), "{selector}");
    }
}

#[test]
fn command_details_preserve_arguments_and_validate_combinations() {
    let input = CargoInput {
        package: Some("member".into()),
        features: vec!["gated".into()],
        target: Some(CargoTarget::Test {
            name: "regression".into(),
        }),
        test_name: Some("module::regression".into()),
        exact: true,
        ..Default::default()
    };
    let operation = CargoOperation::new(CargoAction::Test, input.clone()).unwrap();
    assert_eq!(
        operation.details().args,
        [
            "test",
            "--offline",
            "--message-format=json-diagnostic-rendered-ansi",
            "--package",
            "member",
            "--features",
            "gated",
            "--test",
            "regression",
            "module::regression",
            "--",
            "--exact"
        ]
    );
    assert!(CargoOperation::new(CargoAction::Check, input.clone()).is_err());
    assert!(
        CargoOperation::new(
            CargoAction::Test,
            CargoInput {
                workspace: true,
                ..input
            }
        )
        .is_err()
    );
    assert!(CargoOperation::new(CargoAction::Run, CargoInput::default()).is_err());
    assert!(
        CargoOperation::new(
            CargoAction::Format,
            CargoInput {
                release: true,
                ..Default::default()
            }
        )
        .is_err()
    );
    assert!(
        CargoOperation::new(
            CargoAction::Check,
            CargoInput {
                target_triple: Some("../target.json".into()),
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn program_values_cannot_change_the_executor() {
    for key in [
        "PATH",
        "HOME",
        "CARGO_HOME",
        "RUSTFLAGS",
        "RUSTC_WRAPPER",
        "DYLD_INSERT_LIBRARIES",
    ] {
        assert!(ProgramEnvironment::new(BTreeMap::from([(key.into(), "value".into())])).is_err());
    }
    let operation = CargoOperation::new(
        CargoAction::Run,
        CargoInput {
            target: Some(CargoTarget::Example {
                name: "server".into(),
            }),
            args: vec![
                "--config=net.offline=false".into(),
                "$(touch injected)".into(),
            ],
            environment: BTreeMap::from([("JOE_RUN_MODE".into(), "test".into())]),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        &operation.details().args[5..],
        ["--", "--config=net.offline=false", "$(touch injected)"]
    );
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
struct Fixture {
    root: std::path::PathBuf,
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("joe-m6-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("member/src")).unwrap();
        std::fs::create_dir_all(root.join("member/tests")).unwrap();
        std::fs::create_dir_all(root.join("member/examples")).unwrap();
        std::fs::create_dir_all(root.join("other/src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\", \"other\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        std::fs::write(root.join("member/Cargo.toml"), "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[features]\ngated = []\n").unwrap();
        std::fs::write(
            root.join("other/Cargo.toml"),
            "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("other/src/lib.rs"),
            "compile_error!(\"unrelated package\");\n",
        )
        .unwrap();
        std::fs::write(
            root.join("member/src/lib.rs"),
            "pub fn answer() -> u32 {\n    42\n}\n",
        )
        .unwrap();
        std::fs::write(root.join("member/tests/regression.rs"), "#[test]\nfn gated_regression() {\n    assert!(cfg!(feature = \"gated\"));\n    assert_eq!(member::answer(), 42);\n}\n").unwrap();
        std::fs::write(
            root.join("member/examples/server.rs"),
            r#"fn main() {
    use std::io::Write;
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "finite" => {
            println!("value={}", std::env::var("JOE_RUN_VALUE").unwrap());
            println!("args={:?}", std::env::args().skip(2).collect::<Vec<_>>());
        }
        "stderr" => {
            eprintln!("stderr-only failure");
            std::process::exit(23);
        }
        "huge" => {
            std::io::stdout().write_all(&vec![b'x'; 17 * 1024 * 1024]).unwrap();
        }
        _ => loop {
            println!("ready");
            std::io::stdout().flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(20));
        },
    }
}
"#,
        )
        .unwrap();
        Self {
            root: root.canonicalize().unwrap(),
        }
    }
    fn scope(&self) -> ExecutionScope {
        ExecutionScope::with_workspace(
            crate::workspace::WorkspacePolicy::workspace(self.root.clone()).unwrap(),
        )
    }
    async fn run(&self, action: CargoAction, input: CargoInput) -> CargoResult {
        let scope = self.scope();
        let result = scope
            .enter(CargoOperation::new(action, input).unwrap().execute())
            .await
            .unwrap();
        scope.finish().await;
        result
    }
    fn example(&self, mode: &str) -> CargoInput {
        CargoInput {
            package: Some("member".into()),
            target: Some(CargoTarget::Example {
                name: "server".into(),
            }),
            args: vec![mode.into()],
            ..Default::default()
        }
    }
}
#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn typed_tools_reproduce_fix_and_validate_a_feature_gated_workspace() {
    if crate::test_support::sandbox_available() {
        let fixture = Fixture::new();
        let targeted = CargoInput {
            package: Some("member".into()),
            target: Some(CargoTarget::Test {
                name: "regression".into(),
            }),
            test_name: Some("gated_regression".into()),
            exact: true,
            ..Default::default()
        };
        let failed = fixture.run(CargoAction::Test, targeted.clone()).await;
        assert_eq!(failed.exit_code, Some(101), "{failed:?}");
        assert!(failed.stdout.content.contains("gated_regression"));
        let passed = fixture
            .run(
                CargoAction::Test,
                CargoInput {
                    features: vec!["gated".into()],
                    ..targeted.clone()
                },
            )
            .await;
        assert!(!passed.is_error(), "{passed:?}");
        assert!(passed.stdout.content.contains("1 passed"));
        assert!(!passed.reused);
        std::fs::write(
            fixture.root.join("member/src/lib.rs"),
            "pub fn answer() -> u32 {\n    43\n}\n",
        )
        .unwrap();
        let failed_again = fixture
            .run(
                CargoAction::Test,
                CargoInput {
                    features: vec!["gated".into()],
                    ..targeted
                },
            )
            .await;
        assert!(failed_again.is_error());
        let broader = fixture
            .run(
                CargoAction::Check,
                CargoInput {
                    workspace: true,
                    ..Default::default()
                },
            )
            .await;
        assert!(broader.is_error());
        assert!(
            broader
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.message.contains("unrelated package"))
        );
        std::fs::write(fixture.root.join("Cargo.toml"), "[invalid manifest").unwrap();
        let malformed = fixture.run(CargoAction::Check, Default::default()).await;
        assert!(malformed.is_error());
        assert!(malformed.stderr.content.contains("error"));
        assert_eq!(malformed.command.args[0], "check");
        assert_eq!(malformed.workspace, fixture.root.display().to_string());
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn format_clippy_and_program_failures_keep_exact_evidence() {
    if crate::test_support::sandbox_available() {
        let fixture = Fixture::new();
        let selected = CargoInput {
            package: Some("member".into()),
            ..Default::default()
        };
        std::fs::write(
            fixture.root.join("member/src/lib.rs"),
            "pub fn answer()->u32{let value=42;value}\n",
        )
        .unwrap();
        assert!(
            fixture
                .run(CargoAction::CheckFormat, selected.clone())
                .await
                .is_error()
        );
        assert!(
            !fixture
                .run(CargoAction::Format, selected.clone())
                .await
                .is_error()
        );
        assert!(
            !fixture
                .run(CargoAction::CheckFormat, selected.clone())
                .await
                .is_error()
        );
        let lint = fixture
            .run(
                CargoAction::Clippy,
                CargoInput {
                    deny_warnings: true,
                    ..selected
                },
            )
            .await;
        assert!(lint.is_error(), "{lint:?}");
        assert!(!lint.diagnostics.is_empty());
        let mut input = fixture.example("finite");
        input
            .environment
            .insert("JOE_RUN_VALUE".into(), "literal value".into());
        input.args.push("$(touch unexpected)".into());
        let result = fixture.run(CargoAction::Run, input).await;
        assert!(!result.is_error(), "{result:?}");
        assert!(result.stdout.content.contains("value=literal value"));
        assert!(result.stdout.content.contains("$(touch unexpected)"));
        assert!(!fixture.root.join("unexpected").exists());
        let failed = fixture
            .run(CargoAction::Run, fixture.example("stderr"))
            .await;
        assert_eq!(failed.exit_code, Some(23), "{failed:?}");
        assert!(failed.stderr.content.contains("stderr-only failure"));
        let huge = fixture.run(CargoAction::Run, fixture.example("huge")).await;
        assert_eq!(huge.status, ProcessStatus::OutputLimit);
        assert_eq!(huge.stdout.content.len(), 16 * 1024 * 1024);
        let timeout = fixture
            .run(
                CargoAction::Run,
                CargoInput {
                    timeout_seconds: Some(1),
                    ..fixture.example("server")
                },
            )
            .await;
        assert_eq!(timeout.status, ProcessStatus::TimedOut);
        assert!(timeout.stdout.content.contains("ready"));
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn managed_targets_survive_tool_cleanup_poll_incrementally_and_stop_with_the_turn() {
    if crate::test_support::sandbox_available() {
        let fixture = Fixture::new();
        let turn = fixture.scope();
        let tool = turn.tool_child();
        let started = tool
            .enter(
                CargoOperation::new(CargoAction::Run, fixture.example("server"))
                    .unwrap()
                    .start(),
            )
            .await
            .unwrap();
        let id = started.process_id.unwrap();
        tool.finish().await;
        let process = turn.processes.get(&id).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while !process.output().stdout.contains("ready")
                && process.output().status == ProcessStatus::Running
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            process.output().status,
            ProcessStatus::Running,
            "{:?}",
            process.output()
        );
        let poll = turn.tool_child();
        let first = poll
            .enter(CargoResult::control(
                &id,
                Default::default(),
                ProcessAction::Poll,
            ))
            .await
            .unwrap();
        let offsets = OutputOffsets {
            stdout: first.stdout.next_offset,
            stderr: first.stderr.next_offset,
        };
        let second = poll
            .enter(CargoResult::control(&id, offsets, ProcessAction::Poll))
            .await
            .unwrap();
        assert_eq!(second.stdout.offset, first.stdout.next_offset);
        assert!(
            fixture
                .scope()
                .enter(CargoResult::control(
                    &id,
                    Default::default(),
                    ProcessAction::Stop
                ))
                .await
                .is_err()
        );
        let stopped = poll
            .enter(CargoResult::control(
                &id,
                Default::default(),
                ProcessAction::Stop,
            ))
            .await
            .unwrap();
        assert_eq!(stopped.status, ProcessStatus::Cancelled);
        assert!(turn.resources().is_empty());
        assert_eq!(
            poll.enter(CargoResult::control(
                &id,
                Default::default(),
                ProcessAction::Stop
            ))
            .await
            .unwrap()
            .status,
            ProcessStatus::Cancelled
        );
        poll.finish().await;
        let started = turn
            .enter(
                CargoOperation::new(CargoAction::Run, fixture.example("server"))
                    .unwrap()
                    .start(),
            )
            .await
            .unwrap();
        let process = turn
            .processes
            .get(started.process_id.as_deref().unwrap())
            .unwrap();
        turn.finish().await;
        assert_eq!(process.output().status, ProcessStatus::Cancelled);
        assert!(turn.resources().is_empty());
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn restricted_workers_cannot_bypass_paths_through_cargo_execution() {
    let fixture = Fixture::new();
    let scope = fixture
        .scope()
        .restricted_child(
            &[std::path::PathBuf::from("member")],
            crate::workspace::RootAccess::ReadWrite,
        )
        .unwrap();
    let checked = scope
        .enter(
            CargoOperation::new(CargoAction::Check, CargoInput::default())
                .unwrap()
                .execute(),
        )
        .await
        .unwrap();
    assert_eq!(checked.status, ProcessStatus::Failed);
    assert!(checked.error.unwrap().contains("whole workspace"));
    let started = scope
        .enter(
            CargoOperation::new(CargoAction::Run, fixture.example("server"))
                .unwrap()
                .start(),
        )
        .await
        .unwrap();
    assert_eq!(started.status, ProcessStatus::Failed);
    assert!(started.error.unwrap().contains("whole workspace"));
    assert!(scope.resources().is_empty());
    scope.finish().await;
}
