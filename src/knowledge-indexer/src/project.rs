use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Component, Path, PathBuf},
};

use anyhow::Context;
use cargo_toml::{AbstractFilesystem, Dependency, Manifest, Product};
use ra_ap_cfg::CfgOptions;
use ra_ap_ide_db::{
    ChangeWithProcMacros, RootDatabase,
    base_db::{
        CrateBuilderId, CrateGraphBuilder, CrateName, CrateOrigin, CrateWorkspaceData,
        DependencyBuilder, Env, SourceRoot,
    },
    span::Edition,
};
use ra_ap_intern::Symbol;
use ra_ap_vfs::{AbsPathBuf, FileId, VfsPath, file_set::FileSet};
use triomphe::Arc;

use crate::*;

const VIRTUAL_ROOT: &str = "/__joe_knowledge";
const MAX_PROJECT_ITEMS: usize = 20_000;

pub fn native_target() -> String {
    let platform = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" if cfg!(target_env = "musl") => "unknown-linux-musl",
        "linux" => "unknown-linux-gnu",
        "windows" if cfg!(target_env = "gnu") => "pc-windows-gnu",
        "windows" => "pc-windows-msvc",
        other => other,
    };
    format!("{}-{platform}", std::env::consts::ARCH)
}

pub fn load_sources(
    sources: Vec<SourceFile>,
    profile: SemanticProfile,
    checkpoint: &dyn Fn() -> anyhow::Result<()>,
) -> anyhow::Result<SemanticGraph> {
    checkpoint()?;
    let empty = SemanticGraph::try_from(GraphData {
        version: KNOWLEDGE_PROTOCOL_VERSION,
        profile: profile.clone(),
        sources,
        symbols: Vec::new(),
        relations: Vec::new(),
        diagnostics: Vec::new(),
    })?;
    let sources = GraphData::from(empty).sources;
    let platform = NativeCfg::new(&profile)?;
    let files = CapturedFiles::new(sources)?;
    let packages = files.packages(&profile.manifest, checkpoint)?;
    let mut output = GraphData {
        version: KNOWLEDGE_PROTOCOL_VERSION,
        profile: profile.clone(),
        sources: files.sources.values().cloned().collect(),
        symbols: Vec::new(),
        relations: Vec::new(),
        diagnostics: vec![IndexDiagnostic {
            message: "In-process analysis covers discoverable Cargo packages and declared local path dependencies. No Cargo, compiler, build script or proc-macro process is executed. Sysroot and registry/git sources, generated OUT_DIR contents, Cargo patches/configuration and custom compiler flags are unavailable; affected relationships remain unresolved. Native baseline cfg is used; features are unified across indexed packages, not Cargo build units.".into(),
            location: None,
        }],
    };
    for configuration in &profile.configurations {
        checkpoint()?;
        let dependencies = packages
            .iter()
            .map(|package| package.dependencies(*configuration, &platform))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let features = resolved_features(&packages, &dependencies, &profile.features, checkpoint)?;
        let mut change = ChangeWithProcMacros::default();
        let mut file_set = FileSet::default();
        let mut ids = BTreeMap::new();
        for (index, source) in files.sources.values().enumerate() {
            checkpoint()?;
            let id = FileId::from_raw(index as u32);
            ids.insert(source.path().clone(), id);
            file_set.insert(
                id,
                VfsPath::new_virtual_path(format!("{VIRTUAL_ROOT}/{}", source.path().as_str())),
            );
            change.change_file(id, Some(source.text().to_owned()));
        }
        change.set_roots(vec![SourceRoot::new_local(file_set)]);
        let mut graph = ProjectGraph::new()?;
        for (index, package) in packages.iter().enumerate() {
            for target in package.targets(*configuration) {
                checkpoint()?;
                match target.source(package, &features[index], &ids)? {
                    TargetSource::Captured(file_id) => graph.add_root(
                        index,
                        package,
                        &target,
                        file_id,
                        target.cfg(&platform, *configuration, &features[index]),
                    )?,
                    TargetSource::Missing(path) => {
                        output.diagnostics.push(package.diagnostic(format!(
                            "Target source {} was not captured",
                            path.as_str()
                        )))
                    }
                    TargetSource::Excluded => output
                        .diagnostics
                        .push(package.diagnostic("Target excluded by required-features".into())),
                }
            }
            if package.manifest.package.as_ref().is_some_and(|package| {
                matches!(package.build, Some(cargo_toml::OptionalFile::Path(_)))
            }) {
                output.diagnostics.push(package.diagnostic("Build script is indexed but never executed; generated cfg, environment and OUT_DIR are unavailable".into()));
            }
            if package.manifest.lib.as_ref().is_some_and(|lib| {
                lib.proc_macro || lib.crate_type.iter().any(|kind| kind == "proc-macro")
            }) {
                output.diagnostics.push(
                    package.diagnostic(
                        "Proc-macro source is indexed but expansions are disabled".into(),
                    ),
                );
            }
        }
        for root in &graph.roots {
            checkpoint()?;
            let mut linked = BTreeMap::new();
            for dependency in dependencies[root.package]
                .iter()
                .filter(|dep| dep.applies(root.kind) && features[root.package].active(dep))
            {
                checkpoint()?;
                match dependency.link(&packages, &graph.roots, &mut linked)? {
                    DependencyLink::Pending(edge) => {
                        let diagnostic = graph.builder.add_dep(root.id, edge).err().map(|_| {
                            packages[root.package].diagnostic(format!(
                                "Cyclic dependency edge {} omitted; relationships through this edge are incomplete",
                                dependency.name
                            ))
                        });
                        output.diagnostics.extend(diagnostic);
                    }
                    DependencyLink::Duplicate => {}
                    DependencyLink::Missing => output.diagnostics.push(
                        packages[root.package].diagnostic(format!(
                            "Dependency {} has no captured library target; references through it may be unresolved",
                            dependency.name
                        )),
                    ),
                }
            }
            let library = graph
                .roots
                .iter()
                .find(|candidate| {
                    candidate.package == root.package && candidate.kind == TargetKind::Library
                })
                .filter(|_| matches!(root.kind, TargetKind::Binary | TargetKind::Test));
            if let Some(library) = library {
                graph
                    .builder
                    .add_dep(
                        root.id,
                        DependencyBuilder::new(
                            CrateName::new(&library.name).map_err(|error| {
                                anyhow::anyhow!("Invalid library name: {error}")
                            })?,
                            library.id,
                        ),
                    )
                    .map_err(|error| {
                        anyhow::anyhow!("Invalid package target dependency: {error:?}")
                    })?;
            }
        }
        change.source_change.set_crate_graph(graph.builder);
        let mut db = RootDatabase::default();
        change.apply(&mut db);
        checkpoint()?;
        let indexed = files
            .sources
            .values()
            .map(|source| IndexedSource {
                file_id: source
                    .path()
                    .as_str()
                    .ends_with(".rs")
                    .then(|| ids[source.path()]),
                source: source.clone(),
            })
            .collect();
        let data = GraphData::from(crate::extract::extract_checked(
            &db,
            indexed,
            profile.clone(),
            *configuration,
            checkpoint,
        )?);
        output.symbols.extend(data.symbols);
        output.relations.extend(data.relations);
        output.diagnostics.extend(data.diagnostics);
    }
    checkpoint()?;
    output.diagnostics.sort_by(|left, right| {
        left.message
            .cmp(&right.message)
            .then(left.location.cmp(&right.location))
    });
    output.diagnostics.dedup();
    SemanticGraph::try_from(output)
}

