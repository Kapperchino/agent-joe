use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const KNOWLEDGE_PROTOCOL_VERSION: u32 = 1;
pub const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_GRAPH_ITEMS: usize = 1_000_000;
pub const ANALYZER_VERSION: &str = "0.0.344";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SourcePath(String);

impl TryFrom<String> for SourcePath {
    type Error = anyhow::Error;

    fn try_from(path: String) -> anyhow::Result<Self> {
        let valid = !path.is_empty()
            && !path.contains(['\\', ':', '\0'])
            && path.split('/').all(|part| !matches!(part, "" | "." | ".."));
        match valid {
            true => Ok(Self(path)),
            false => Err(anyhow::anyhow!(
                "Expected a normalized relative source path"
            )),
        }
    }
}

impl From<SourcePath> for String {
    fn from(path: SourcePath) -> Self {
        path.0
    }
}

impl SourcePath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "SpanData")]
pub struct ByteSpan {
    start: u32,
    end: u32,
}

#[derive(Deserialize)]
struct SpanData {
    start: u32,
    end: u32,
}

impl TryFrom<SpanData> for ByteSpan {
    type Error = anyhow::Error;

    fn try_from(span: SpanData) -> anyhow::Result<Self> {
        Self::new(span.start, span.end)
    }
}

impl ByteSpan {
    pub fn new(start: u32, end: u32) -> anyhow::Result<Self> {
        match start <= end {
            true => Ok(Self { start, end }),
            false => Err(anyhow::anyhow!("Source span ends before it starts")),
        }
    }

    pub fn start(self) -> u32 {
        self.start
    }

    pub fn end(self) -> u32 {
        self.end
    }

