use crate::*;

fn profile() -> SemanticProfile {
    SemanticProfile {
        manifest: "Cargo.toml".to_owned().try_into().unwrap(),
        target: native_target(),
        features: Features::Default,
        configurations: [Configuration::Normal, Configuration::Test].into(),
        analyzer_version: ANALYZER_VERSION.into(),
    }
}

fn sources(files: &[(&str, &str)]) -> Vec<SourceFile> {
    files
        .iter()
        .map(|(path, text)| {
            SourceFile::new((*path).to_owned().try_into().unwrap(), (*text).into()).unwrap()
        })
        .collect()
}

fn targets(graph: &SemanticGraph, caller: &str, kind: RelationKind) -> Vec<String> {
    let data = graph.data();
    data.relations
        .iter()
        .filter(|relation| {
            relation.kind == kind
                && data
                    .symbols
                    .iter()
                    .any(|symbol| symbol.id == relation.source && symbol.name == caller)
        })
        .flat_map(|relation| relation.target.symbols())
        .filter_map(|id| data.symbols.iter().find(|symbol| &symbol.id == id))
        .map(|symbol| symbol.name.clone())
        .collect()
}

#[test]
fn direct_library_resolves_inherited_renamed_dependencies_modules_and_traits() {
    let files = sources(&[
        (
            "Cargo.toml",
            "[workspace]\nmembers=['app','domain']\n[workspace.package]\nedition='2024'\nversion='0.1.0'\n[workspace.dependencies]\nalias={package='domain',path='domain'}",
        ),
        (
            "app/Cargo.toml",
            "[package]\nname='app'\nversion.workspace=true\nedition.workspace=true\n[dependencies]\nalias.workspace=true",
        ),
        (
            "domain/Cargo.toml",
            "[package]\nname='domain'\nversion.workspace=true\nedition.workspace=true",
        ),
        (
            "domain/src/lib.rs",
            "mod service; pub use service::{Service, Item, serve};",
        ),
        (
            "domain/src/service.rs",
            "pub trait Service { fn run(&self); } pub struct Item; impl Service for Item { fn run(&self) {} } pub fn serve<T: Service>(value: T) { value.run(); }",
        ),
        (
            "app/src/main.rs",
            "use alias::{serve, Item}; fn main() { serve(Item); }",
        ),
    ]);
    let graph = load_sources(files.clone(), profile(), &|| Ok(())).unwrap();
    assert_eq!(graph.data().sources.len(), files.len());
    assert!(targets(&graph, "main", RelationKind::Calls).contains(&"serve".into()));
    assert!(targets(&graph, "serve", RelationKind::DispatchesToTrait).contains(&"run".into()));
    assert!(
        graph
            .data()
            .relations
            .iter()
            .any(|relation| relation.kind == RelationKind::Implements
                && !relation.target.symbols().collect::<Vec<_>>().is_empty())
    );
    assert!(
        graph
            .data()
            .symbols
            .iter()
            .any(|symbol| symbol.configuration == Configuration::Test)
    );
}

#[test]
fn direct_library_honors_optional_dependency_features_and_configuration() {
    let files = sources(&[
        (
            "Cargo.toml",
            "[package]\nname='app'\nversion='0.1.0'\nedition='2024'\n[features]\ndefault=['enabled']\nenabled=['dep:other','other/selected']\nweak=['other?/selected']\n[dependencies]\nother={path='other',optional=true,default-features=false}",
        ),
        (
            "src/lib.rs",
            "#[cfg(feature=\"enabled\")] pub fn caller() { other::chosen(); } #[cfg(test)] fn only_test() { caller(); }",
        ),
        (
            "other/Cargo.toml",
            "[package]\nname='other'\nversion='0.1.0'\nedition='2024'\n[features]\nselected=[]",
        ),
        (
            "other/src/lib.rs",
            "#[cfg(feature=\"selected\")] pub fn chosen() {}",
        ),
    ]);
    let graph = load_sources(files.clone(), profile(), &|| Ok(())).unwrap();
    assert!(targets(&graph, "caller", RelationKind::Calls).contains(&"chosen".into()));
    let tests = graph
        .data()
        .symbols
        .iter()
        .filter(|symbol| symbol.name == "only_test" && symbol.state == DefinitionState::Resolved)
        .collect::<Vec<_>>();
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0].configuration, Configuration::Test);
    let mut selected = profile();
    selected.features = Features::Named {
        names: ["weak".into()].into(),
        defaults: DefaultFeatures::Disabled,
    };
    let weak = load_sources(files, selected, &|| Ok(())).unwrap();
    assert!(targets(&weak, "caller", RelationKind::Calls).is_empty());
}

