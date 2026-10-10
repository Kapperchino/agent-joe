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

#[derive(Debug, Serialize)]
pub struct RelatedText {
    pub symbol: SymbolHeader,
    pub location: SourceLocation,
    pub lines: LineSpan,
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct FileContext {
    pub generation: String,
    pub path: SourcePath,
    pub primary_shards: BTreeSet<usize>,
    pub related_shards: BTreeSet<usize>,
    pub related: Vec<RelatedText>,
    pub related_total: usize,
    pub related_truncated: bool,
}

struct LocatedShard {
    span: ByteSpan,
    shard: usize,
}

struct SearchSymbol {
    name: String,
    qualified: String,
    signature: Option<String>,
    source: Option<usize>,
}

pub(super) struct Routing {
    symbols: FnvHashMap<SymbolId, usize>,
    sources: FnvHashMap<SourcePath, usize>,
    file_symbols: Vec<Vec<usize>>,
    relations: FnvHashMap<SymbolId, Vec<usize>>,
    search_symbols: Vec<SearchSymbol>,
    search_paths: Vec<String>,
    symbol_shards: FnvHashMap<SymbolId, BTreeSet<usize>>,
    file_shards: FnvHashMap<SourcePath, BTreeSet<usize>>,
    neighbors: FnvHashMap<SymbolId, BTreeSet<SymbolId>>,
}

impl Routing {
    pub fn new(graph: &SemanticGraph, shards: &[KnowledgeShard]) -> anyhow::Result<Self> {
        let mut files: FnvHashMap<SourcePath, Vec<LocatedShard>> = FnvHashMap::default();
        let mut file_shards: FnvHashMap<SourcePath, BTreeSet<usize>> = FnvHashMap::default();
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
        let mut symbol_shards = FnvHashMap::default();
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
        let sources: FnvHashMap<_, _> = graph
            .data()
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| (source.path().clone(), index))
            .collect();
        let search_symbols: Vec<_> = graph
            .data()
            .symbols
            .iter()
            .map(|symbol| SearchSymbol {
                name: symbol.name.to_lowercase(),
                qualified: symbol.qualified_name.to_lowercase(),
                signature: symbol.signature.as_deref().map(str::to_lowercase),
                source: symbol
                    .origin
                    .location()
                    .and_then(|location| sources.get(&location.path).copied()),
            })
            .collect();
        let mut file_symbols = vec![Vec::new(); graph.data().sources.len()];
        for (index, symbol) in search_symbols.iter().enumerate() {
            if let Some(source) = symbol.source {
                file_symbols[source].push(index);
            }
        }
        let mut relations: FnvHashMap<SymbolId, Vec<usize>> = FnvHashMap::default();
        let mut neighbors: FnvHashMap<SymbolId, BTreeSet<SymbolId>> = FnvHashMap::default();
        for (index, relation) in graph.data().relations.iter().enumerate() {
            relations
                .entry(relation.source.clone())
                .or_default()
                .push(index);
            for target in relation.target.symbols() {
                if target != &relation.source {
                    relations.entry(target.clone()).or_default().push(index);
                }
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
            sources,
            file_symbols,
            relations,
            search_symbols,
            search_paths: graph
                .data()
                .sources
                .iter()
                .map(|source| source.path().as_str().to_lowercase())
                .collect(),
            symbol_shards,
            file_shards,
            neighbors,
        })
    }

    fn relations(&self, id: &SymbolId) -> impl Iterator<Item = usize> + '_ {
        self.relations.get(id).into_iter().flatten().copied()
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
        self.words.iter().all(|word| text.contains(word))
    }
}