    pub fn len(self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    pub fn text(self, source: &str) -> anyhow::Result<&str> {
        source
            .get(self.start as usize..self.end as usize)
            .context("Source span is outside its file or splits a UTF-8 character")
    }

    pub fn lines(self, source: &str) -> anyhow::Result<LineSpan> {
        self.text(source)?;
        let before_start = &source[..self.start as usize];
        let before_end = &source[..self.end as usize];
        let start = before_start.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
        let end = before_end.bytes().filter(|byte| *byte == b'\n').count() as u32
            + 1
            + u32::from(!before_end.ends_with('\n') && !self.is_empty());
        Ok(LineSpan { start, end })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LineSpan {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SourceData")]
pub struct SourceFile {
    path: SourcePath,
    text: String,
}

#[derive(Deserialize)]
struct SourceData {
    path: SourcePath,
    text: String,
}

impl TryFrom<SourceData> for SourceFile {
    type Error = anyhow::Error;

    fn try_from(source: SourceData) -> anyhow::Result<Self> {
        Self::new(source.path, source.text)
    }
}

impl SourceFile {
    pub fn new(path: SourcePath, text: String) -> anyhow::Result<Self> {
        match text.len() <= MAX_FILE_BYTES && !text.contains('\0') {
            true => Ok(Self { path, text }),
            false => Err(anyhow::anyhow!(
                "Source file is binary or exceeds the size limit"
            )),
        }
    }

    pub fn path(&self) -> &SourcePath {
        &self.path
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn span(&self) -> ByteSpan {
        ByteSpan {
            start: 0,
            end: self.text.len() as u32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SymbolId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "selection", rename_all = "snake_case")]
pub enum Features {
    Default,
    None,
    Named {
        names: BTreeSet<String>,
        defaults: DefaultFeatures,
    },
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, turbo_code_macros::ToolSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DefaultFeatures {
    Enabled,
    Disabled,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    turbo_code_macros::ToolSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Configuration {
    Normal,
    Test,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticProfile {
    pub manifest: SourcePath,
    pub target: String,
    pub features: Features,
    pub configurations: BTreeSet<Configuration>,
    pub analyzer_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceLocation {
    pub path: SourcePath,
    pub span: ByteSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum SymbolOrigin {
    Source {
        location: SourceLocation,
    },
    Expansion {
        call_site: SourceLocation,
        definition_site: Option<SourceLocation>,
    },
    External {
        crate_name: String,
    },
}

impl SymbolOrigin {
    pub fn location(&self) -> Option<&SourceLocation> {
        match self {
            Self::Source { location } => Some(location),
            Self::Expansion { call_site, .. } => Some(call_site),
            Self::External { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    File,
    Module,
    Function,
    Method,
    Struct,
    Enum,
    Union,
    Variant,
    Field,
    Trait,
    Impl,
    TypeAlias,
    Constant,
    Static,
    Macro,
    Local,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionState {
    Resolved,
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub qualified_name: String,
    pub crate_key: String,
    pub configuration: Configuration,
    pub kind: SymbolKind,
    pub origin: SymbolOrigin,
    pub state: DefinitionState,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Contains,
    References,
    Calls,
    DispatchesToTrait,
    Implements,
    ExcludesTrait,
    ImplForType,
    AssociatedItemOf,
    Overrides,
    Expands,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "resolution", content = "target", rename_all = "snake_case")]
pub enum RelationTarget {
    Resolved(SymbolId),
    Candidates(BTreeSet<SymbolId>),
    Unresolved(String),
}

impl RelationTarget {
    pub fn symbols(&self) -> Box<dyn Iterator<Item = &SymbolId> + '_> {
        match self {
            Self::Resolved(id) => Box::new(std::iter::once(id)),
            Self::Candidates(ids) => Box::new(ids.iter()),
            Self::Unresolved(_) => Box::new(std::iter::empty()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Relation {
    pub source: SymbolId,
    pub kind: RelationKind,
    pub target: RelationTarget,
    pub site: Option<SourceLocation>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct IndexDiagnostic {
    pub message: String,
    pub location: Option<SourceLocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphData {
    pub version: u32,
    pub profile: SemanticProfile,
    pub sources: Vec<SourceFile>,
    pub symbols: Vec<Symbol>,
    pub relations: Vec<Relation>,
    pub diagnostics: Vec<IndexDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "GraphData", into = "GraphData")]
pub struct SemanticGraph(GraphData);

impl TryFrom<GraphData> for SemanticGraph {
    type Error = anyhow::Error;

    fn try_from(mut data: GraphData) -> anyhow::Result<Self> {
        let source_bytes = data
            .sources
            .iter()
            .map(|source| source.text.len())
            .fold(0usize, usize::saturating_add);
        let items = [
            data.sources.len(),
            data.symbols.len(),
            data.relations.len(),
            data.diagnostics.len(),
        ]
        .into_iter()
        .fold(0usize, usize::saturating_add);
        let valid = data.version == KNOWLEDGE_PROTOCOL_VERSION
            && source_bytes <= MAX_SOURCE_BYTES
            && items <= MAX_GRAPH_ITEMS
            && !data.profile.configurations.is_empty()
            && !data.profile.target.is_empty()
            && !data.profile.analyzer_version.is_empty();
        match valid {
            true => {}
            false => Err(anyhow::anyhow!(
                "Graph version, profile, or resource limits are invalid"
            ))?,
        }
        data.sources
            .sort_by(|left, right| left.path.cmp(&right.path));
        data.symbols.sort_by(|left, right| left.id.cmp(&right.id));
        data.relations.sort();
        data.relations.dedup();
        data.diagnostics.sort();
        data.diagnostics.dedup();
        let sources = data
            .sources
            .iter()
            .map(|source| (&source.path, source))
            .collect::<BTreeMap<_, _>>();
        let symbols = data
            .symbols
            .iter()
            .map(|symbol| (&symbol.id, symbol))
            .collect::<BTreeMap<_, _>>();
        let unique = sources.len() == data.sources.len()
            && symbols.len() == data.symbols.len()
            && symbols.keys().all(|id| !id.0.is_empty());
        match unique {
            true => {}
            false => Err(anyhow::anyhow!(
                "Graph contains duplicate paths or invalid symbol identities"
            ))?,
        }
        for symbol in &data.symbols {
            match data.profile.configurations.contains(&symbol.configuration)
                && !symbol.crate_key.is_empty()
                && !symbol.qualified_name.is_empty()
            {
                true => {}
                false => Err(anyhow::anyhow!(
                    "Symbol configuration or identity is invalid"
                ))?,
            }
            match &symbol.origin {
                SymbolOrigin::Source { location } => {
                    CheckedLocation::new(location, &sources)?;
                }
                SymbolOrigin::Expansion {
                    call_site,
                    definition_site,
                } => {
                    CheckedLocation::new(call_site, &sources)?;
                    if let Some(location) = definition_site {
                        CheckedLocation::new(location, &sources)?;
                    }
                }
                SymbolOrigin::External { .. } => {}
            }
        }
        for relation in &data.relations {
            let source = symbols
                .get(&relation.source)
                .context("Graph relation references an unknown source symbol")?;
            for id in relation.target.symbols() {
                let target = symbols
                    .get(id)
                    .context("Graph relation references an unknown target symbol")?;
                match source.configuration == target.configuration {
                    true => {}
                    false => Err(anyhow::anyhow!(
                        "Graph relation crosses semantic configurations"
                    ))?,
                }
            }
            match &relation.target {
                RelationTarget::Candidates(ids) if ids.is_empty() => {
                    Err(anyhow::anyhow!("Graph relation has no candidates"))?
                }
                RelationTarget::Unresolved(reason) if reason.is_empty() => {
                    Err(anyhow::anyhow!("Unresolved relation has no explanation"))?
                }
                _ => {}
            }
            if let Some(site) = &relation.site {
                CheckedLocation::new(site, &sources)?;
            }
        }
        for diagnostic in &data.diagnostics {
            if let Some(location) = &diagnostic.location {
                CheckedLocation::new(location, &sources)?;
            }
        }
        Ok(Self(data))
    }
}

impl From<SemanticGraph> for GraphData {
    fn from(graph: SemanticGraph) -> Self {
        graph.0
    }
}

impl SemanticGraph {
    pub fn data(&self) -> &GraphData {
        &self.0
    }
}

struct CheckedLocation;

impl CheckedLocation {
    fn new(
        location: &SourceLocation,
        sources: &BTreeMap<&SourcePath, &SourceFile>,
    ) -> anyhow::Result<Self> {
        let source = sources
            .get(&location.path)
            .context("Graph location references an unknown file")?;
        location.span.text(source.text())?;
        Ok(Self)
    }
}
