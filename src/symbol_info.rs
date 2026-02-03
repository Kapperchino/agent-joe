use crate::analysis::Range;
use crate::cache::{TypedCache, TypedCacheDb};
use crate::rust_proj::RustProject;
use anyhow::anyhow;
use ra_ap_ide::{NavigationTarget, StructureNode, StructureNodeKind};
use ra_ap_ide_db::SymbolKind;
use ra_ap_vfs::{FileId, Vfs};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize)]
#[serde(remote = "SymbolKind")]
enum SymbolKindDef {
    Attribute,
    BuiltinAttr,
    Const,
    ConstParam,
    Derive,
    DeriveHelper,
    Enum,
    Field,
    Function,
    Method,
    Impl,
    InlineAsmRegOrRegClass,
    Label,
    LifetimeParam,
    Local,
    Macro,
    ProcMacro,
    Module,
    SelfParam,
    SelfType,
    Static,
    Struct,
    ToolModule,
    Trait,
    TypeAlias,
    TypeParam,
    Union,
    ValueParam,
    Variant,
}

#[derive(Serialize, Deserialize, Debug, Clone, Eq, PartialEq, Hash)]
pub struct SymbolInfo {
    pub rpath: String,
    pub full_range: Range,
    pub focus_range: Option<Range>,
    pub name: String,
    #[serde(with = "SymbolKindDef")]
    pub kind: SymbolKind,
    pub container_name: Option<String>,
    pub docs: Option<String>,
    pub description: Option<String>,
}

impl SymbolInfo {
    pub(crate) fn from_nav(n: NavigationTarget, vfs: &Vfs) -> Self {
        SymbolInfo {
            rpath: vfs.file_path(n.file_id).to_string(),
            full_range: Range {
                start: n.full_range.start().into(),
                end: n.full_range.end().into(),
            },
            focus_range: n.focus_range.map(|t| Range {
                start: t.start().into(),
                end: t.end().into(),
            }),
            name: n.name.to_string(),
            kind: n.kind.unwrap(),
            container_name: n.container_name.map(|s| s.to_string()),
            docs: n.docs.map(|d| d.as_str().to_string()),
            description: n.description,
        }
    }

    pub fn from_file_structs(
        file_id: FileId,
        file_structs: Vec<StructureNode>,
        path_buf: PathBuf,
    ) -> Vec<Self> {
        file_structs
            .iter()
            .filter(|fs| match fs.kind {
                StructureNodeKind::SymbolKind(_) => true,
                _ => false,
            })
            .map(|fs| SymbolInfo {
                rpath: path_buf.to_string_lossy().to_string(),
                full_range: Range {
                    start: fs.node_range.start().into(),
                    end: fs.node_range.end().into(),
                },
                focus_range: Some(Range {
                    start: fs.navigation_range.start().into(),
                    end: fs.navigation_range.end().into(),
                }),
                name: fs.label.clone(),
                kind: match fs.kind {
                    StructureNodeKind::SymbolKind(kind) => kind,
                    _ => unreachable!(),
                },
                container_name: fs
                    .parent
                    .and_then(|t| file_structs.get(t).map(|node| node.label.clone())),
                docs: None,
                description: fs.detail.clone(),
            })
            .collect()
    }

    pub async fn get_symbols_from_cache(
        cache: &TypedCache<SymbolInfo, SymbolInfo>,
    ) -> anyhow::Result<Vec<SymbolInfo>> {
        cache.read_transaction(|db| {
            let res: Vec<_> = db.iter()?.collect();
            Ok(res.into_iter().collect())
        })
    }

    pub async fn get_symbols_with_cache_write(
        rust_proj: &RustProject,
        cache: &mut TypedCache<SymbolInfo, SymbolInfo>,
    ) -> anyhow::Result<Vec<SymbolInfo>> {
        let is_empty = cache.read_transaction(|db| db.is_empty())?;
        if is_empty {
            let symbols = rust_proj.get_all_proj_symbols().await?;
            cache.transaction(|db: &mut TypedCacheDb<_, _>| {
                let (_, errs): (Vec<_>, Vec<_>) = symbols
                    .iter()
                    .map(|v| db.put(v, v))
                    .partition(|r| r.is_ok());
                // report on this
                let errs: Vec<_> = errs.into_iter().flat_map(|x1| x1.err()).collect();
                println!("{:?}", errs);
                if !errs.is_empty() {
                    return Err(anyhow!("Error while wrriting to DB! {:?}", errs));
                }
                Ok(symbols)
            })
        } else {
            cache.read_transaction(|db| {
                let res: Vec<_> = db.iter()?.collect();
                Ok(res.into_iter().collect())
            })
        }
    }
}
