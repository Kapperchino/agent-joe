use knowledge_indexer::*;
use std::fs;

#[test]
fn captured_workspace_resolves_source_without_executing_generators() {
    let temporary = tempfile::tempdir().unwrap();
    let files = [
        (
            "Cargo.toml",
            "[workspace]\nmembers = ['app', 'macros']\nresolver = '2'\n",
        ),
        (
            "Cargo.lock",
            "version = 4\n[[package]]\nname = 'app'\nversion = '0.1.0'\ndependencies = ['macros']\n[[package]]\nname = 'macros'\nversion = '0.1.0'\n",
        ),
        (
            "app/Cargo.toml",
            "[package]\nname = 'app'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\nmacros = { path = '../macros' }\n",
        ),
        (
            "app/build.rs",
            "fn main() { std::fs::write(std::path::Path::new(&std::env::var(\"OUT_DIR\").unwrap()).join(\"generated.rs\"), \"pub fn generated() { crate::helper::target(); }\").unwrap(); }",
        ),
        (
            "app/src/lib.rs",
            "mod helper;\ninclude!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\nmacros::produce!();\npub fn entry() { helper::target(); generated(); made(); }\n#[cfg(test)] fn only_test() { helper::target(); }\n",
        ),
        ("app/src/helper.rs", "pub fn target() {}\n"),
        (
            "macros/Cargo.toml",
            "[package]\nname = 'macros'\nversion = '0.1.0'\nedition = '2024'\n[lib]\nproc-macro = true\n",
        ),
        (
            "macros/src/lib.rs",
            "extern crate proc_macro;\n#[proc_macro] pub fn produce(_: proc_macro::TokenStream) -> proc_macro::TokenStream { \"pub fn made() { crate::helper::target(); }\".parse().unwrap() }\n",
        ),
    ];
    let mut sources = Vec::new();
    for (path, text) in files {
        let destination = temporary.path().join(path);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        fs::write(destination, text).unwrap();
        sources.push(
            SourceFile::new(SourcePath::try_from(path.to_owned()).unwrap(), text.into()).unwrap(),
        );
    }
    let graph = load_sources(
        sources,
        SemanticProfile {
            manifest: "Cargo.toml".to_owned().try_into().unwrap(),
            target: native_target(),
            features: Features::Default,
            configurations: [Configuration::Normal, Configuration::Test].into(),
            analyzer_version: ANALYZER_VERSION.into(),
        },
        &|| Ok(()),
    )
    .unwrap();
    let data = graph.data();
    assert!(
        !data
            .symbols
            .iter()
            .any(|symbol| symbol.name == "made" && symbol.state == DefinitionState::Resolved)
    );
    assert!(
        !data
            .sources
            .iter()
            .any(|source| source.path().as_str().ends_with("generated.rs"))
    );
    assert!(data.symbols.iter().any(|symbol| symbol.name == "only_test" && symbol.configuration == Configuration::Test));
    assert!(
        !data
            .symbols
            .iter()
            .any(|symbol| symbol.name == "only_test"
                && symbol.configuration == Configuration::Normal)
    );
    assert!(
        data.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Proc-macro source"))
    );
    assert!(
        data.relations
            .iter()
            .any(|relation| relation.kind == RelationKind::Calls
                && matches!(relation.target, RelationTarget::Unresolved(_)))
    );
    for name in ["entry", "only_test"] {
        assert!(data.relations.iter().any(|relation| {
            relation.kind == RelationKind::Calls
                && data
                    .symbols
                    .iter()
                    .any(|symbol| symbol.id == relation.source && symbol.name == name)
                && relation.target.symbols().any(|id| {
                    data.symbols
                        .iter()
                        .any(|symbol| &symbol.id == id && symbol.name == "target")
                })
        }));
    }
    for (path, text) in files {
        assert_eq!(
            fs::read_to_string(temporary.path().join(path)).unwrap(),
            text
        );
    }
}