struct CapturedFiles {
    sources: BTreeMap<SourcePath, SourceFile>,
    directories: BTreeMap<PathBuf, HashSet<Box<str>>>,
}

impl CapturedFiles {
    fn new(sources: Vec<SourceFile>) -> anyhow::Result<Self> {
        match sources.len() <= MAX_PROJECT_ITEMS {
            true => {}
            false => Err(anyhow::anyhow!("Too many captured files"))?,
        }
        let mut directories: BTreeMap<PathBuf, HashSet<Box<str>>> = BTreeMap::new();
        for source in &sources {
            let mut path = Path::new(source.path().as_str());
            while let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
                directories
                    .entry(parent.to_owned())
                    .or_default()
                    .insert(name.to_str().context("Non-UTF-8 source path")?.into());
                path = parent;
            }
        }
        Ok(Self {
            sources: sources
                .into_iter()
                .map(|source| (source.path().clone(), source))
                .collect(),
            directories,
        })
    }

    fn packages(
        &self,
        entry: &SourcePath,
        checkpoint: &dyn Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<Vec<Package>> {
        let manifests = self
            .sources
            .values()
            .filter(|source| {
                Path::new(source.path().as_str())
                    .file_name()
                    .is_some_and(|name| name == "Cargo.toml")
            })
            .map(|source| {
                checkpoint()?;
                Ok((
                    source.path().clone(),
                    Manifest::from_slice(source.text().as_bytes())
                        .with_context(|| format!("Invalid manifest {}", source.path().as_str()))?,
                ))
            })
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        manifests
            .get(entry)
            .context("Selected Cargo manifest was not captured")?;
        manifests
            .iter()
            .filter(|(_, manifest)| manifest.package.is_some())
            .map(|(path, manifest)| {
                checkpoint()?;
                let directory = Path::new(path.as_str())
                    .parent()
                    .context("Manifest has no parent")?
                    .to_owned();
                let explicit_workspace = manifest
                    .package
                    .as_ref()
                    .and_then(|package| package.workspace.as_ref());
                let workspace = match explicit_workspace {
                    Some(path) => {
                        let path = relative_path(
                            &directory,
                            &format!(
                                "{}/Cargo.toml",
                                path.to_str().context("Non-UTF-8 workspace path")?
                            ),
                        )?;
                        let manifest = manifests
                            .get(&path)
                            .filter(|manifest| manifest.workspace.is_some())
                            .context("Explicit workspace manifest was not captured")?;
                        Some((
                            manifest,
                            Path::new(VIRTUAL_ROOT).join(
                                Path::new(path.as_str())
                                    .parent()
                                    .context("Missing workspace directory")?,
                            ),
                        ))
                    }
                    None => directory
                        .ancestors()
                        .filter_map(|directory| {
                            let path: SourcePath = directory
                                .join("Cargo.toml")
                                .to_str()?
                                .to_owned()
                                .try_into()
                                .ok()?;
                            manifests
                                .get(&path)
                                .filter(|manifest| manifest.workspace.is_some())
                                .map(|manifest| (manifest, Path::new(VIRTUAL_ROOT).join(directory)))
                        })
                        .next(),
                };
                let mut manifest = manifest.clone();
                manifest.complete_from_abstract_filesystem(
                    CapturedDirectory {
                        files: self,
                        directory: &directory,
                    },
                    workspace
                        .as_ref()
                        .map(|(manifest, directory)| (*manifest, directory.as_path())),
                )?;
                let name = manifest
                    .package
                    .as_ref()
                    .context("Missing package")?
                    .name
                    .clone();
                Ok(Package {
                    directory,
                    path: path.clone(),
                    manifest,
                    name,
                })
            })
            .collect()
    }
}

