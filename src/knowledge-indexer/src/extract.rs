use std::collections::{BTreeMap, HashMap};

use anyhow::Context;
use ra_ap_hir::{AsAssocItem, AssocItem, AssocItemContainer, CallableKind, Semantics};
use ra_ap_ide::{NavigationTarget, TryToNav};
use ra_ap_ide_db::{
    RootDatabase,
    defs::{Definition, IdentClass, NameClass},
};
use ra_ap_syntax::{AstNode, SyntaxNode, TextRange, ast};
use ra_ap_vfs::FileId;

use crate::*;

pub struct IndexedSource {
    pub file_id: Option<FileId>,
    pub source: SourceFile,
}

pub fn extract(
    db: &RootDatabase,
    sources: Vec<IndexedSource>,
    profile: SemanticProfile,
    configuration: Configuration,
) -> anyhow::Result<SemanticGraph> {
    extract_checked(db, sources, profile, configuration, &|| Ok(()))
}

pub(crate) fn extract_checked(
    db: &RootDatabase,
    sources: Vec<IndexedSource>,
    profile: SemanticProfile,
    configuration: Configuration,
    checkpoint: &dyn Fn() -> anyhow::Result<()>,
) -> anyhow::Result<SemanticGraph> {
    ra_ap_hir::attach_db(db, || {
        extract_attached(db, sources, profile, configuration, checkpoint)
    })
}

fn extract_attached(
    db: &RootDatabase,
    sources: Vec<IndexedSource>,
    profile: SemanticProfile,
    configuration: Configuration,
    checkpoint: &dyn Fn() -> anyhow::Result<()>,
) -> anyhow::Result<SemanticGraph> {
    let mut index = Index {
        sema: Semantics::new(db),
        paths: sources
            .iter()
            .filter_map(|source| source.file_id.map(|id| (id, source.source.path().clone())))
            .collect(),
        definitions: HashMap::new(),
        symbols: BTreeMap::new(),
        relations: Vec::new(),
        diagnostics: Vec::new(),
        configuration,
        visited_nodes: 0,
        checkpoint,
    };
    for source in &sources {
        checkpoint()?;
        let file = index.file_symbol(&source.source)?;
        match source.file_id {
            Some(file_id) => {
                let root = index.sema.parse_guess_edition(file_id);
                let modules = index.sema.file_to_module_defs(file_id).collect::<Vec<_>>();
                if modules.len() > 1 {
                    index.diagnostics.push(IndexDiagnostic {
                        message: "This file belongs to multiple crate/module contexts; source queries use the analyzer-selected context".into(),
                        location: Some(SourceLocation { path: source.source.path().clone(), span: source.source.span() }),
                    });
                }
                let owner = match modules.first() {
                    Some(module) => {
                        let module = index.definition(Definition::Module(*module))?;
                        index.relate(
                            &file,
                            RelationKind::Contains,
                            RelationTarget::Resolved(module.clone()),
                            None,
                        );
                        module
                    }
                    None => file,
                };
                match root.syntax().text().to_string() == source.source.text() {
                    true => index.walk(root.syntax().clone(), owner)?,
                    false => Err(anyhow::anyhow!(
                        "Analyzer source differs from captured file {}",
                        source.source.path().as_str()
                    ))?,
                }
            }
            None => index.diagnostics.push(IndexDiagnostic {
                message: "Captured text has no Rust semantic file in this profile".into(),
                location: Some(SourceLocation {
                    path: source.source.path().clone(),
                    span: source.source.span(),
                }),
            }),
        }
    }
    SemanticGraph::try_from(GraphData {
        version: KNOWLEDGE_PROTOCOL_VERSION,
        profile,
        sources: sources.into_iter().map(|source| source.source).collect(),
        symbols: index.symbols.into_values().collect(),
        relations: index.relations,
        diagnostics: index.diagnostics,
    })
}

struct Frame {
    node: SyntaxNode,
    owner: SymbolId,
    expansion_depth: usize,
}

impl Frame {
    fn root(node: SyntaxNode, owner: SymbolId) -> Self {
        Self {
            node,
            owner,
            expansion_depth: 0,
        }
    }

    fn expansion(&self, node: SyntaxNode, owner: &SymbolId) -> anyhow::Result<Self> {
        match self.expansion_depth {
            0..64 => Ok(Self {
                node,
                owner: owner.clone(),
                expansion_depth: self.expansion_depth + 1,
            }),
            _ => Err(anyhow::anyhow!("Semantic macro expansion depth exceeds 64")),
        }
    }