#[test]
fn direct_library_keeps_generated_gaps_and_never_runs_project_code() {
    let directory = tempfile::tempdir().unwrap();
    let sentinel = directory.path().join("should-not-exist");
    let script = format!(
        "fn main() {{ std::fs::write({:?}, \"executed\").unwrap(); }}",
        sentinel
    );
    let files = sources(&[
        (
            "Cargo.toml",
            "[package]\nname='app'\nversion='0.1.0'\nedition='2024'\n[dependencies]\nunknown='1'",
        ),
        (
            "src/lib.rs",
            "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\")); pub fn caller() { generated(); unknown::missing(); }",
        ),
        ("build.rs", &script),
    ]);
    let graph = load_sources(files, profile(), &|| Ok(())).unwrap();
    assert!(!sentinel.exists());
    assert!(
        graph
            .data()
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("never executed"))
    );
    assert!(
        graph
            .data()
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Dependency unknown"))
    );
    assert!(
        graph
            .data()
            .relations
            .iter()
            .any(|relation| matches!(relation.target, RelationTarget::Unresolved(_)))
    );
}

#[test]
fn direct_library_checks_cancellation_and_rejects_unsupported_profiles() {
    let files = sources(&[
        ("Cargo.toml", "[package]\nname='app'\nversion='0.1.0'"),
        ("src/lib.rs", "pub fn a() {}"),
    ]);
    assert!(
        load_sources(files.clone(), profile(), &|| Err(anyhow::anyhow!(
            "cancelled"
        )))
        .unwrap_err()
        .to_string()
        .contains("cancelled")
    );
    let count = std::cell::Cell::new(0);
    assert!(
        load_sources(files.clone(), profile(), &|| {
            count.set(count.get() + 1);
            match count.get() {
                0..=10 => Ok(()),
                _ => Err(anyhow::anyhow!("cancelled during analysis")),
            }
        })
        .is_err()
    );
    let mut unsupported = profile();
    unsupported.target = "unsupported".into();
    assert!(load_sources(files, unsupported, &|| Ok(())).is_err());
}

#[test]
fn direct_library_handles_explicit_workspaces_custom_libraries_and_implicit_features() {
    let files = sources(&[
        (
            "Cargo.toml",
            "[workspace]\nmembers=['app']\n[workspace.package]\nedition='2015'",
        ),
        (
            "settings/Cargo.toml",
            "[workspace]\nmembers=['../app']\n[workspace.package]\nedition='2024'",
        ),
        (
            "app/Cargo.toml",
            "[package]\nname='app'\nversion='0.1.0'\nworkspace='../settings'\nedition.workspace=true\nbuild='generate.rs'\n[features]\ndefault=['enabled']\nenabled=['library/selected']\n[dependencies]\nlibrary={path='../library',optional=true,default-features=false}\n[dev-dependencies]\nlibrary={path='../library',default-features=false}",
        ),
        (
            "app/src/lib.rs",
            "#[cfg(feature=\"library\")] pub fn caller() { custom::chosen(); }",
        ),
        ("app/generate.rs", "fn main() {}"),
        (
            "library/Cargo.toml",
            "[package]\nname='library'\nversion='0.1.0'\nedition='2024'\n[lib]\nname='custom'\n[features]\nselected=[]",
        ),
        (
            "library/src/lib.rs",
            "#[cfg(feature=\"selected\")] pub fn chosen() {}",
        ),
    ]);
    let graph = load_sources(files, profile(), &|| Ok(())).unwrap();
    assert_eq!(
        targets(&graph, "caller", RelationKind::Calls),
        ["chosen", "chosen"]
    );
    assert!(
        graph
            .data()
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Build script is indexed"))
    );
}

#[test]
fn direct_library_expands_declarative_macros_but_cannot_read_uncaptured_files() {
    let directory = tempfile::tempdir().unwrap();
    let external = directory.path().join("secret.rs");
    std::fs::write(&external, "pub fn secret() {}").unwrap();
    let library = format!(
        "include!({external:?}); macro_rules! make {{ () => {{ pub fn made() {{ target(); }} }} }} make!(); pub fn target() {{}} pub fn caller() {{ made(); secret(); }}"
    );
    let graph = load_sources(
        sources(&[
            (
                "Cargo.toml",
                "[package]\nname='app'\nversion='0.1.0'\nedition='2024'",
            ),
            ("src/lib.rs", &library),
        ]),
        profile(),
        &|| Ok(()),
    )
    .unwrap();
    assert!(targets(&graph, "caller", RelationKind::Calls).contains(&"made".into()));
    assert!(targets(&graph, "made", RelationKind::Calls).contains(&"target".into()));
    assert!(
        !graph
            .data()
            .symbols
            .iter()
            .any(|symbol| symbol.name == "secret" && symbol.state == DefinitionState::Resolved)
    );
    assert!(
        graph
            .data()
            .symbols
            .iter()
            .any(|symbol| symbol.name == "made"
                && matches!(symbol.origin, SymbolOrigin::Expansion { .. }))
    );
}