struct CapturedDirectory<'a> {
    files: &'a CapturedFiles,
    directory: &'a Path,
}

impl AbstractFilesystem for CapturedDirectory<'_> {
    fn file_names_in(&self, rel_path: &str) -> std::io::Result<HashSet<Box<str>>> {
        Ok(self
            .files
            .directories
            .get(&self.directory.join(rel_path))
            .cloned()
            .unwrap_or_default())
    }
}

fn relative_path(directory: &Path, value: &str) -> anyhow::Result<SourcePath> {
    let input = Path::new(value);
    let joined = match input.is_absolute() {
        true => input
            .strip_prefix(VIRTUAL_ROOT)
            .context("Path lies outside captured source")?
            .to_owned(),
        false => directory.join(input),
    };
    let mut components = Vec::new();
    for component in joined.components() {
        match component {
            Component::Normal(name) => components.push(name.to_str().context("Non-UTF-8 path")?),
            Component::CurDir => {}
            Component::ParentDir => {
                components.pop().context("Path leaves captured source")?;
            }
            _ => Err(anyhow::anyhow!("Unsupported source path"))?,
        }
    }
    components.join("/").try_into()
}

struct Package {
    directory: PathBuf,
    path: SourcePath,
    manifest: Manifest,
    name: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetKind {
    Library,
    Binary,
    Test,
    Build,
}

struct Target {
    product: Product,
    kind: TargetKind,
}

enum TargetSource {
    Captured(FileId),
    Missing(SourcePath),
    Excluded,
}

impl Target {
    fn source(
        &self,
        package: &Package,
        features: &EnabledFeatures,
        files: &BTreeMap<SourcePath, FileId>,
    ) -> anyhow::Result<TargetSource> {
        match self
            .product
            .required_features
            .iter()
            .find(|feature| !features.enabled.contains(*feature))
        {
            Some(_) => Ok(TargetSource::Excluded),
            None => {
                let path = relative_path(
                    &package.directory,
                    self.product
                        .path
                        .as_deref()
                        .context("Cargo target has no source path")?,
                )?;
                Ok(match files.get(&path) {
                    Some(file_id) => TargetSource::Captured(*file_id),
                    None => TargetSource::Missing(path),
                })
            }
        }
    }