    fn children(&self, owner: &SymbolId) -> impl Iterator<Item = Self> {
        let owner = owner.clone();
        self.node.children().map(move |node| Self {
            node,
            owner: owner.clone(),
            expansion_depth: self.expansion_depth,
        })
    }
}

enum ScopeTransition {
    Inherit,
    Enter(SymbolId),
}

struct Index<'db> {
    sema: Semantics<'db, RootDatabase>,
    paths: BTreeMap<FileId, SourcePath>,
    definitions: HashMap<Definition<'db>, SymbolId>,
    symbols: BTreeMap<SymbolId, Symbol>,
    relations: Vec<Relation>,
    diagnostics: Vec<IndexDiagnostic>,
    configuration: Configuration,
    visited_nodes: usize,
    checkpoint: &'db dyn Fn() -> anyhow::Result<()>,
}

impl<'db> Index<'db> {
    fn file_symbol(&mut self, source: &SourceFile) -> anyhow::Result<SymbolId> {
        let id = identity(&serde_json::json!([
            "file",
            self.configuration,
            source.path()
        ]))?;
        self.symbols.insert(
            id.clone(),
            Symbol {
                id: id.clone(),
                name: source.path().as_str().into(),
                qualified_name: source.path().as_str().into(),
                crate_key: "source".into(),
                configuration: self.configuration,
                kind: SymbolKind::File,
                origin: SymbolOrigin::Source {
                    location: SourceLocation {
                        path: source.path().clone(),
                        span: source.span(),
                    },
                },
                state: DefinitionState::Resolved,
                signature: None,
            },
        );
        Ok(id)
    }

    fn location(&self, file_id: FileId, range: TextRange) -> Option<SourceLocation> {
        Some(SourceLocation {
            path: self.paths.get(&file_id)?.clone(),
            span: ByteSpan::new(range.start().into(), range.end().into()).ok()?,
        })
    }

    fn site(&self, node: &SyntaxNode) -> Option<SourceLocation> {
        let range = self.sema.original_range(node).into_file_id(self.sema.db);
        self.location(range.file_id, range.range)
    }

    fn nav_location(&self, nav: &NavigationTarget) -> Option<SourceLocation> {
        self.location(nav.file_id, nav.full_range)
    }

    fn definition(&mut self, definition: Definition<'db>) -> anyhow::Result<SymbolId> {
        match self.definitions.get(&definition) {
            Some(id) => Ok(id.clone()),
            None => self.insert_definition(definition),
        }
    }

