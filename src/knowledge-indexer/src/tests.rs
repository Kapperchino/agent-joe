use ra_ap_ide_db::{RootDatabase, base_db::SourceDatabase};
use ra_ap_test_fixture::WithFixture;

use crate::*;

fn graph(fixture: &str) -> SemanticGraph {
    let (db, files) = RootDatabase::with_many_files(fixture);
    let sources = files
        .into_iter()
        .map(|file| {
            let file_id = file.file_id(&db);
            let root = db.file_source_root(file_id).source_root_id(&db);
            let root = db.source_root(root).source_root(&db);
            let path = root.path_for_file(&file_id).unwrap().to_string();
            let text = db.file_text(file_id).text(&db).to_string();
            IndexedSource {
                file_id: Some(file_id),
                source: SourceFile::new(
                    path.trim_start_matches('/').to_string().try_into().unwrap(),
                    text,
                )
                .unwrap(),
            }
        })
        .collect();
    extract(
        &db,
        sources,
        SemanticProfile {
            manifest: "Cargo.toml".to_string().try_into().unwrap(),
            target: "x86_64-unknown-linux-gnu".into(),
            features: Features::Default,
            configurations: [Configuration::Normal].into(),
            analyzer_version: "0.0.344".into(),
        },
        Configuration::Normal,
    )
    .unwrap()
}

fn edges<'a>(graph: &'a SemanticGraph, source: &str, kind: RelationKind) -> Vec<&'a Symbol> {
    let data = graph.data();
    data.relations
        .iter()
        .filter(|relation| {
            relation.kind == kind
                && data
                    .symbols
                    .iter()
                    .any(|symbol| symbol.id == relation.source && symbol.name == source)
        })
        .flat_map(|relation| relation.target.symbols())
        .filter_map(|id| data.symbols.iter().find(|symbol| &symbol.id == id))
        .collect()
}

#[test]
fn graph_protocol_rejects_invalid_paths_spans_and_relationships() {
    for path in [
        "",
        "/absolute",
        "../escape",
        "a/./b",
        "a//b",
        "C:/file",
        "a\\b",
    ] {
        assert!(SourcePath::try_from(path.to_owned()).is_err());
    }
    assert!(ByteSpan::new(5, 4).is_err());
    assert!(ByteSpan::new(0, 1).unwrap().text("🦀").is_err());
    assert_eq!(ByteSpan::new(2, 4).unwrap().lines("a\nb\n").unwrap().end, 3);
    let data = GraphData::from(graph("fn main() {}"));
    let mut duplicate = data.clone();
    duplicate.sources.push(duplicate.sources[0].clone());
    assert!(SemanticGraph::try_from(duplicate).is_err());
    let mut unknown = data.clone();
    unknown.relations.push(Relation {
        source: unknown.symbols[0].id.clone(),
        kind: RelationKind::Calls,
        target: RelationTarget::Resolved(SymbolId("missing".into())),
        site: None,
    });
    assert!(SemanticGraph::try_from(unknown).is_err());
    let mut invalid = data;
    invalid.symbols[0].origin = SymbolOrigin::Source {
        location: SourceLocation {
            path: invalid.sources[0].path().clone(),
            span: ByteSpan::new(0, 1000).unwrap(),
        },
    };
    assert!(
        serde_json::from_value::<SemanticGraph>(serde_json::to_value(invalid).unwrap()).is_err()
    );
}

#[test]
fn graph_round_trip_preserves_utf8_source() {
    let mut data = GraphData::from(graph("fn main() {}"));
    data.sources.push(
        SourceFile::new(
            "README.md".to_owned().try_into().unwrap(),
            "🦀\n".repeat(1024),
        )
        .unwrap(),
    );
    let graph = SemanticGraph::try_from(data).unwrap();
    let bytes = serde_json::to_vec(&graph).unwrap();
    let restored: SemanticGraph = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(restored.data().sources, graph.data().sources);
}

#[test]
fn cross_file_aliases_resolve_without_confusing_references_and_calls() {
    let graph = graph(
        r#"
//- /main.rs crate:app deps:library
mod other;
use library::run as execute;
fn main() { let callback = execute; execute(); other::run(); }
//- /other.rs
pub fn run() {}
//- /library.rs crate:library
pub fn run() {}
"#,
    );
    let calls = edges(&graph, "main", RelationKind::Calls);
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .any(|symbol| symbol.qualified_name.starts_with("library::"))
    );
    assert!(
        calls
            .iter()
            .any(|symbol| symbol.qualified_name.contains("other::run"))
    );
    assert_eq!(
        edges(&graph, "main", RelationKind::References)
            .iter()
            .filter(|symbol| symbol.name == "run")
            .count(),
        3
    );
}