    fn cfg(
        &self,
        platform: &NativeCfg,
        configuration: Configuration,
        features: &EnabledFeatures,
    ) -> CfgOptions {
        let mut cfg = platform.options.clone();
        for feature in &features.enabled {
            cfg.insert_key_value(Symbol::intern("feature"), Symbol::intern(feature));
        }
        match (configuration, self.kind) {
            (Configuration::Test, TargetKind::Library | TargetKind::Binary | TargetKind::Test) => {
                cfg.insert_atom(Symbol::intern("test"));
            }
            _ => {}
        }
        if self.is_proc_macro() {
            cfg.insert_atom(Symbol::intern("proc_macro"));
        }
        cfg
    }

    fn is_proc_macro(&self) -> bool {
        self.product.proc_macro
            || self
                .product
                .crate_type
                .iter()
                .any(|kind| kind == "proc-macro")
    }
}

struct CrateRoot {
    package: usize,
    name: String,
    id: CrateBuilderId,
    kind: TargetKind,
}

struct ProjectGraph {
    builder: CrateGraphBuilder,
    roots: Vec<CrateRoot>,
    cwd: Arc<AbsPathBuf>,
    workspace: Arc<CrateWorkspaceData>,
}

impl ProjectGraph {
    fn new() -> anyhow::Result<Self> {
        let cwd = AbsPathBuf::try_from(if cfg!(windows) {
            "C:/__joe_knowledge"
        } else {
            VIRTUAL_ROOT
        })
        .map_err(|_| anyhow::anyhow!("Invalid virtual directory"))?;
        Ok(Self {
            builder: CrateGraphBuilder::default(),
            roots: Vec::new(),
            cwd: Arc::new(cwd),
            workspace: Arc::new(CrateWorkspaceData {
                target: Err("No compiler target-layout query is executed".into()),
                toolchain: None,
            }),
        })
    }