    fn insert_definition(&mut self, definition: Definition<'db>) -> anyhow::Result<SymbolId> {
        let db = self.sema.db;
        let krate = definition.krate(db);
        let edition = krate
            .map(|krate| krate.edition(db))
            .unwrap_or(ra_ap_syntax::Edition::Edition2024);
        let crate_name = krate
            .and_then(|krate| krate.display_name(db))
            .map(|name| name.to_string())
            .unwrap_or_else(|| "builtin".into());
        let crate_root = krate
            .and_then(|krate| self.paths.get(&krate.root_file(db)))
            .map(SourcePath::as_str)
            .unwrap_or("external");
        let crate_key = format!(
            "{crate_name}@{}:{crate_root}",
            krate
                .and_then(|krate| krate.version(db))
                .unwrap_or_default()
        );
        let name = definition
            .name(db)
            .map(|name| name.display(db, edition).to_string())
            .unwrap_or_else(|| match definition {
                Definition::Module(_) | Definition::Crate(_) => crate_name.clone(),
                _ => "impl".into(),
            });
        let modules = definition
            .canonical_module_path(db)
            .into_iter()
            .flatten()
            .filter_map(|module| module.name(db))
            .map(|name| name.display(db, edition).to_string())
            .collect::<Vec<_>>();
        let qualified_name = std::iter::once(crate_name.as_str())
            .chain(modules.iter().map(String::as_str))
            .chain(std::iter::once(name.as_str()))
            .collect::<Vec<_>>()
            .join("::");
        let nav = definition.try_to_nav(&self.sema);
        let location = nav
            .as_ref()
            .and_then(|nav| self.nav_location(&nav.call_site));
        let definition_site = nav
            .as_ref()
            .and_then(|nav| nav.def_site.as_ref())
            .and_then(|nav| self.nav_location(nav));
        let expansion = nav.as_ref().is_some_and(|nav| {
            let root = self.sema.parse_guess_edition(nav.call_site.file_id);
            root.syntax()
                .text_range()
                .contains_range(nav.call_site.full_range)
                && root
                    .syntax()
                    .covering_element(nav.call_site.full_range)
                    .ancestors()
                    .any(|node| ast::MacroCall::can_cast(node.kind()))
        });
        let origin = match (location, definition_site, expansion) {
            (Some(call_site), definition_site, true)
            | (Some(call_site), definition_site @ Some(_), _) => SymbolOrigin::Expansion {
                call_site,
                definition_site,
            },
            (Some(location), None, false) => SymbolOrigin::Source { location },
            (None, _, _) => SymbolOrigin::External { crate_name },
        };
        let kind = definition_kind(definition);
        let focus = nav
            .as_ref()
            .map(|nav| nav.call_site.focus_or_full_range())
            .map(|range| (u32::from(range.start()), u32::from(range.end())));
        let id = identity(&serde_json::json!([
            self.configuration,
            crate_key,
            qualified_name,
            kind,
            origin,
            focus
        ]))?;
        let symbol = Symbol {
            id: id.clone(),
            name,
            qualified_name,
            crate_key,
            configuration: self.configuration,
            kind,
            origin,
            state: DefinitionState::Resolved,
            signature: nav.and_then(|nav| nav.call_site.description),
        };
        match self.symbols.contains_key(&id) {
            true => Err(anyhow::anyhow!(
                "Distinct semantic definitions have the same portable identity: {}",
                symbol.qualified_name
            ))?,
            false => {
                self.symbols.insert(id.clone(), symbol);
            }
        }
        self.definitions.insert(definition, id.clone());
        self.associations(definition, &id)?;
        self.limits()?;
        Ok(id)
    }

    fn associations(
        &mut self,
        definition: Definition<'db>,
        source: &SymbolId,
    ) -> anyhow::Result<()> {
        let db = self.sema.db;
        let associated = match definition {
            Definition::Function(item) => item.as_assoc_item(db),
            Definition::Const(item) => item.as_assoc_item(db),
            Definition::TypeAlias(item) => item.as_assoc_item(db),
            _ => None,
        };
        if let Some(item) = associated {
            let container = match item.container(db) {
                AssocItemContainer::Trait(item) => Definition::Trait(item),
                AssocItemContainer::Impl(item) => Definition::SelfType(item),
            };
            let target = self.definition(container)?;
            self.relate(
                source,
                RelationKind::AssociatedItemOf,
                RelationTarget::Resolved(target),
                None,
            );
            if let Some(trait_) = item.implemented_trait(db) {
                for candidate in trait_.items(db).into_iter().filter(|candidate| {
                    candidate.name(db) == item.name(db)
                        && std::mem::discriminant(candidate) == std::mem::discriminant(&item)
                }) {
                    let target = self.definition(associated_definition(candidate))?;
                    self.relate(
                        source,
                        RelationKind::Overrides,
                        RelationTarget::Resolved(target),
                        None,
                    );
                }
            }
        }
        if let Definition::SelfType(item) = definition {
            if let Some(trait_) = item.trait_(db) {
                let target = self.definition(Definition::Trait(trait_))?;
                let kind = match item.is_negative(db) {
                    true => RelationKind::ExcludesTrait,
                    false => RelationKind::Implements,
                };
                self.relate(source, kind, RelationTarget::Resolved(target), None);
            }
            if let Some(adt) = item.self_ty(db).as_adt() {
                let target = self.definition(Definition::Adt(adt))?;
                self.relate(
                    source,
                    RelationKind::ImplForType,
                    RelationTarget::Resolved(target),
                    None,
                );
            }
        }
        Ok(())
    }

    fn relate(
        &mut self,
        source: &SymbolId,
        kind: RelationKind,
        target: RelationTarget,
        site: Option<SourceLocation>,
    ) {
        self.relations.push(Relation {
            source: source.clone(),
            kind,
            target,
            site,
        });
    }

