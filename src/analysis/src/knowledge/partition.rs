use super::*;
use petgraph::{
    algo::kosaraju_scc,
    graph::{DiGraph, NodeIndex},
};
use ra_ap_syntax::{AstNode, Edition};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Node {
    Symbol(SymbolId),
    File(SourcePath),
}

impl From<&OwnedSpan> for Node {
    fn from(part: &OwnedSpan) -> Self {
        match &part.owner {
            Some(owner) => Self::Symbol(owner.clone()),
            None => Self::File(part.location.path.clone()),
        }
    }
}

struct Forest {
    owned: Vec<Vec<OwnedSpan>>,
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
    postorder: Vec<usize>,
}

enum Visit {
    Enter {
        component: usize,
        parent: Option<usize>,
    },
    Exit(usize),
}

impl Forest {
    fn new(graph: &SemanticGraph, owned: Vec<OwnedSpan>) -> anyhow::Result<Self> {
        let mut dependencies = DiGraph::<Node, ()>::new();
        let mut nodes = BTreeMap::new();
        for node in graph
            .data()
            .symbols
            .iter()
            .map(|symbol| Node::Symbol(symbol.id.clone()))
            .chain(
                graph
                    .data()
                    .sources
                    .iter()
                    .map(|source| Node::File(source.path().clone())),
            )
        {
            nodes.insert(node.clone(), dependencies.add_node(node));
        }
        for relation in &graph.data().relations {
            for target in relation.target.symbols() {
                dependencies.add_edge(
                    nodes[&Node::Symbol(relation.source.clone())],
                    nodes[&Node::Symbol(target.clone())],
                    (),
                );
            }
        }
        let mut components = kosaraju_scc(&dependencies);
        let symbols: BTreeMap<_, _> = graph
            .data()
            .symbols
            .iter()
            .map(|symbol| (&symbol.id, symbol))
            .collect();
        let priority = |node: NodeIndex| {
            let rank = match &dependencies[node] {
                Node::Symbol(id) if symbols[id].name == "main" => 0,
                Node::Symbol(id)
                    if symbols[id]
                        .signature
                        .as_deref()
                        .is_some_and(|signature| signature.starts_with("pub")) =>
                {
                    1
                }
                Node::Symbol(_) => 2,
                Node::File(_) => 3,
            };
            (rank, dependencies[node].clone())
        };
        for component in &mut components {
            component.sort_by_key(|node| priority(*node));
        }
        components.sort_by_key(|component| priority(component[0]));
        let mut membership = vec![0; nodes.len()];
        for (index, component) in components.iter().enumerate() {
            for node in component {
                membership[node.index()] = index;
            }
        }
        let mut outgoing: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); components.len()];
        for edge in dependencies.raw_edges() {
            let source = membership[edge.source().index()];
            let target = membership[edge.target().index()];
            if source != target {
                outgoing[source].insert(target);
            }
        }
        let mut forest = Self {
            owned: vec![Vec::new(); components.len()],
            children: vec![Vec::new(); components.len()],
            roots: Vec::new(),
            postorder: Vec::new(),
        };
        for part in owned {
            let component = membership[nodes[&Node::from(&part)].index()];
            forest.owned[component].push(part);
        }
        let mut visited = BTreeSet::new();
        for root in 0..components.len() {
            let mut pending = vec![Visit::Enter {
                component: root,
                parent: None,
            }];
            while let Some(visit) = pending.pop() {
                match visit {
                    Visit::Enter { component, parent } if visited.insert(component) => {
                        match parent {
                            Some(parent) => forest.children[parent].push(component),
                            None => forest.roots.push(component),
                        }
                        pending.push(Visit::Exit(component));
                        pending.extend(outgoing[component].iter().rev().map(|child| {
                            Visit::Enter {
                                component: *child,
                                parent: Some(component),
                            }
                        }));
                    }
                    Visit::Enter { .. } => {}
                    Visit::Exit(component) => forest.postorder.push(component),
                }
            }
        }
        Ok(forest)
    }
}

struct Cutter<'a> {
    rendering: &'a Rendering<'a>,
    budget: KnowledgeBudget,
    measure: &'a dyn Fn(&str) -> anyhow::Result<usize>,
    boundaries: BTreeMap<SourcePath, BTreeSet<u32>>,
    measurements: usize,
    finished: Vec<Vec<OwnedSpan>>,
}

