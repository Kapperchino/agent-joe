use crate::analysis::Range;
use crate::rust_proj::RustProject;
use crate::symbol_info::SymbolInfo;
use crate::utils::Utils;
use futures::{future, StreamExt};
use itertools::Itertools;
use ra_ap_ide::LineIndex;
use ra_ap_ide_db::SymbolKind;
use ra_ap_vfs::VfsPath;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

pub struct FileMeta {
    pub line_index: triomphe::Arc<LineIndex>,
    pub rpath: String,
    pub file_id: u32,
    pub hash: Vec<u8>,
}
pub struct ProjMeta {
    pub enums: Vec<EnumMeta>,
    pub structs: Vec<StructMeta>,
    pub functions: Vec<FunctionMeta>,
    pub type_alias: Vec<TypeAliasMeta>,
    pub traits: Vec<TraitMeta>,
    pub files: HashMap<String, FileMeta>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct EnumMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
    pub variants: Vec<EVariantMeta>,
    pub functions: Vec<FunctionMeta>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct EVariantMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
    pub functions: Vec<FunctionMeta>,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct StructMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
    pub functions: Vec<FunctionMeta>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FunctionMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct TypeAliasMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct TraitMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
    pub functions: Vec<FunctionMeta>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImplMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
    pub functions: Vec<FunctionMeta>,
}

impl From<SymbolInfo> for EnumMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
            variants: vec![],
            functions: vec![],
        }
    }
}

impl From<SymbolInfo> for EVariantMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
            functions: vec![],
        }
    }
}

impl From<SymbolInfo> for StructMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
            functions: vec![],
        }
    }
}

impl From<SymbolInfo> for ImplMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
            functions: vec![],
        }
    }
}

impl From<SymbolInfo> for FunctionMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
        }
    }
}

impl From<SymbolInfo> for TypeAliasMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
        }
    }
}

impl From<SymbolInfo> for TraitMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
            functions: vec![],
        }
    }
}

impl ProjMeta {
    pub async fn get_proj_meta_from_symbols(
        vec: Vec<SymbolInfo>,
        rust_proj: &RustProject,
    ) -> anyhow::Result<ProjMeta> {
        let joe_2: HashMap<_, _> = vec.iter().map(|s| (s.name.clone(), s.clone())).collect();
        let joe = vec.into_iter().into_group_map_by(|x2| x2.kind.clone());
        let structs = Self::get_symbol_map(&joe, &SymbolKind::Struct);
        let enums = Self::get_symbol_map(&joe, &SymbolKind::Enum);
        let variants = Self::get_symbol_map(&joe, &SymbolKind::Variant);
        let traits = Self::get_symbol_map(&joe, &SymbolKind::Trait);
        let type_alias = Self::get_symbol_map(&joe, &SymbolKind::TypeAlias);
        let fields = Self::get_symbol_map(&joe, &SymbolKind::Field);

        let mut traits_metas: HashMap<String, TraitMeta> = Self::into_meta(&traits);
        let type_alias_metas: HashMap<String, TypeAliasMeta> = Self::into_meta(&type_alias);

        let (stand_alone, functions): (Vec<_>, _) =
            Self::get_symbol_map(&joe, &SymbolKind::Function)
                .into_values()
                .partition(|info| info.container_name.is_none());

        let stand_alone_func: Vec<_> = stand_alone.into_iter().map(|i| i.into()).collect();

        let mut struct_metas: HashMap<String, StructMeta> = Self::into_meta(&structs);

        let mut enum_metas: HashMap<String, EnumMeta> = Self::into_meta(&enums);

        let mut evariants_metas: HashMap<String, EVariantMeta> = Self::into_meta(&variants);

        let e_variants: HashMap<String, Vec<EVariantMeta>> = variants
            .into_values()
            .into_group_map_by(|v| v.container_name.clone().unwrap())
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().map(|i| i.into()).collect()))
            .collect::<HashMap<String, Vec<EVariantMeta>>>();

        let inner_funcs: HashMap<String, Vec<FunctionMeta>> = functions
            .iter()
            .into_group_map_by(|v| v.container_name.clone().unwrap())
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().map(|i| i.clone().into()).collect()))
            .collect::<HashMap<String, Vec<FunctionMeta>>>();

        inner_funcs.into_iter().for_each(|(k, v)| {
            if let Some(s_meta) = struct_metas.get_mut(&k) {
                s_meta.functions = v
            } else if let Some(t_meta) = traits_metas.get_mut(&k) {
                t_meta.functions = v
            } else if let Some(e_meta) = enum_metas.get_mut(&k) {
                e_meta.functions = v;
            } else if let Some(ev_meta) = evariants_metas.get_mut(&k) {
                ev_meta.functions = v
            } else {
                match joe_2.get(&k) {
                    None => {
                        //println!("{:?}", v);
                    }
                    Some(val) => {
                        //println!("{:?}", val);
                    }
                };
            }
        });

        e_variants.into_iter().for_each(|(k, v)| {
            if let Some(e_meta) = enum_metas.get_mut(&k) {
                e_meta.variants = v
            }
        });

        let files = Self::get_file_metas(rust_proj)
            .await?
            .into_iter()
            .map(|x| (x.rpath.clone(), x))
            .collect();

        Ok(ProjMeta {
            enums: enum_metas.into_values().collect(),
            structs: struct_metas.into_values().collect(),
            functions: stand_alone_func,
            type_alias: type_alias_metas.into_values().collect(),
            traits: traits_metas.into_values().collect(),
            files,
        })
    }

    fn into_meta<T: From<SymbolInfo>>(map: &HashMap<String, SymbolInfo>) -> HashMap<String, T> {
        map.iter()
            .map(|(k, info)| {
                let info = info.clone();
                let k = k.clone();
                let v: T = info.into();
                (k, v)
            })
            .collect()
    }

    fn get_symbol_map(
        symbols: &HashMap<SymbolKind, Vec<SymbolInfo>>,
        kind: &SymbolKind,
    ) -> HashMap<String, SymbolInfo> {
        symbols
            .get(kind)
            .map(|t| {
                let res: HashMap<_, _> = t
                    .into_iter()
                    .cloned()
                    .map(|info| (info.name.clone(), info.clone()))
                    .collect();
                res
            })
            .unwrap_or_default()
    }

    pub async fn get_file_metas(proj: &RustProject) -> anyhow::Result<Vec<FileMeta>> {
        let hashes = Self::get_file_hashes(proj).await?;
        let analysis = proj.new_analysis().await;
        hashes
            .into_iter()
            .map(|(pb, hash)| {
                let path = pb.to_string_lossy().to_string();
                let file_id = proj
                    .vfs
                    .lock()
                    .unwrap()
                    .file_id(&VfsPath::new_real_path(path.clone()))
                    .unwrap()
                    .0;

                let line_index = analysis.get_line_indecies(file_id)?;

                Ok(FileMeta {
                    line_index,
                    rpath: path,
                    file_id: file_id.index(),
                    hash,
                })
            })
            .collect()
    }

    pub async fn get_file_hashes(proj: &RustProject) -> anyhow::Result<Vec<(PathBuf, Vec<u8>)>> {
        let contents = Self::get_files(proj).await?;
        let results: Vec<_> = futures::stream::iter(contents)
            .map(|(path, content)| {
                tokio::spawn(
                    async move { (path, blake3::hash(content.as_bytes()).as_bytes().to_vec()) },
                )
            })
            .buffer_unordered(100)
            .collect::<Vec<_>>()
            .await;

        let res = results.into_iter().collect::<Result<Vec<_>, _>>()?;
        Ok(res)
    }

    async fn get_files(proj: &RustProject) -> anyhow::Result<Vec<(PathBuf, String)>> {
        let anal = proj.new_analysis().await;
        future::join_all(anal.get_work_files().into_iter().flat_map(|file| {
            file.path.into_abs_path().map(async |path| {
                Utils::get_file_content(&path.clone().into())
                    .await
                    .map(|content| (path.into(), content))
            })
        }))
        .await
        .into_iter()
        .collect()
    }
}