    fn walk(&mut self, node: SyntaxNode, owner: SymbolId) -> anyhow::Result<()> {
        let mut pending = vec![Frame::root(node, owner)];
        while let Some(frame) = pending.pop() {
            (self.checkpoint)()?;
            self.visited_nodes += 1;
            self.limits()?;
            let owner = match self.declaration(&frame.node, &frame.owner)? {
                ScopeTransition::Inherit => frame.owner.clone(),
                ScopeTransition::Enter(owner) => owner,
            };
            if let Some(name) = ast::NameRef::cast(frame.node.clone()) {
                self.reference(&owner, &name)?;
            }
            if let Some(call) = ast::CallableExpr::cast(frame.node.clone()) {
                self.call(&owner, call)?;
            }
            for expansion in self.expansions(&frame.node) {
                pending.push(frame.expansion(expansion, &owner)?);
            }
            pending.extend(frame.children(&owner));
        }
        Ok(())
    }

    fn declaration(
        &mut self,
        node: &SyntaxNode,
        owner: &SymbolId,
    ) -> anyhow::Result<ScopeTransition> {
        let declaration = ast::Item::can_cast(node.kind())
            || ast::RecordField::can_cast(node.kind())
            || ast::Variant::can_cast(node.kind())
            || ast::IdentPat::can_cast(node.kind())
            || ast::SelfParam::can_cast(node.kind());
        let definition = ast::Impl::cast(node.clone())
            .and_then(|item| self.sema.to_def(&item))
            .map(Definition::SelfType)
            .or_else(|| {
                declaration
                    .then(|| {
                        node.children()
                            .find_map(ast::Name::cast)
                            .and_then(|name| NameClass::classify(&self.sema, &name))
                            .and_then(NameClass::defined)
                    })
                    .flatten()
            });
        match definition {
            Some(definition) => {
                let id = self.definition(definition)?;
                self.relate(
                    owner,
                    RelationKind::Contains,
                    RelationTarget::Resolved(id.clone()),
                    self.site(node),
                );
                Ok(match definition_kind(definition) {
                    SymbolKind::Local | SymbolKind::Other => ScopeTransition::Inherit,
                    _ => ScopeTransition::Enter(id),
                })
            }
            None => Ok(ScopeTransition::Inherit),
        }
    }

    fn expansions(&mut self, node: &SyntaxNode) -> Vec<SyntaxNode> {
        let mut expansions = Vec::new();
        if let Some(call) = ast::MacroCall::cast(node.clone()) {
            match self.sema.expand_macro_call(&call) {
                Some(expansion) => expansions.push(expansion.value),
                None => self.diagnostics.push(IndexDiagnostic {
                    message: "Macro expansion is unavailable".into(),
                    location: self.site(node),
                }),
            }
        }
        if let Some(expansion) =
            ast::Item::cast(node.clone()).and_then(|item| self.sema.expand_attr_macro(&item))
        {
            let diagnostic = expansion.err.map(|error| IndexDiagnostic {
                message: format!("Attribute expansion: {error:?}"),
                location: self.site(node),
            });
            self.diagnostics.extend(diagnostic);
            expansions.push(expansion.value.value);
        }
        let derived =
            ast::Meta::cast(node.clone()).and_then(|meta| self.sema.expand_derive_macro(&meta));
        for expansion in derived.into_iter().flatten().flatten() {
            let diagnostic = expansion.err.map(|error| IndexDiagnostic {
                message: format!("Derive expansion: {error:?}"),
                location: self.site(node),
            });
            self.diagnostics.extend(diagnostic);
            expansions.push(expansion.value);
        }
        expansions
    }

    fn reference(&mut self, owner: &SymbolId, name: &ast::NameRef) -> anyhow::Result<()> {
        let definitions = name
            .syntax()
            .first_token()
            .and_then(|token| IdentClass::classify_token(&self.sema, &token))
            .map(|class| class.definitions())
            .unwrap_or_default();
        let site = self.site(name.syntax());
        match definitions.is_empty() {
            true => self.relate(
                owner,
                RelationKind::References,
                RelationTarget::Unresolved(name.text().to_string()),
                site,
            ),
            false => {
                for (definition, _) in definitions {
                    let target = self.definition(definition)?;
                    self.relate(
                        owner,
                        RelationKind::References,
                        RelationTarget::Resolved(target),
                        site.clone(),
                    );
                }
            }
        }
        Ok(())
    }