impl Cutter<'_> {
    fn fits(&mut self, parts: &[OwnedSpan]) -> anyhow::Result<bool> {
        self.measurements += 1;
        match self.measurements <= 200_000 {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Knowledge partition work exceeds 200000 budget measurements"
            )),
        }?;
        let bytes: usize = parts
            .iter()
            .map(|part| part.location.span.len() as usize)
            .sum();
        match parts.is_empty() {
            true => Ok(true),
            false if bytes > self.budget.window() * 16 => Ok(false),
            false => Ok((self.measure)(&self.rendering.render(parts)?)? <= self.budget.context()),
        }
    }

    fn finish(&mut self, parts: Vec<OwnedSpan>) -> anyhow::Result<()> {
        if !parts.is_empty() {
            match self.finished.len() < MAX_SHARDS {
                true => self.finished.push(parts),
                false => Err(anyhow::anyhow!(
                    "Knowledge preparation requires more than 256 shards; use a larger context or reduce discoverable inputs"
                ))?,
            }
        }
        Ok(())
    }

    fn split(&mut self, parts: Vec<OwnedSpan>) -> anyhow::Result<Vec<Vec<OwnedSpan>>> {
        let mut pending = vec![parts];
        let mut admitted = Vec::new();
        while let Some(parts) = pending.pop() {
            match self.fits(&parts)? {
                true => admitted.push(parts),
                false if parts.len() > 1 => {
                    let mut parts = parts;
                    let right = parts.split_off(parts.len() / 2);
                    pending.push(right);
                    pending.push(parts);
                }
                false => {
                    let part = parts
                        .into_iter()
                        .next()
                        .context("Cannot split an empty context")?;
                    let source = self.rendering.sources[&part.location.path];
                    let span = part.location.span;
                    let middle = span.start() + span.len() / 2;
                    let boundaries = self
                        .boundaries
                        .entry(part.location.path.clone())
                        .or_insert_with(|| match part.location.path.as_str().ends_with(".rs") {
                            true => {
                                ra_ap_syntax::SourceFile::parse(source.text(), Edition::Edition2024)
                                    .tree()
                                    .syntax()
                                    .descendants()
                                    .flat_map(|node| {
                                        [
                                            u32::from(node.text_range().start()),
                                            u32::from(node.text_range().end()),
                                        ]
                                    })
                                    .collect()
                            }
                            false => source
                                .text()
                                .match_indices('\n')
                                .map(|(index, _)| (index + 1) as u32)
                                .collect(),
                        });
                    let lower = span.start() + span.len() / 4;
                    let upper = span.end() - span.len() / 4;
                    let boundary = boundaries.range(lower..=upper).copied()
                        .filter(|position| *position > span.start() && *position < span.end())
                        .min_by_key(|position| position.abs_diff(middle))
                        .or_else(|| (span.start() + 1..span.end()).filter(|position| source.text().is_char_boundary(*position as usize)).min_by_key(|position| position.abs_diff(middle)))
                        .context("A source character plus required metadata cannot fit the knowledge request budget")?;
                    let mut right = part.clone();
                    right.location.span = ByteSpan::new(boundary, span.end())?;
                    let mut left = part;
                    left.location.span = ByteSpan::new(span.start(), boundary)?;
                    pending.push(vec![right]);
                    pending.push(vec![left]);
                }
            }
        }
        Ok(admitted)
    }
}

pub(super) fn partition(
    graph: &SemanticGraph,
    owned: Vec<OwnedSpan>,
    rendering: &Rendering<'_>,
    budget: KnowledgeBudget,
    measure: &dyn Fn(&str) -> anyhow::Result<usize>,
) -> anyhow::Result<Vec<Vec<OwnedSpan>>> {
    let mut forest = Forest::new(graph, owned)?;
    let mut cutter = Cutter {
        rendering,
        budget,
        measure,
        boundaries: BTreeMap::new(),
        measurements: 0,
        finished: Vec::new(),
    };
    let mut bags: Vec<Vec<OwnedSpan>> = vec![Vec::new(); forest.owned.len()];
    for component in forest.postorder {
        let own = std::mem::take(&mut forest.owned[component]);
        let groups = match cutter.fits(&own)? {
            true => vec![own],
            false => {
                let mut by_owner: BTreeMap<Node, Vec<OwnedSpan>> = BTreeMap::new();
                for part in own {
                    by_owner.entry(Node::from(&part)).or_default().push(part);
                }
                by_owner.into_values().collect()
            }
        };
        let mut bag = Vec::new();
        for group in groups {
            for part in cutter.split(group)? {
                let combined = bag
                    .iter()
                    .cloned()
                    .chain(part.iter().cloned())
                    .collect::<Vec<_>>();
                match cutter.fits(&combined)? {
                    true => bag = combined,
                    false => {
                        cutter.finish(bag)?;
                        bag = part;
                    }
                }
            }
        }
        for child in &forest.children[component] {
            let child = std::mem::take(&mut bags[*child]);
            let combined = bag
                .iter()
                .cloned()
                .chain(child.iter().cloned())
                .collect::<Vec<_>>();
            match cutter.fits(&combined)? {
                true => bag = combined,
                false => cutter.finish(child)?,
            }
        }
        bags[component] = bag;
    }
    let mut remainder = Vec::new();
    for root in forest.roots {
        let bag = std::mem::take(&mut bags[root]);
        let combined = remainder
            .iter()
            .cloned()
            .chain(bag.iter().cloned())
            .collect::<Vec<_>>();
        match cutter.fits(&combined)? {
            true => remainder = combined,
            false => {
                cutter.finish(remainder)?;
                remainder = bag;
            }
        }
    }
    cutter.finish(remainder)?;
    Ok(cutter.finished)
}
