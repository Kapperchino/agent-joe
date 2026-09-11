use super::*;
use crate::tool_defs::LenientDeserialize;
use serde_json::json;

#[test]
fn status_accepts_saved_calls_with_unused_fields() {
    for value in [
        json!({"operation":"status","path":"","revision":"","target":"","limit":0}),
        json!({"operation":"status","path":"","revision":"","target":"unstaged","limit":10}),
        json!({"operation":"status","path":"","revision":"","target":"","limit":20}),
    ] {
        let input = GitInput::deserialize_lenient(value).unwrap();
        assert!(matches!(
            GitOperation::try_from(input).unwrap(),
            GitOperation::Status
        ));
    }
}

#[test]
fn diff_accepts_saved_call_with_unused_fields() {
    let input = GitInput::deserialize_lenient(json!({
        "operation":"diff",
        "path":"src/actors/src/context.rs",
        "revision":"",
        "target":"unstaged",
        "limit":1
    }))
    .unwrap();
    assert!(matches!(
        GitOperation::try_from(input).unwrap(),
        GitOperation::Diff { target: DiffTarget::Unstaged, path: Some(path) }
            if path == PathBuf::from("src/actors/src/context.rs")
    ));
}

#[test]
fn omitted_null_and_empty_options_use_operation_defaults() {
    for options in [
        json!({}),
        json!({"path":null,"revision":null,"target":null,"limit":null}),
        json!({"path":"","revision":"","target":""}),
    ] {
        for operation in ["status", "diff", "show", "log"] {
            let mut value = options.clone();
            value["operation"] = json!(operation);
            let input = GitInput::deserialize_lenient(value).unwrap();
            match GitOperation::try_from(input).unwrap() {
                GitOperation::Status => assert_eq!(operation, "status"),
                GitOperation::Diff { target, path } => {
                    assert!(matches!(target, DiffTarget::Unstaged));
                    assert!(path.is_none());
                }
                GitOperation::Show { revision, path } => {
                    assert_eq!(serde_json::to_value(revision).unwrap(), json!("HEAD"));
                    assert!(path.is_none());
                }
                GitOperation::Log { revision, .. } => {
                    assert_eq!(serde_json::to_value(revision).unwrap(), json!("HEAD"));
                }
            }
        }
    }
}

#[test]
fn relevant_options_are_preserved_and_validated() {
    for target in ["staged", "unstaged", "head"] {
        let input = GitInput::deserialize_lenient(json!({
            "operation":"diff","target":target,"path":" file with spaces "
        }))
        .unwrap();
        match GitOperation::try_from(input).unwrap() {
            GitOperation::Diff {
                target: actual,
                path,
            } => {
                assert_eq!(serde_json::to_value(actual).unwrap(), json!(target));
                assert_eq!(path, Some(PathBuf::from(" file with spaces ")));
            }
            _ => panic!("Expected a diff operation"),
        }
    }
    for value in [
        json!({"operation":"commit"}),
        json!({"operation":"diff","target":"invalid"}),
        json!({"operation":"show","revision":"HEAD:file"}),
        json!({"operation":"log","revision":"--all"}),
        json!({"operation":"log","limit":0}),
        json!({"operation":"log","limit":101}),
    ] {
        let operation =
            GitInput::deserialize_lenient(value.clone()).and_then(GitOperation::try_from);
        assert!(operation.is_err(), "{value}");
    }
}