#[test]
fn trait_implementations_and_generic_dispatch_are_distinct() {
    let graph = graph(
        r#"
trait Run { fn run(&self); }
struct Engine;
impl Run for Engine { fn run(&self) {} }
fn generic<T: Run>(value: &T) { value.run(); }
fn concrete(value: &Engine) { value.run(); }
"#,
    );
    assert_eq!(
        edges(&graph, "impl", RelationKind::Implements)[0].name,
        "Run"
    );
    assert_eq!(
        edges(&graph, "impl", RelationKind::ImplForType)[0].name,
        "Engine"
    );
    assert_eq!(edges(&graph, "run", RelationKind::Overrides).len(), 1);
    assert_eq!(
        edges(&graph, "generic", RelationKind::DispatchesToTrait).len(),
        1
    );
    assert!(edges(&graph, "generic", RelationKind::Calls).is_empty());
}

#[test]
fn unresolved_and_inactive_source_is_preserved() {
    let graph = graph("#[cfg(missing)] fn hidden() {}\nfn main() { missing(); }");
    assert!(graph.data().sources[0].text().contains("fn hidden"));
    assert!(
        graph
            .data()
            .relations
            .iter()
            .any(|relation| relation.kind == RelationKind::Calls
                && matches!(relation.target, RelationTarget::Unresolved(_)))
    );
}

#[test]
fn generated_definitions_keep_provenance_and_stable_identities() {
    let source = r#"
macro_rules! generate { () => { fn first() {} fn second() { first(); } } }
generate!();
fn main() { second(); }
"#;
    let first = graph(source);
    let second = graph(source);
    assert_eq!(
        serde_json::to_value(&first).unwrap(),
        serde_json::to_value(&second).unwrap()
    );
    let generated = first
        .data()
        .symbols
        .iter()
        .filter(|symbol| matches!(symbol.name.as_str(), "first" | "second"))
        .collect::<Vec<_>>();
    assert_eq!(generated.len(), 2);
    assert_ne!(generated[0].id, generated[1].id);
    assert!(
        generated
            .iter()
            .all(|symbol| matches!(symbol.origin, SymbolOrigin::Expansion { .. }))
    );
    assert_eq!(
        edges(&first, "second", RelationKind::Calls)[0].name,
        "first"
    );
}

#[test]
fn traversal_inherits_local_scopes_and_restores_sibling_owners() {
    let graph = graph(
        r#"
fn leaf() {}
fn outer(argument: usize) {
    let local = leaf();
    fn nested() { leaf(); }
    let callback = || leaf();
    nested();
}
fn sibling() { leaf(); }
"#,
    );
    let mut calls = edges(&graph, "outer", RelationKind::Calls)
        .into_iter()
        .map(|symbol| symbol.name.as_str())
        .collect::<Vec<_>>();
    calls.sort();
    assert_eq!(calls, ["leaf", "leaf", "nested"]);
    assert_eq!(edges(&graph, "nested", RelationKind::Calls)[0].name, "leaf");
    assert_eq!(
        edges(&graph, "sibling", RelationKind::Calls)[0].name,
        "leaf"
    );
    for local in ["argument", "local", "callback"] {
        assert!(edges(&graph, local, RelationKind::Calls).is_empty());
        assert!(
            edges(&graph, "outer", RelationKind::Contains)
                .iter()
                .any(|symbol| symbol.name == local)
        );
    }
}

#[test]
fn nested_macro_expansions_keep_their_generated_call_owner() {
    let graph = graph(
        r#"
macro_rules! inner { () => { fn generated() { leaf(); } } }
macro_rules! outer { () => { inner!(); } }
outer!();
fn leaf() {}
fn caller() { generated(); }
"#,
    );
    assert_eq!(
        edges(&graph, "caller", RelationKind::Calls)[0].name,
        "generated"
    );
    assert_eq!(
        edges(&graph, "generated", RelationKind::Calls)[0].name,
        "leaf"
    );
    assert!(graph.data().symbols.iter().any(|symbol| {
        symbol.name == "generated" && matches!(symbol.origin, SymbolOrigin::Expansion { .. })
    }));
}