impl KnowledgeIndex {
    pub fn file_context(
        &self,
        path: &SourcePath,
        range: Option<LineSpan>,
    ) -> anyhow::Result<FileContext> {
        let source_index = self
            .routing
            .sources
            .get(path)
            .context("File is not in the prepared knowledge generation")?;
        let source = &self.graph.data().sources[*source_index];
        let selected: BTreeSet<_> = self.routing.file_symbols[*source_index]
            .iter()
            .map(|index| &self.graph.data().symbols[*index])
            .filter_map(|symbol| {
                symbol
                    .origin
                    .location()
                    .filter(|location| {
                        range.is_none_or(|range| {
                            location.span.lines(source.text()).is_ok_and(|lines| {
                                lines.start < range.end && range.start < lines.end
                            })
                        })
                    })
                    .map(|_| symbol.id.clone())
            })
            .collect();
        let relation_indices: BTreeSet<_> = selected
            .iter()
            .flat_map(|id| self.routing.relations(id))
            .collect();
        let adjacent: BTreeSet<_> = relation_indices
            .into_iter()
            .map(|index| &self.graph.data().relations[index])
            .flat_map(|relation| {
                relation.target.symbols().filter_map(|target| {
                    match (
                        selected.contains(&relation.source),
                        selected.contains(target),
                    ) {
                        (true, false) => Some(target.clone()),
                        (false, true) => Some(relation.source.clone()),
                        _ => None,
                    }
                })
            })
            .collect();
        let mut locations = BTreeSet::new();
        let related: Vec<_> = adjacent
            .iter()
            .filter_map(|id| self.routing.symbols.get(id))
            .map(|index| &self.graph.data().symbols[*index])
            .filter_map(|symbol| symbol.origin.location().map(|location| (symbol, location)))
            .filter(|(_, location)| locations.insert((*location).clone()))
            .collect();
        let related_total = related.len();
        let excerpts = related
            .into_iter()
            .take(8)
            .map(|(symbol, location)| {
                let source = self
                    .routing
                    .sources
                    .get(&location.path)
                    .map(|index| &self.graph.data().sources[*index])
                    .context("Missing related source")?;
                let original = location.span.text(source.text())?;
                let text = clipped(original, 2048);
                let span = ByteSpan::new(
                    location.span.start(),
                    location.span.start() + text.len() as u32,
                )?;
                Ok(RelatedText {
                    symbol: SymbolHeader::from(symbol),
                    location: SourceLocation {
                        path: location.path.clone(),
                        span,
                    },
                    lines: span.lines(source.text())?,
                    truncated: text.len() < original.len(),
                    text,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let primary_shards = self
            .routing
            .file_shards
            .get(path)
            .cloned()
            .unwrap_or_default();
        let related_shards = adjacent
            .iter()
            .flat_map(|id| self.routing.symbol_shards.get(id).into_iter().flatten())
            .filter(|shard| !primary_shards.contains(shard))
            .copied()
            .collect();
        Ok(FileContext {
            generation: self.generation.clone(),
            path: path.clone(),
            primary_shards,
            related_shards,
            related: excerpts,
            related_total,
            related_truncated: related_total > 8,
        })
    }

    pub fn search(&self, text: &str, offset: usize, limit: usize) -> anyhow::Result<RoutePage> {
        let query = Query::new(text, offset, limit)?;
        let symbols =
            self.routing
                .search_symbols
                .iter()
                .enumerate()
                .filter_map(|(index, searchable)| {
                    let symbol = &self.graph.data().symbols[index];
                    let ranked = |score, reason| Ranked {
                        score,
                        matched: Match::Symbol { index, reason },
                    };
                    match () {
                        _ if searchable.qualified == query.text => {
                            Some(ranked(120, "exact qualified symbol"))
                        }
                        _ if searchable.name == query.text => {
                            Some(ranked(110, "exact symbol name"))
                        }
                        _ if searchable.qualified.contains(&query.text) => {
                            Some(ranked(90, "qualified symbol substring"))
                        }
                        _ if searchable.source.is_some_and(|source| {
                            query.matches(&self.routing.search_paths[source])
                        }) =>
                        {
                            Some(ranked(70, "symbol source path"))
                        }
                        _ if searchable
                            .signature
                            .as_deref()
                            .is_some_and(|signature| query.matches(signature)) =>
                        {
                            Some(ranked(50, "symbol signature"))
                        }
                        _ => searchable
                            .source
                            .and_then(|source| {
                                symbol.origin.location().and_then(|location| {
                                    location
                                        .span
                                        .text(self.graph.data().sources[source].text())
                                        .ok()
                                })
                            })
                            .filter(|text| query.matches(&clipped(text, 512).to_lowercase()))
                            .map(|_| ranked(30, "leading documentation or source excerpt")),
                    }
                });
        let files = self
            .routing
            .search_paths
            .iter()
            .enumerate()
            .filter_map(|(index, path)| {
                let source = &self.graph.data().sources[index];
                let ranked = |score, reason| Ranked {
                    score,
                    matched: Match::File { index, reason },
                };
                match () {
                    _ if path == &query.text => Some(ranked(140, "exact source path")),
                    _ if query.matches(path) => Some(ranked(80, "source path")),
                    _ if !path.ends_with(".rs")
                        && query.matches(&clipped(source.text(), 2048).to_lowercase()) =>
                    {
                        Some(ranked(25, "leading document excerpt"))
                    }
                    _ => None,
                }
            });
        let mut ranked: Vec<_> = symbols.chain(files).collect();
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
        let mut found = self
            .routing
            .relations(id)
            .map(|index| &self.graph.data().relations[index]);
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
