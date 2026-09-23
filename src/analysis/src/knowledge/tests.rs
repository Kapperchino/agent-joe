use super::*;

fn source(path: &str, text: impl Into<String>) -> SourceFile {
    SourceFile::new(SourcePath::try_from(path.to_owned()).unwrap(), text.into()).unwrap()
}

fn symbol(id: &str, source: &SourceFile, kind: SymbolKind) -> Symbol {
    Symbol {
        id: SymbolId(id.into()),
        name: id.into(),
        qualified_name: format!("fixture::{id}"),
        crate_key: "fixture".into(),
        configuration: Configuration::Normal,
        kind,
        origin: SymbolOrigin::Source {
            location: SourceLocation {
                path: source.path().clone(),
                span: source.span(),
            },
        },
        state: DefinitionState::Resolved,
        signature: Some(format!("pub fn {id}()")),
    }
}

fn graph(
    sources: Vec<SourceFile>,
    symbols: Vec<Symbol>,
    relations: Vec<Relation>,
) -> Arc<SemanticGraph> {
    Arc::new(
        SemanticGraph::try_from(GraphData {
            version: KNOWLEDGE_PROTOCOL_VERSION,
            profile: SemanticProfile {
                manifest: SourcePath::try_from("Cargo.toml".to_owned()).unwrap(),
                target: "x86_64-unknown-linux-gnu".into(),
                features: Features::Default,
                configurations: BTreeSet::from([Configuration::Normal, Configuration::Test]),
                analyzer_version: ANALYZER_VERSION.into(),
            },
            sources,
            symbols,
            relations,
            diagnostics: Vec::new(),
        })
        .unwrap(),
    )
}

fn relation(source: &str, target: &str) -> Relation {
    Relation {
        source: SymbolId(source.into()),
        kind: RelationKind::Calls,
        target: RelationTarget::Resolved(SymbolId(target.into())),
        site: None,
    }
}

fn measure(context: &str) -> anyhow::Result<usize> {
    Ok(1024 + context.len().div_ceil(4))
}

fn index(graph: Arc<SemanticGraph>) -> KnowledgeIndex {
    KnowledgeIndex::new(
        graph,
        "generation".into(),
        KnowledgeBudget::new(8192, 8192, 1024).unwrap(),
        &measure,
    )
    .unwrap()
}

fn assert_coverage(index: &KnowledgeIndex) {
    ownership::Coverage::new(&index.graph, &index.shards).unwrap();
    for source in &index.graph.data().sources {
        let mut parts: Vec<_> = index
            .shards
            .iter()
            .flat_map(|shard| &shard.owned)
            .filter(|part| part.location.path == *source.path())
            .collect();
        parts.sort_by_key(|part| part.location.span);
        let joined = parts
            .iter()
            .map(|part| part.location.span.text(source.text()).unwrap())
            .collect::<String>();
        assert_eq!(joined, source.text());
    }
    for shard in &index.shards {
        assert_eq!(
            shard.summary.estimated_tokens,
            measure(&shard.context).unwrap()
        );
        assert!(shard.summary.estimated_tokens <= index.budget.context());
    }
}

#[test]
fn knowledge_cycles_shared_dependencies_and_disconnected_source_are_owned_once() {
    let sources = vec![
        source("main.rs", "fn main() { a(); b(); }"),
        source("a.rs", "pub fn a() { b(); shared(); }"),
        source("b.rs", "pub fn b() { a(); shared(); }"),
        source("shared.rs", "pub fn shared() {}"),
        source("inactive.rs", "#[cfg(disabled)] fn dormant() {}"),
        source("README.md", "Repository documentation"),
        source("empty.rs", ""),
    ];
    let symbols = sources[..4]
        .iter()
        .zip(["main", "a", "b", "shared"])
        .map(|(source, id)| symbol(id, source, SymbolKind::Function))
        .collect();
    let graph = graph(
        sources,
        symbols,
        vec![
            relation("main", "a"),
            relation("main", "b"),
            relation("a", "b"),
            relation("b", "a"),
            relation("a", "shared"),
            relation("b", "shared"),
        ],
    );
    let first = index(graph.clone());
    let second = index(graph);
    assert_coverage(&first);
    assert_eq!(first.shards.len(), 1);
    assert_eq!(
        first
            .shards
            .iter()
            .map(|shard| &shard.context)
            .collect::<Vec<_>>(),
        second
            .shards
            .iter()
            .map(|shard| &shard.context)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        first.search("inactive.rs", 0, 10).unwrap().hits[0]
            .primary_shards
            .len(),
        1
    );
}

#[test]
fn knowledge_oversized_cycles_and_single_items_split_on_valid_source_boundaries() {
    let sources = vec![
        source(
            "a.rs",
            format!("fn a() {{ {} }}", "let café = \"🦀\";\n".repeat(1800)),
        ),
        source("b.rs", "fn b() {}\n".repeat(3000)),
    ];
    let symbols = sources
        .iter()
        .zip(["a", "b"])
        .map(|(source, id)| symbol(id, source, SymbolKind::Function))
        .collect();
    let index = index(graph(
        sources,
        symbols,
        vec![relation("a", "b"), relation("b", "a")],
    ));
    assert_coverage(&index);
    assert!(index.shards.len() > 2);
    let route = index.inspect(&SymbolId("a".into())).unwrap();
    assert!(route.primary_shards.len() > 1);
    assert_eq!(route.relations.len(), 2);
}

