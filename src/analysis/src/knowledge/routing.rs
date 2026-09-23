use super::*;

#[derive(Debug, Clone, Serialize)]
pub struct Route {
    pub symbol: Option<SymbolHeader>,
    pub path: Option<SourcePath>,
    pub primary_shards: BTreeSet<usize>,
    pub related_shards: BTreeSet<usize>,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct RoutePage {
    pub generation: String,
    pub hits: Vec<Route>,
    pub total: usize,
    pub next_offset: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct RelationSummary {
    pub source: SymbolId,
    pub kind: RelationKind,
    pub targets: Vec<SymbolId>,
    pub target_count: usize,
    pub unresolved: Option<String>,
    pub site: Option<SourceLocation>,
}

#[derive(Debug, Serialize)]
pub struct SymbolInspection {
    pub generation: String,
    pub symbol: SymbolHeader,
    pub primary_shards: BTreeSet<usize>,
    pub related_shards: BTreeSet<usize>,
    pub relations: Vec<RelationSummary>,
    pub relations_truncated: bool,
}

struct LocatedShard {
    span: ByteSpan,
    shard: usize,
}

pub(super) struct Routing {
    symbols: BTreeMap<SymbolId, usize>,
    symbol_shards: BTreeMap<SymbolId, BTreeSet<usize>>,
    file_shards: BTreeMap<SourcePath, BTreeSet<usize>>,
    neighbors: BTreeMap<SymbolId, BTreeSet<SymbolId>>,
}

impl Routing {
    pub fn new(graph: &SemanticGraph, shards: &[KnowledgeShard]) -> anyhow::Result<Self> {
        let mut files: BTreeMap<SourcePath, Vec<LocatedShard>> = BTreeMap::new();
        let mut file_shards: BTreeMap<SourcePath, BTreeSet<usize>> = BTreeMap::new();
        for shard in shards {
            for part in &shard.owned {
                files
                    .entry(part.location.path.clone())
                    .or_default()
                    .push(LocatedShard {
                        span: part.location.span,
                        shard: shard.summary.index,
                    });
                file_shards
                    .entry(part.location.path.clone())
                    .or_default()
                    .insert(shard.summary.index);
            }
        }
        for parts in files.values_mut() {
            parts.sort_by_key(|part| part.span);
        }
        let mut symbol_shards = BTreeMap::new();
        let mut assignments = 0usize;
        for symbol in &graph.data().symbols {
            let mut ownership = BTreeSet::new();
            if let Some(location) = symbol.origin.location() {
                let parts = files
                    .get(&location.path)
                    .context("Missing source ownership for routing")?;
                let start = parts.partition_point(|part| {
                    part.span.end() <= location.span.start() && !part.span.is_empty()
                });
                match location.span.is_empty() {
                    true => {
                        if let Some(part) = parts.get(start).or_else(|| parts.last()) {
                            ownership.insert(part.shard);
                        }
                    }
                    false => {
                        for part in parts[start..]
                            .iter()
                            .take_while(|part| part.span.start() < location.span.end())
                        {
                            ownership.insert(part.shard);
                        }
                    }
                }
            }
            assignments = assignments.saturating_add(ownership.len());
            match assignments <= 4_000_000 {
                true => {
                    symbol_shards.insert(symbol.id.clone(), ownership);
                }
                false => Err(anyhow::anyhow!(
                    "Knowledge routing exceeds four million ownership assignments"
                ))?,
            }
        }
        let mut neighbors: BTreeMap<SymbolId, BTreeSet<SymbolId>> = BTreeMap::new();
        for relation in &graph.data().relations {
            for target in relation.target.symbols() {
                for (from, to) in [(&relation.source, target), (target, &relation.source)] {
                    let adjacent = neighbors.entry(from.clone()).or_default();
                    if adjacent.len() < 64 {
                        adjacent.insert(to.clone());
                    }
                }
            }
        }
        Ok(Self {
            symbols: graph
                .data()
                .symbols
                .iter()
                .enumerate()
                .map(|(index, symbol)| (symbol.id.clone(), index))
                .collect(),
            symbol_shards,
            file_shards,
            neighbors,
        })
    }

    fn related(&self, id: &SymbolId) -> BTreeSet<usize> {
        let mut seen = BTreeSet::from([id.clone()]);
        let mut frontier = vec![id.clone()];
        let mut shards = BTreeSet::new();
        for _ in 0..2 {
            let mut next = Vec::new();
            for source in frontier {
                for target in self.neighbors.get(&source).into_iter().flatten() {
                    if seen.len() < 64 && seen.insert(target.clone()) {
                        shards.extend(
                            self.symbol_shards
                                .get(target)
                                .into_iter()
                                .flatten()
                                .copied(),
                        );
                        next.push(target.clone());
                    }
                }
            }
            frontier = next;
        }
        for primary in self.symbol_shards.get(id).into_iter().flatten() {
            shards.remove(primary);
        }
        shards
    }
}

enum Match {
    Symbol { index: usize, reason: &'static str },
    File { index: usize, reason: &'static str },
}

struct Ranked {
    score: usize,
    matched: Match,
}

struct Query {
    text: String,
    words: Vec<String>,
    offset: usize,
    limit: usize,
}

impl Query {
    fn new(text: &str, offset: usize, limit: usize) -> anyhow::Result<Self> {
        let text = text.trim().to_lowercase();
        let words = text
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        match !text.is_empty()
            && text.len() <= 2048
            && words.len() <= 32
            && (1..=100).contains(&limit)
            && offset <= MAX_GRAPH_ITEMS
        {
            true => Ok(Self {
                text,
                words,
                offset,
                limit,
            }),
            false => Err(anyhow::anyhow!(
                "Knowledge search needs 1–32 words, at most 2048 bytes, a limit of 1–100, and a bounded offset"
            )),
        }
    }

    fn matches(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        self.words.iter().all(|word| text.contains(word))
    }
}

impl KnowledgeIndex {
    pub fn search(&self, text: &str, offset: usize, limit: usize) -> anyhow::Result<RoutePage> {
        let query = Query::new(text, offset, limit)?;
        let sources: BTreeMap<_, _> = self
            .graph
            .data()
            .sources
            .iter()
            .map(|source| (source.path(), source))
            .collect();
        let mut ranked = Vec::new();
        for (index, symbol) in self.graph.data().symbols.iter().enumerate() {
            let name = symbol.name.to_lowercase();
            let qualified = symbol.qualified_name.to_lowercase();
            let matched = match () {
                _ if qualified == query.text => Some((120, "exact qualified symbol")),
                _ if name == query.text => Some((110, "exact symbol name")),
                _ if qualified.contains(&query.text) => Some((90, "qualified symbol substring")),
                _ if symbol
                    .origin
                    .location()
                    .is_some_and(|location| query.matches(location.path.as_str())) =>
                {
                    Some((70, "symbol source path"))
                }
                _ if symbol
                    .signature
                    .as_deref()
                    .is_some_and(|signature| query.matches(signature)) =>
                {
                    Some((50, "symbol signature"))
                }
                _ => symbol
                    .origin
                    .location()
                    .and_then(|location| {
                        sources.get(&location.path).map(|source| (location, source))
                    })
                    .and_then(|(location, source)| location.span.text(source.text()).ok())
                    .filter(|text| query.matches(&clipped(text, 512)))
                    .map(|_| (30, "leading documentation or source excerpt")),
            };
            if let Some((score, reason)) = matched {
                ranked.push(Ranked {
                    score,
                    matched: Match::Symbol { index, reason },
                });
            }
        }
        for (index, source) in self.graph.data().sources.iter().enumerate() {
            let path = source.path().as_str().to_lowercase();
            let matched = match () {
                _ if path == query.text => Some((140, "exact source path")),
                _ if query.matches(&path) => Some((80, "source path")),
                _ if !path.ends_with(".rs") && query.matches(&clipped(source.text(), 2048)) => {
                    Some((25, "leading document excerpt"))
                }
                _ => None,
            };
            if let Some((score, reason)) = matched {
                ranked.push(Ranked {
                    score,
                    matched: Match::File { index, reason },
                });
            }
        }
        ranked.sort_by_key(|hit| std::cmp::Reverse(hit.score));
        let total = ranked.len();
        let hits = ranked
            .into_iter()
            .skip(query.offset)
            .take(query.limit)
            .map(|hit| match hit.matched {
                Match::Symbol { index, reason } => {
                    let symbol = &self.graph.data().symbols[index];
                    Route {
                        symbol: Some(SymbolHeader::from(symbol)),
                        path: symbol
                            .origin
                            .location()
                            .map(|location| location.path.clone()),
                        primary_shards: self
                            .routing
                            .symbol_shards
                            .get(&symbol.id)
                            .cloned()
                            .unwrap_or_default(),
                        related_shards: self.routing.related(&symbol.id),
                        reason: reason.into(),
                    }
                }
                Match::File { index, reason } => {
                    let path = self.graph.data().sources[index].path();
                    Route {
                        symbol: None,
                        path: Some(path.clone()),
                        primary_shards: self
                            .routing
                            .file_shards
                            .get(path)
                            .cloned()
                            .unwrap_or_default(),
                        related_shards: BTreeSet::new(),
                        reason: reason.into(),
                    }
                }
            })
            .collect();
        let next = query.offset.saturating_add(query.limit);
        Ok(RoutePage {
            generation: self.generation.clone(),
            hits,
            total,
            next_offset: (next < total).then_some(next),
        })
    }

    pub fn inspect(&self, id: &SymbolId) -> anyhow::Result<SymbolInspection> {
        let symbol = self
            .routing
            .symbols
            .get(id)
            .map(|index| &self.graph.data().symbols[*index])
            .context("Unknown semantic symbol identity")?;
        let mut found = self.graph.data().relations.iter().filter(|relation| {
            &relation.source == id || relation.target.symbols().any(|target| target == id)
        });
        let relations = found
            .by_ref()
            .take(32)
            .map(|relation| RelationSummary {
                source: relation.source.clone(),
                kind: relation.kind,
                targets: relation.target.symbols().take(8).cloned().collect(),
                target_count: relation.target.symbols().count(),
                unresolved: match &relation.target {
                    RelationTarget::Unresolved(reason) => Some(clipped(reason, 256)),
                    _ => None,
                },
                site: relation.site.clone(),
            })
            .collect();
        Ok(SymbolInspection {
            generation: self.generation.clone(),
            symbol: SymbolHeader::from(symbol),
            primary_shards: self
                .routing
                .symbol_shards
                .get(id)
                .cloned()
                .unwrap_or_default(),
            related_shards: self.routing.related(id),
            relations,
            relations_truncated: found.next().is_some(),
        })
    }
}