    fn add_root(
        &mut self,
        package_index: usize,
        package: &Package,
        target: &Target,
        file_id: FileId,
        cfg: CfgOptions,
    ) -> anyhow::Result<()> {
        let name = target
            .product
            .name
            .as_deref()
            .unwrap_or(&package.name)
            .replace('-', "_");
        let edition = target
            .product
            .edition
            .unwrap_or(
                package
                    .manifest
                    .package
                    .as_ref()
                    .context("Missing package")?
                    .edition(),
            )
            .to_string()
            .parse::<Edition>()?;
        let id = self.builder.add_crate_root(
            file_id,
            edition,
            Some(
                CrateName::new(&name)
                    .map_err(|error| anyhow::anyhow!("Invalid crate name: {error}"))?
                    .into(),
            ),
            None,
            cfg.clone(),
            Some(cfg),
            Env::default(),
            CrateOrigin::Local {
                repo: None,
                name: Some(Symbol::intern(&package.name)),
            },
            Vec::new(),
            target.is_proc_macro(),
            self.cwd.clone(),
            self.workspace.clone(),
        );
        self.roots.push(CrateRoot {
            package: package_index,
            name,
            id,
            kind: target.kind,
        });
        match self.roots.len() {
            0..=MAX_PROJECT_ITEMS => Ok(()),
            _ => Err(anyhow::anyhow!("Too many crate targets")),
        }
    }
}

impl Package {
    fn default_feature(&self, defaults: &DefaultFeatures) -> Option<String> {
        match defaults {
            DefaultFeatures::Enabled => self
                .manifest
                .features
                .get("default")
                .map(|_| "default".to_owned()),
            DefaultFeatures::Disabled => None,
        }
    }

    fn diagnostic(&self, message: String) -> IndexDiagnostic {
        IndexDiagnostic {
            message: format!("{}: {message}", self.path.as_str()),
            location: None,
        }
    }

    fn targets(&self, configuration: Configuration) -> Vec<Target> {
        let mut targets = self
            .manifest
            .lib
            .iter()
            .cloned()
            .map(|product| Target {
                product,
                kind: TargetKind::Library,
            })
            .chain(self.manifest.bin.iter().cloned().map(|product| Target {
                product,
                kind: TargetKind::Binary,
            }))
            .collect::<Vec<_>>();
        if configuration == Configuration::Test {
            targets.extend(
                self.manifest
                    .test
                    .iter()
                    .chain(&self.manifest.example)
                    .chain(&self.manifest.bench)
                    .cloned()
                    .map(|product| Target {
                        product,
                        kind: TargetKind::Test,
                    }),
            );
        }
        if let Some(cargo_toml::OptionalFile::Path(path)) = self
            .manifest
            .package
            .as_ref()
            .and_then(|package| package.build.as_ref())
        {
            targets.push(Target {
                product: Product {
                    path: Some(path.to_string_lossy().into_owned()),
                    name: Some("build_script_build".into()),
                    ..Product::default()
                },
                kind: TargetKind::Build,
            });
        }
        targets
    }

