use super::*;

#[derive(Default)]
struct Boundary {
    start: Vec<Candidate>,
    end: Vec<Candidate>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Candidate {
    bytes: u32,
    priority: u8,
    id: SymbolId,
}

pub(super) fn ownership(graph: &SemanticGraph) -> anyhow::Result<Vec<OwnedSpan>> {
    let mut events: BTreeMap<&SourcePath, BTreeMap<u32, Boundary>> = graph
        .data()
        .sources
        .iter()
        .map(|source| {
            (
                source.path(),
                [
                    (0, Boundary::default()),
                    (source.span().end(), Boundary::default()),
                ]
                .into(),
            )
        })
        .collect();
    for symbol in &graph.data().symbols {
        if let SymbolOrigin::Source { location } = &symbol.origin
            && !matches!(symbol.kind, SymbolKind::Local | SymbolKind::Other)
            && !location.span.is_empty()
        {
            let priority = match symbol.kind {
                SymbolKind::File => 2,
                SymbolKind::Module => 1,
                _ => 0,
            };
            let candidate = Candidate {
                bytes: location.span.len(),
                priority,
                id: symbol.id.clone(),
            };
            let events = events
                .get_mut(&location.path)
                .context("Unknown ownership source")?;
            events
                .entry(location.span.start())
                .or_default()
                .start
                .push(candidate.clone());
            events
                .entry(location.span.end())
                .or_default()
                .end
                .push(candidate);
        }
    }
    let mut owned: Vec<OwnedSpan> = Vec::new();
    for source in &graph.data().sources {
        let mut active = BTreeSet::new();
        let mut previous = 0;
        for (position, boundary) in events
            .remove(source.path())
            .context("Missing source boundaries")?
        {
            if position > previous {
                let owner = active
                    .first()
                    .map(|candidate: &Candidate| candidate.id.clone());
                match owned.last_mut() {
                    Some(last)
                        if last.location.path == *source.path()
                            && last.owner == owner
                            && last.location.span.end() == previous =>
                    {
                        last.location.span = ByteSpan::new(last.location.span.start(), position)?;
                    }
                    _ => owned.push(OwnedSpan {
                        location: SourceLocation {
                            path: source.path().clone(),
                            span: ByteSpan::new(previous, position)?,
                        },
                        owner,
                    }),
                }
            }
            for candidate in boundary.end {
                active.remove(&candidate);
            }
            active.extend(boundary.start);
            previous = position;
        }
        if source.text().is_empty() {
            owned.push(OwnedSpan {
                location: SourceLocation {
                    path: source.path().clone(),
                    span: source.span(),
                },
                owner: None,
            });
        }
    }
    Ok(owned)
}

pub(super) struct Coverage;

impl Coverage {
    pub fn new(graph: &SemanticGraph, shards: &[KnowledgeShard]) -> anyhow::Result<Self> {
        let mut by_file: BTreeMap<&SourcePath, Vec<ByteSpan>> = BTreeMap::new();
        for part in shards.iter().flat_map(|shard| &shard.owned) {
            by_file
                .entry(&part.location.path)
                .or_default()
                .push(part.location.span);
        }
        for source in &graph.data().sources {
            let mut spans = by_file
                .remove(source.path())
                .context("A source file has no primary shard")?;
            spans.sort();
            let mut end = 0;
            for span in &spans {
                match span.start() == end {
                    true => end = span.end(),
                    false => Err(anyhow::anyhow!(
                        "Knowledge source coverage has a gap or overlap"
                    ))?,
                }
            }
            match end == source.span().end()
                && (source.text().is_empty().then_some(spans.len()).unwrap_or(1) == 1)
            {
                true => Ok(()),
                false => Err(anyhow::anyhow!(
                    "Knowledge source coverage is incomplete or duplicates an empty file"
                )),
            }?;
        }
        match by_file.is_empty() {
            true => Ok(Self),
            false => Err(anyhow::anyhow!(
                "Knowledge ownership contains unknown source files"
            )),
        }
    }
}