    fn call(&mut self, owner: &SymbolId, call: ast::CallableExpr) -> anyhow::Result<()> {
        let definition = match &call {
            ast::CallableExpr::MethodCall(call) => self
                .sema
                .resolve_method_call(call)
                .map(Definition::Function),
            ast::CallableExpr::Call(call) => call
                .expr()
                .and_then(|expr| self.sema.type_of_expr(&expr))
                .and_then(|ty| ty.original.as_callable(self.sema.db))
                .and_then(|callable| match callable.kind() {
                    CallableKind::Function(function) => Some(Definition::Function(function)),
                    CallableKind::TupleStruct(item) => Some(Definition::Adt(item.into())),
                    CallableKind::TupleEnumVariant(item) => Some(Definition::EnumVariant(item)),
                    _ => None,
                }),
        };
        let site = self.site(call.syntax());
        match definition {
            Some(definition) => {
                let kind = match definition {
                    Definition::Function(function)
                        if function
                            .as_assoc_item(self.sema.db)
                            .and_then(|item| item.container_trait(self.sema.db))
                            .is_some() =>
                    {
                        RelationKind::DispatchesToTrait
                    }
                    _ => RelationKind::Calls,
                };
                let target = self.definition(definition)?;
                self.relate(owner, kind, RelationTarget::Resolved(target), site);
            }
            None => self.relate(
                owner,
                RelationKind::Calls,
                RelationTarget::Unresolved("Indirect or unresolved callable".into()),
                site,
            ),
        }
        Ok(())
    }

    fn limits(&self) -> anyhow::Result<()> {
        match self.symbols.len() + self.relations.len() + self.diagnostics.len() <= MAX_GRAPH_ITEMS
            && self.visited_nodes <= MAX_GRAPH_ITEMS * 16
        {
            true => Ok(()),
            false => Err(anyhow::anyhow!(
                "Semantic extraction exceeds graph resource limits"
            )),
        }
    }
}

fn associated_definition(item: AssocItem) -> Definition<'static> {
    match item {
        AssocItem::Function(item) => Definition::Function(item),
        AssocItem::Const(item) => Definition::Const(item),
        AssocItem::TypeAlias(item) => Definition::TypeAlias(item),
    }
}

fn definition_kind(definition: Definition<'_>) -> SymbolKind {
    match definition {
        Definition::Function(_) => SymbolKind::Function,
        Definition::Adt(ra_ap_hir::Adt::Struct(_)) => SymbolKind::Struct,
        Definition::Adt(ra_ap_hir::Adt::Enum(_)) => SymbolKind::Enum,
        Definition::Adt(ra_ap_hir::Adt::Union(_)) => SymbolKind::Union,
        Definition::Trait(_) => SymbolKind::Trait,
        Definition::SelfType(_) => SymbolKind::Impl,
        Definition::Module(_) | Definition::Crate(_) => SymbolKind::Module,
        Definition::Field(_) | Definition::TupleField(_) => SymbolKind::Field,
        Definition::EnumVariant(_) => SymbolKind::Variant,
        Definition::Const(_) => SymbolKind::Constant,
        Definition::Static(_) => SymbolKind::Static,
        Definition::TypeAlias(_) => SymbolKind::TypeAlias,
        Definition::Macro(_) => SymbolKind::Macro,
        Definition::Local(_) => SymbolKind::Local,
        _ => SymbolKind::Other,
    }
}

fn identity(value: &serde_json::Value) -> anyhow::Result<SymbolId> {
    let serialized = serde_json::to_vec(value).context("Serialize semantic identity")?;
    Ok(SymbolId(blake3::hash(&serialized).to_hex().to_string()))
}

#[cfg(test)]
mod traversal_tests {
    use super::*;

    #[test]
    fn expansion_frames_enforce_depth_without_resetting_for_syntax_children() {
        let node = ast::SourceFile::parse("fn item() {}", ra_ap_syntax::Edition::Edition2024)
            .tree()
            .syntax()
            .clone();
        let owner = SymbolId("owner".into());
        let frame = (0..64)
            .try_fold(Frame::root(node.clone(), owner.clone()), |frame, _| {
                frame.expansion(node.clone(), &owner)
            })
            .unwrap();
        let child = frame.children(&owner).next().unwrap();
        assert_eq!(child.expansion_depth, 64);
        assert_eq!(child.owner, owner);
        assert!(child.expansion(node, &owner).is_err());
    }
}
