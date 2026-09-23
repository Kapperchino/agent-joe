use anyhow::Context;
use common_models::knowledge::*;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

mod ownership;
mod partition;
mod routing;

pub use routing::{Route, RoutePage, SymbolInspection};

pub const MAX_SHARDS: usize = 256;
const MAX_CONTEXT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct KnowledgeBudget {
    window: usize,
    response: u32,
    question: usize,
    margin: usize,
}

impl KnowledgeBudget {
    pub fn new(
        model_window: usize,
        configured_window: usize,
        response: u32,
    ) -> anyhow::Result<Self> {
        let window = model_window.min(configured_window).min(65_536);
        let response = response.min((window / 4) as u32);
        match window >= 4096 && response >= 1024 {
            true => Ok(Self {
                window,
                response,
                question: 4096.min(window / 8),
                margin: window / 10,
            }),
            false => Err(anyhow::anyhow!(
                "Knowledge context requires at least 4096 tokens and a 1024-token response reserve"
            )),
        }
    }

    pub fn window(self) -> usize {
        self.window
    }
    pub fn response(self) -> u32 {
        self.response
    }
    pub fn question(self) -> usize {
        self.question
    }
    pub fn input(self) -> usize {
        self.window - self.response as usize - self.margin
    }
    pub fn context(self) -> usize {
        self.input() - self.question
    }
    pub fn admits(self, tokens: usize) -> bool {
        tokens <= self.input()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnedSpan {
    pub location: SourceLocation,
    pub owner: Option<SymbolId>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShardSummary {
    pub index: usize,
    pub estimated_tokens: usize,
    pub owned_bytes: usize,
    pub paths: BTreeSet<SourcePath>,
    pub symbols: Vec<SymbolId>,
    pub symbol_count: usize,
}

#[derive(Debug)]
pub struct KnowledgeShard {
    pub summary: ShardSummary,
    pub context: String,
    pub owned: Vec<OwnedSpan>,
}

pub struct KnowledgeIndex {
    pub generation: String,
    pub graph: Arc<SemanticGraph>,
    pub budget: KnowledgeBudget,
    pub shards: Vec<KnowledgeShard>,
    routing: routing::Routing,
}

impl KnowledgeIndex {
    pub fn new(
        graph: Arc<SemanticGraph>,
        generation: String,
        budget: KnowledgeBudget,
        measure: &dyn Fn(&str) -> anyhow::Result<usize>,
    ) -> anyhow::Result<Self> {
        match !generation.is_empty() && generation.len() <= 64 {
            true => Ok(()),
            false => Err(anyhow::anyhow!("Invalid knowledge generation identity")),
        }?;
        let owned = ownership::ownership(&graph)?;
        let rendering = Rendering::new(&graph, &generation, budget, measure);
        let groups = partition::partition(&graph, owned, &rendering, budget, measure)?;
        let mut total_bytes = 0usize;
        let shards = groups
            .into_iter()
            .enumerate()
            .map(|(index, mut owned)| {
                owned.sort_by(|left, right| left.location.cmp(&right.location));
                let context = rendering.render(&owned)?;
                let estimated_tokens = measure(&context)?;
                total_bytes = total_bytes.saturating_add(context.len());
                match estimated_tokens <= budget.context()
                    && total_bytes <= MAX_CONTEXT_BYTES
                    && index < MAX_SHARDS
                {
                    true => Ok(()),
                    false => Err(anyhow::anyhow!(
                        "Knowledge partitions exceed request, actor, or total context limits"
                    )),
                }?;
                let symbols: BTreeSet<_> =
                    owned.iter().filter_map(|part| part.owner.clone()).collect();
                Ok(KnowledgeShard {
                    summary: ShardSummary {
                        index,
                        estimated_tokens,
                        owned_bytes: owned
                            .iter()
                            .map(|part| part.location.span.len() as usize)
                            .sum(),
                        paths: owned
                            .iter()
                            .map(|part| part.location.path.clone())
                            .collect(),
                        symbol_count: symbols.len(),
                        symbols: symbols.into_iter().take(64).collect(),
                    },
                    context,
                    owned,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        ownership::Coverage::new(&graph, &shards)?;
        let routing = routing::Routing::new(&graph, &shards)?;
        Ok(Self {
            generation,
            graph,
            budget,
            shards,
            routing,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolHeader {
    pub id: SymbolId,
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub signature: Option<String>,
    pub origin: SymbolOrigin,
    pub configuration: Configuration,
}

impl From<&Symbol> for SymbolHeader {
    fn from(symbol: &Symbol) -> Self {
        Self {
            id: symbol.id.clone(),
            name: clipped(&symbol.name, 128),
            qualified_name: clipped(&symbol.qualified_name, 256),
            kind: symbol.kind,
            signature: symbol
                .signature
                .as_deref()
                .map(|signature| clipped(signature, 256)),
            origin: symbol.origin.clone(),
            configuration: symbol.configuration,
        }
    }
}

fn clipped(text: &str, characters: usize) -> String {
    text.chars().take(characters).collect()
}

struct Rendering<'a> {
    graph: &'a SemanticGraph,
    generation: &'a str,
    budget: KnowledgeBudget,
    measure: &'a dyn Fn(&str) -> anyhow::Result<usize>,
    sources: BTreeMap<&'a SourcePath, &'a SourceFile>,
    symbols: BTreeMap<&'a SymbolId, &'a Symbol>,
    neighbors: BTreeMap<&'a SymbolId, BTreeSet<&'a SymbolId>>,
}

impl<'a> Rendering<'a> {
    fn new(
        graph: &'a SemanticGraph,
        generation: &'a str,
        budget: KnowledgeBudget,
        measure: &'a dyn Fn(&str) -> anyhow::Result<usize>,
    ) -> Self {
        let mut neighbors: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        for relation in &graph.data().relations {
            for target in relation.target.symbols() {
                neighbors
                    .entry(&relation.source)
                    .or_default()
                    .insert(target);
                neighbors
                    .entry(target)
                    .or_default()
                    .insert(&relation.source);
            }
        }
        Self {
            graph,
            generation,
            budget,
            measure,
            sources: graph
                .data()
                .sources
                .iter()
                .map(|source| (source.path(), source))
                .collect(),
            symbols: graph
                .data()
                .symbols
                .iter()
                .map(|symbol| (&symbol.id, symbol))
                .collect(),
            neighbors,
        }
    }

    fn render(&self, owned: &[OwnedSpan]) -> anyhow::Result<String> {
        #[derive(Serialize)]
        struct Fragment<'a> {
            path: &'a SourcePath,
            bytes: ByteSpan,
            lines: LineSpan,
            owner: &'a Option<SymbolId>,
            text: &'a str,
        }
        let mut ordered: Vec<_> = owned.iter().collect();
        ordered.sort_by(|left, right| left.location.cmp(&right.location));
        let fragments = ordered
            .into_iter()
            .map(|part| {
                let source = self
                    .sources
                    .get(&part.location.path)
                    .context("Missing owned source")?;
                Ok(Fragment {
                    path: &part.location.path,
                    bytes: part.location.span,
                    lines: part.location.span.lines(source.text())?,
                    owner: &part.owner,
                    text: part.location.span.text(source.text())?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let owners: BTreeSet<_> = owned
            .iter()
            .filter_map(|part| part.owner.as_ref())
            .collect();
        let mut primary_headers: Vec<_> = owners
            .iter()
            .take(8)
            .filter_map(|id| self.symbols.get(id))
            .map(|symbol| SymbolHeader::from(*symbol))
            .collect();
        let secondary: BTreeSet<_> = owners
            .iter()
            .flat_map(|owner| self.neighbors.get(owner).into_iter().flatten().copied())
            .filter(|id| !owners.contains(id))
            .take(8)
            .collect();
        let mut secondary: Vec<_> = secondary
            .into_iter()
            .filter_map(|id| self.symbols.get(id))
            .map(|symbol| SymbolHeader::from(*symbol))
            .collect();
        let render = |primary_headers: &[SymbolHeader], secondary: &[SymbolHeader]| {
            serde_json::to_string(&serde_json::json!({
                "generation": self.generation,
                "profile": self.graph.data().profile,
                "coverage": "Owned fragments are exhaustive for this shard, not necessarily complete items. Secondary signatures are bounded reference metadata, not owned source. Inactive/unresolved text is retained; resolution covers only the selected profile.",
                "diagnostic_count": self.graph.data().diagnostics.len(),
                "primary_headers": primary_headers,
                "secondary_headers": secondary,
                "owned": fragments,
            }))
        };
        let mut context = render(&primary_headers, &secondary)?;
        while (!primary_headers.is_empty() || !secondary.is_empty())
            && (self.measure)(&context)? > self.budget.context()
        {
            primary_headers.truncate(primary_headers.len() / 2);
            secondary.truncate(secondary.len() / 2);
            context = render(&primary_headers, &secondary)?;
        }
        Ok(context)
    }
}

#[cfg(test)]
mod tests;