impl Display for FunctionMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "fn {}() @ {}:{}-{}", self.name, self.rpath, self.full_range.start, self.full_range.end)
    }
}

impl Display for EVariantMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Display for EnumMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "- enum {} @ {}:{}-{}", self.name, self.rpath, self.full_range.start, self.full_range.end)?;
        if !self.variants.is_empty() {
            let variants: Vec<_> = self.variants.iter().map(|v| v.name.as_str()).collect();
            write!(f, "\n    variants: [{}]", variants.join(", "))?;
        }
        if !self.functions.is_empty() {
            write!(f, "\n    methods:")?;
            for func in &self.functions {
                write!(f, "\n      - {}() L{}-{}", func.name, func.full_range.start, func.full_range.end)?;
            }
        }
        Ok(())
    }
}

impl Display for StructMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "- struct {} @ {}:{}-{}", self.name, self.rpath, self.full_range.start, self.full_range.end)?;
        if !self.functions.is_empty() {
            write!(f, "\n    methods:")?;
            for func in &self.functions {
                write!(f, "\n      - {}() L{}-{}", func.name, func.full_range.start, func.full_range.end)?;
            }
        }
        Ok(())
    }
}

impl Display for TypeAliasMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "- type {} @ {}:{}-{}", self.name, self.rpath, self.full_range.start, self.full_range.end)
    }
}

impl Display for TraitMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "- trait {} @ {}:{}-{}", self.name, self.rpath, self.full_range.start, self.full_range.end)?;
        if !self.functions.is_empty() {
            write!(f, "\n    methods:")?;
            for func in &self.functions {
                write!(f, "\n      - {}() L{}-{}", func.name, func.full_range.start, func.full_range.end)?;
            }
        }
        Ok(())
    }
}

impl Display for ProjMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "# Project Symbols")?;
        writeln!(f, "Format: <name> @ <file>:<start_line>-<end_line>")?;
        writeln!(f)?;

        if !self.structs.is_empty() {
            writeln!(f, "## Structs ({})", self.structs.len())?;
            for s in &self.structs {
                writeln!(f, "{}", s)?;
            }
            writeln!(f)?;
        }

        if !self.enums.is_empty() {
            writeln!(f, "## Enums ({})", self.enums.len())?;
            for e in &self.enums {
                writeln!(f, "{}", e)?;
            }
            writeln!(f)?;
        }

        if !self.traits.is_empty() {
            writeln!(f, "## Traits ({})", self.traits.len())?;
            for t in &self.traits {
                writeln!(f, "{}", t)?;
            }
            writeln!(f)?;
        }

        if !self.functions.is_empty() {
            writeln!(f, "## Standalone Functions ({})", self.functions.len())?;
            for func in &self.functions {
                writeln!(f, "- {}", func)?;
            }
            writeln!(f)?;
        }

        if !self.type_alias.is_empty() {
            writeln!(f, "## Type Aliases ({})", self.type_alias.len())?;
            for ta in &self.type_alias {
                writeln!(f, "{}", ta)?;
            }
        }

        Ok(())
    }
}