    fn dependencies(
        &self,
        configuration: Configuration,
        platform: &NativeCfg,
    ) -> anyhow::Result<Vec<LocalDependency>> {
        let mut groups = vec![
            (&self.manifest.dependencies, DependencyKind::Normal),
            (&self.manifest.build_dependencies, DependencyKind::Build),
        ];
        if configuration == Configuration::Test {
            groups.push((&self.manifest.dev_dependencies, DependencyKind::Dev));
        }
        for (condition, target) in &self.manifest.target {
            if condition
                .parse::<cargo_platform::Platform>()?
                .matches(&native_target(), &platform.cargo)
            {
                groups.push((&target.dependencies, DependencyKind::Normal));
                groups.push((&target.build_dependencies, DependencyKind::Build));
                if configuration == Configuration::Test {
                    groups.push((&target.dev_dependencies, DependencyKind::Dev));
                }
            }
        }
        groups
            .into_iter()
            .flat_map(|(deps, kind)| {
                deps.iter()
                    .map(move |(name, dependency)| (name, dependency, kind))
            })
            .map(|(name, dependency, kind)| {
                let path = dependency
                    .detail()
                    .and_then(|detail| detail.path.as_deref())
                    .and_then(|path| {
                        relative_path(&self.directory, &format!("{path}/Cargo.toml")).ok()
                    });
                Ok(LocalDependency {
                    name: name.clone(),
                    dependency: dependency.clone(),
                    kind,
                    path,
                })
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
enum DependencyKind {
    Normal,
    Dev,
    Build,
}
struct LocalDependency {
    name: String,
    dependency: Dependency,
    kind: DependencyKind,
    path: Option<SourcePath>,
}

enum DependencyLink {
    Pending(DependencyBuilder),
    Duplicate,
    Missing,
}

impl LocalDependency {
    fn link(
        &self,
        packages: &[Package],
        roots: &[CrateRoot],
        linked: &mut BTreeMap<String, CrateBuilderId>,
    ) -> anyhow::Result<DependencyLink> {
        let target = self.destination(packages)?.and_then(|index| {
            roots.iter().find(|candidate| {
                candidate.package == index && candidate.kind == TargetKind::Library
            })
        });
        match target {
            Some(target) => {
                let name = match self.dependency.package() {
                    Some(_) => self.name.replace('-', "_"),
                    None => target.name.clone(),
                };
                match linked.insert(name.clone(), target.id) {
                    Some(previous) if previous != target.id => {
                        Err(anyhow::anyhow!("Ambiguous local dependency {name}"))
                    }
                    Some(_) => Ok(DependencyLink::Duplicate),
                    None => Ok(DependencyLink::Pending(DependencyBuilder::new(
                        CrateName::new(&name)
                            .map_err(|error| anyhow::anyhow!("Invalid dependency name: {error}"))?,
                        target.id,
                    ))),
                }
            }
            None => Ok(DependencyLink::Missing),
        }
    }

    fn applies(&self, target: TargetKind) -> bool {
        matches!(
            (self.kind, target),
            (DependencyKind::Build, TargetKind::Build)
                | (
                    DependencyKind::Normal | DependencyKind::Dev,
                    TargetKind::Library | TargetKind::Binary | TargetKind::Test
                )
        )
    }

    fn destination(&self, packages: &[Package]) -> anyhow::Result<Option<usize>> {
        let destination = self
            .path
            .as_ref()
            .and_then(|path| packages.iter().position(|package| &package.path == path));
        if let Some(index) = destination {
            match packages[index].name == self.dependency.package().unwrap_or(&self.name) {
                true => {}
                false => Err(anyhow::anyhow!(
                    "Path dependency {} points to a differently named package",
                    self.name
                ))?,
            }
        }
        Ok(destination)
    }
}

enum DependencyActivation {
    Enable,
    IfEnabled,
}

enum FeatureRequest<'a> {
    Named(&'a str),
    Dependency(&'a str),
    DependencyFeature {
        dependency: &'a str,
        feature: &'a str,
        activation: DependencyActivation,
    },
}

impl<'a> FeatureRequest<'a> {
    fn new(value: &'a str) -> Self {
        match value.split_once('/') {
            Some((dependency, feature)) => Self::DependencyFeature {
                dependency: dependency.trim_end_matches('?'),
                feature,
                activation: if dependency.ends_with('?') {
                    DependencyActivation::IfEnabled
                } else {
                    DependencyActivation::Enable
                },
            },
            None => match value.strip_prefix("dep:") {
                Some(dependency) => Self::Dependency(dependency),
                None => Self::Named(value),
            },
        }
    }
}

#[derive(Default)]
struct EnabledFeatures {
    enabled: BTreeSet<String>,
    dependencies: BTreeSet<String>,
    requested: BTreeMap<String, BTreeSet<String>>,
}

impl EnabledFeatures {
    fn active(&self, dependency: &LocalDependency) -> bool {
        !dependency.dependency.optional() || self.dependencies.contains(&dependency.name)
    }

    fn new(
        package: &Package,
        dependencies: &[LocalDependency],
        seeds: &BTreeSet<String>,
        checkpoint: &dyn Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<Self> {
        let mut state = Self::default();
        let mut pending = seeds.iter().cloned().collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        while let Some(feature) = pending.pop() {
            checkpoint()?;
            if seen.insert(feature.clone()) {
                match seen.len() {
                    0..=MAX_PROJECT_ITEMS => {}
                    _ => Err(anyhow::anyhow!("Feature expansion exceeds limit"))?,
                }
                state.expand(
                    FeatureRequest::new(&feature),
                    package,
                    dependencies,
                    &mut pending,
                )?;
            }
        }
        Ok(state)
    }

    fn expand(
        &mut self,
        request: FeatureRequest<'_>,
        package: &Package,
        dependencies: &[LocalDependency],
        pending: &mut Vec<String>,
    ) -> anyhow::Result<()> {
        match request {
            FeatureRequest::Named(feature) => {
                self.enabled.insert(feature.to_owned());
                match package.manifest.features.get(feature) {
                    Some(children) => pending.extend(children.iter().cloned()),
                    None if Self::implicit_dependency(package, dependencies, feature) => {
                        self.dependencies.insert(feature.to_owned());
                    }
                    None => Err(anyhow::anyhow!(
                        "Unknown feature {feature} for {}",
                        package.name
                    ))?,
                }
            }
            FeatureRequest::Dependency(dependency) => {
                self.dependencies.insert(dependency.to_owned());
            }
            FeatureRequest::DependencyFeature {
                dependency,
                feature,
                activation,
            } => {
                match activation {
                    DependencyActivation::Enable => {
                        self.dependencies.insert(dependency.to_owned());
                        pending.extend(
                            Self::implicit_dependency(package, dependencies, dependency)
                                .then(|| dependency.to_owned()),
                        );
                    }
                    DependencyActivation::IfEnabled => {}
                }
                self.requested
                    .entry(dependency.to_owned())
                    .or_default()
                    .insert(feature.to_owned());
            }
        }
        Ok(())
    }

    fn requests(&self, dependency: &LocalDependency, destination: &Package) -> BTreeSet<String> {
        let defaults = match dependency.dependency.detail() {
            Some(detail) if !detail.default_features => DefaultFeatures::Disabled,
            _ => DefaultFeatures::Enabled,
        };
        dependency
            .dependency
            .req_features()
            .iter()
            .chain(self.requested.get(&dependency.name).into_iter().flatten())
            .cloned()
            .chain(destination.default_feature(&defaults))
            .collect()
    }

    fn implicit_dependency(
        package: &Package,
        dependencies: &[LocalDependency],
        name: &str,
    ) -> bool {
        dependencies
            .iter()
            .any(|dependency| dependency.name == name && dependency.dependency.optional())
            && !package
                .manifest
                .features
                .values()
                .flatten()
                .any(|value| value == &format!("dep:{name}"))
    }
}

enum FeatureProgress {
    Expanding,
    Complete,
}

struct FeatureSeeds {
    packages: Vec<BTreeSet<String>>,
}

struct FeatureSeed<'a> {
    package: usize,
    feature: &'a str,
}

impl FeatureSeeds {
    fn new(
        packages: &[Package],
        dependencies: &[Vec<LocalDependency>],
        selection: &Features,
    ) -> anyhow::Result<Self> {
        let defaults = match selection {
            Features::Default
            | Features::Named {
                defaults: DefaultFeatures::Enabled,
                ..
            } => &DefaultFeatures::Enabled,
            _ => &DefaultFeatures::Disabled,
        };
        let mut seeds = Self {
            packages: packages
                .iter()
                .map(|package| package.default_feature(defaults).into_iter().collect())
                .collect(),
        };
        if let Features::Named { names, .. } = selection {
            for name in names {
                let mut selected = packages
                    .iter()
                    .enumerate()
                    .filter_map(|(index, package)| {
                        let feature = match name.split_once('/') {
                            Some((owner, feature)) if owner == package.name => Some(feature),
                            None if package.manifest.features.contains_key(name)
                                || dependencies[index]
                                    .iter()
                                    .any(|dep| dep.name == *name && dep.dependency.optional()) =>
                            {
                                Some(name.as_str())
                            }
                            _ => None,
                        };
                        feature.map(|feature| FeatureSeed {
                            package: index,
                            feature,
                        })
                    })
                    .peekable();
                selected
                    .peek()
                    .with_context(|| format!("No captured package provides feature {name}"))?;
                for seed in selected {
                    seeds.packages[seed.package].insert(seed.feature.to_owned());
                }
            }
        }
        Ok(seeds)
    }

    fn count(&self) -> usize {
        self.packages.iter().map(BTreeSet::len).sum()
    }

    fn propagate(
        &mut self,
        packages: &[Package],
        dependencies: &[Vec<LocalDependency>],
        enabled: &[EnabledFeatures],
        checkpoint: &dyn Fn() -> anyhow::Result<()>,
    ) -> anyhow::Result<FeatureProgress> {
        let previous_count = self.count();
        for (features, dependencies) in enabled.iter().zip(dependencies) {
            for dependency in dependencies.iter().filter(|dep| features.active(dep)) {
                checkpoint()?;
                if let Some(destination) = dependency.destination(packages)? {
                    self.packages[destination]
                        .extend(features.requests(dependency, &packages[destination]));
                }
            }
        }
        Ok(if self.count() == previous_count {
            FeatureProgress::Complete
        } else {
            FeatureProgress::Expanding
        })
    }
}

fn resolved_features(
    packages: &[Package],
    dependencies: &[Vec<LocalDependency>],
    selection: &Features,
    checkpoint: &dyn Fn() -> anyhow::Result<()>,
) -> anyhow::Result<Vec<EnabledFeatures>> {
    let mut seeds = FeatureSeeds::new(packages, dependencies, selection)?;
    let mut progress = FeatureProgress::Expanding;
    let mut result = Vec::new();
    let mut iterations = 0;
    while let FeatureProgress::Expanding = progress {
        checkpoint()?;
        iterations += 1;
        match iterations {
            0..=MAX_PROJECT_ITEMS => {}
            _ => Err(anyhow::anyhow!("Feature resolution exceeds limit"))?,
        }
        result = packages
            .iter()
            .enumerate()
            .map(|(index, package)| {
                EnabledFeatures::new(
                    package,
                    &dependencies[index],
                    &seeds.packages[index],
                    checkpoint,
                )
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        progress = seeds.propagate(packages, dependencies, &result, checkpoint)?;
    }
    Ok(result)
}

struct NativeCfg {
    options: CfgOptions,
    cargo: Vec<cargo_platform::Cfg>,
}

impl NativeCfg {
    fn new(profile: &SemanticProfile) -> anyhow::Result<Self> {
        match profile {
            profile
                if profile.target == native_target()
                    && profile.analyzer_version == ANALYZER_VERSION => {}
            _ => Err(anyhow::anyhow!(
                "In-process indexing requires the native target {} and analyzer {ANALYZER_VERSION}",
                native_target()
            ))?,
        }
        let mut options = CfgOptions::default();
        let mut cargo = Vec::new();
        for (enabled, flag) in [
            (cfg!(unix), "unix"),
            (cfg!(windows), "windows"),
            (true, "debug_assertions"),
        ] {
            if enabled {
                options.insert_atom(Symbol::intern(flag));
                cargo.push(flag.parse()?);
            }
        }
        for (key, value) in [
            ("target_arch", std::env::consts::ARCH.to_owned()),
            ("target_os", std::env::consts::OS.to_owned()),
            ("target_family", std::env::consts::FAMILY.to_owned()),
            ("target_pointer_width", usize::BITS.to_string()),
            (
                "target_endian",
                if cfg!(target_endian = "little") {
                    "little"
                } else {
                    "big"
                }
                .into(),
            ),
            (
                "target_env",
                [
                    (cfg!(target_env = "gnu"), "gnu"),
                    (cfg!(target_env = "musl"), "musl"),
                    (cfg!(target_env = "msvc"), "msvc"),
                ]
                .into_iter()
                .find_map(|(active, value)| active.then_some(value))
                .unwrap_or("")
                .into(),
            ),
            (
                "target_vendor",
                [
                    (cfg!(target_vendor = "apple"), "apple"),
                    (cfg!(target_vendor = "pc"), "pc"),
                ]
                .into_iter()
                .find_map(|(active, value)| active.then_some(value))
                .unwrap_or("unknown")
                .into(),
            ),
            ("panic", "unwind".into()),
        ] {
            options.insert_key_value(Symbol::intern(key), Symbol::intern(&value));
            cargo.push(format!("{key} = {value:?}").parse()?);
        }
        Ok(Self { options, cargo })
    }
}