#[test]
fn knowledge_nested_and_configuration_overlaps_do_not_duplicate_source() {
    let source = source("lib.rs", "mod nested { fn selected() {} }");
    let module = symbol("module", &source, SymbolKind::Module);
    let mut function = symbol("selected", &source, SymbolKind::Function);
    function.origin = SymbolOrigin::Source {
        location: SourceLocation {
            path: source.path().clone(),
            span: ByteSpan::new(13, 29).unwrap(),
        },
    };
    let mut test = function.clone();
    test.id = SymbolId("test-selected".into());
    test.configuration = Configuration::Test;
    let index = index(graph(
        vec![source],
        vec![module, function, test],
        Vec::new(),
    ));
    assert_coverage(&index);
    assert_eq!(
        index
            .inspect(&SymbolId("selected".into()))
            .unwrap()
            .primary_shards,
        index
            .inspect(&SymbolId("test-selected".into()))
            .unwrap()
            .primary_shards
    );
}

#[test]
fn knowledge_routing_has_stable_pages_relationships_and_explicit_limits() {
    let source = source("main.rs", "fn main() { external(); }");
    let main = symbol("main", &source, SymbolKind::Function);
    let mut external = main.clone();
    external.id = SymbolId("external".into());
    external.name = "external".into();
    external.qualified_name = "dependency::external".into();
    external.origin = SymbolOrigin::External {
        crate_name: "dependency".into(),
    };
    let index = index(graph(
        vec![source],
        vec![main, external],
        vec![relation("main", "external")],
    ));
    let page = index.search("main", 0, 1).unwrap();
    assert_eq!(page.next_offset, Some(1));
    assert!(page.hits[0].symbol.is_some());
    assert!(index.search("main", 1, 1).unwrap().hits[0].symbol.is_none());
    let external = index.inspect(&SymbolId("external".into())).unwrap();
    assert!(external.primary_shards.is_empty());
    assert_eq!(external.related_shards, BTreeSet::from([0]));
    assert!(index.search("", 0, 1).is_err());
    assert!(index.search("main", 0, 101).is_err());
    assert!(index.inspect(&SymbolId("unknown".into())).is_err());
}

#[test]
fn knowledge_many_small_disconnected_files_pack_without_exhausting_actors() {
    let sources = (0..300)
        .map(|index| source(&format!("docs/{index}.md"), "small document"))
        .collect();
    let index = index(graph(sources, Vec::new(), Vec::new()));
    assert_coverage(&index);
    assert!(index.shards.len() < 10);
}

#[test]
fn knowledge_budget_accounts_for_rendered_metadata_and_request_reserves() {
    let graph = graph(vec![source("single.rs", "🦀")], Vec::new(), Vec::new());
    let budget = KnowledgeBudget::new(4096, 8192, 16000).unwrap();
    assert_eq!(budget.window(), 4096);
    assert_eq!(budget.response(), 1024);
    assert_eq!(budget.input() - budget.context(), budget.question());
    assert!(
        KnowledgeIndex::new(graph, "generation".into(), budget, &|_| Ok(budget
            .context()
            + 1))
        .is_err()
    );
    assert!(KnowledgeBudget::new(1024, 8192, 1024).is_err());
}

#[test]
fn knowledge_optional_headers_yield_to_owned_source_and_point_locations_route() {
    let source = source("main.rs", "fn main() {}");
    let main = symbol("main", &source, SymbolKind::Function);
    let point = Symbol {
        id: SymbolId("point".into()),
        origin: SymbolOrigin::Source {
            location: SourceLocation {
                path: source.path().clone(),
                span: ByteSpan::new(3, 3).unwrap(),
            },
        },
        kind: SymbolKind::Local,
        ..main.clone()
    };
    let external: Vec<_> = (0..8)
        .map(|index| Symbol {
            id: SymbolId(format!("external{index}")),
            name: "X".repeat(128),
            qualified_name: "X".repeat(256),
            signature: Some("X".repeat(256)),
            origin: SymbolOrigin::External {
                crate_name: "dependency".into(),
            },
            ..main.clone()
        })
        .collect();
    let relations = external
        .iter()
        .map(|external| relation("main", &external.id.0))
        .collect();
    let symbols = [main, point].into_iter().chain(external).collect();
    let graph = graph(vec![source], symbols, relations);
    let index = KnowledgeIndex::new(
        graph,
        "generation".into(),
        KnowledgeBudget::new(4096, 4096, 1024).unwrap(),
        &measure,
    )
    .unwrap();
    assert_coverage(&index);
    assert_eq!(
        index
            .inspect(&SymbolId("point".into()))
            .unwrap()
            .primary_shards,
        BTreeSet::from([0])
    );
    let rendered: serde_json::Value = serde_json::from_str(&index.shards[0].context).unwrap();
    assert!(rendered["secondary_headers"].as_array().unwrap().len() < 8);
}
