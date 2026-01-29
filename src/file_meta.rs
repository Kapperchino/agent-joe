use crate::analysis::Range;
use crate::rust_proj::RustProject;
use crate::symbol_info::SymbolInfo;
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
    pub data: FileMetaData,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct FileMetaData {
    pub rpath: String,
    pub file_id: u32,
    pub enums: Vec<EnumMeta>,
    pub structs: Vec<StructMeta>,
    pub functions: Vec<FunctionMeta>,
    pub type_alias: Vec<TypeAliasMeta>,
    pub traits: Vec<TraitMeta>,
    pub hash: Vec<u8>,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct EnumMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
    pub variants: Vec<EVariantMeta>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct EVariantMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct StructMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
    pub functions: Vec<FunctionMeta>,
}
#[derive(Serialize, Deserialize, Clone)]
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
}

impl From<SymbolInfo> for EnumMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
            variants: vec![],
        }
    }
}

impl From<SymbolInfo> for EVariantMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
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
        }
    }
}

impl FileMetaData {
    pub fn get_file_meta_datas_cache_miss(
        vec: Vec<SymbolInfo>,
        rust_proj: &RustProject,
        hashes: &HashMap<PathBuf, Vec<u8>>,
    ) -> anyhow::Result<HashMap<String, FileMetaData>> {
        let grouped = vec.into_iter().into_group_map_by(|x| x.rpath.clone());
        grouped
            .into_iter()
            .map(|(k, v)| {
                let joe = v.iter().into_group_map_by(|x2| x2.kind.clone());
                let structs = Self::get_symbol_map(&joe, &SymbolKind::Struct);
                let enums = Self::get_symbol_map(&joe, &SymbolKind::Enum);
                let variants = Self::get_symbol_map(&joe, &SymbolKind::Variant);
                let traits = Self::get_symbol_map(&joe, &SymbolKind::Trait);
                let type_alias = Self::get_symbol_map(&joe, &SymbolKind::TypeAlias);

                let traits_metas: HashMap<String, TraitMeta> = Self::into_meta(&traits);
                let type_alias_metas: HashMap<String, TypeAliasMeta> = Self::into_meta(&type_alias);

                let (stand_alone, functions): (Vec<_>, _) =
                    Self::get_symbol_map(&joe, &SymbolKind::Function)
                        .into_values()
                        .partition(|info| info.container_name.is_none());

                let stand_alone_func: Vec<_> = stand_alone.into_iter().map(|i| i.into()).collect();

                let mut struct_metas: HashMap<String, StructMeta> = Self::into_meta(&structs);

                let inner_funcs: HashMap<String, Vec<FunctionMeta>> = functions
                    .into_iter()
                    .into_group_map_by(|v| v.container_name.clone().unwrap())
                    .into_iter()
                    .map(|(k, v)| (k, v.into_iter().map(|i| i.into()).collect()))
                    .collect::<HashMap<String, Vec<FunctionMeta>>>();

                inner_funcs.into_iter().for_each(|(k, v)| {
                    if let Some(s_meta) = struct_metas.get_mut(&k) {
                        s_meta.functions = v
                    } else { println!("{}",k) }
                });

                let mut enum_metas: HashMap<String, EnumMeta> = Self::into_meta(&enums);

                let e_variants: HashMap<String, Vec<EVariantMeta>> = variants
                    .into_values()
                    .into_group_map_by(|v| v.container_name.clone().unwrap())
                    .into_iter()
                    .map(|(k, v)| (k, v.into_iter().map(|i| i.into()).collect()))
                    .collect::<HashMap<String, Vec<EVariantMeta>>>();

                e_variants.into_iter().for_each(|(k, v)| {
                    if let Some(e_meta) = enum_metas.get_mut(&k) {
                        e_meta.variants = v
                    }
                });

                let file_id = rust_proj
                    .vfs
                    .lock()
                    .unwrap()
                    .file_id(&VfsPath::new_real_path(k.clone()))
                    .unwrap()
                    .0;

                let hash = hashes
                    .get(&PathBuf::from(k.clone()))
                    .cloned()
                    .unwrap_or(Vec::new());

                Ok((
                    k.clone(),
                    FileMetaData {
                        rpath: k.clone(),
                        enums: enum_metas.into_values().collect(),
                        structs: struct_metas.into_values().collect(),
                        functions: stand_alone_func,
                        type_alias: type_alias_metas.into_values().collect(),
                        traits: traits_metas.into_values().collect(),
                        file_id: file_id.index(),
                        hash: hash,
                    },
                ))
            })
            .collect()
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
        symbols: &HashMap<SymbolKind, Vec<&SymbolInfo>>,
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
}

impl Display for FunctionMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "fn {}() [{}:{}]",
            self.name, self.full_range.start, self.full_range.end
        )
    }
}

impl Display for EVariantMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl Display for EnumMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "enum {} [{}:{}]",
            self.name, self.full_range.start, self.full_range.end
        )?;
        if !self.variants.is_empty() {
            let variants: Vec<_> = self.variants.iter().map(|v| v.name.as_str()).collect();
            write!(f, " {{ {} }}", variants.join(", "))?;
        }
        Ok(())
    }
}

impl Display for StructMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "struct {} [{}:{}]",
            self.name, self.full_range.start, self.full_range.end
        )?;
        if !self.functions.is_empty() {
            writeln!(f)?;
            for func in &self.functions {
                writeln!(f, "    {}", func)?;
            }
        }
        Ok(())
    }
}

impl Display for TypeAliasMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "type {} [{}:{}]",
            self.name, self.full_range.start, self.full_range.end
        )
    }
}

impl Display for TraitMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "trait {} [{}:{}]",
            self.name, self.full_range.start, self.full_range.end
        )
    }
}

impl Display for FileMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "## {}", self.data.rpath)?;

        if !self.data.structs.is_empty() {
            writeln!(f, "### Structs")?;
            for s in &self.data.structs {
                writeln!(f, "  {}", s)?;
            }
        }

        if !self.data.enums.is_empty() {
            writeln!(f, "### Enums")?;
            for e in &self.data.enums {
                writeln!(f, "  {}", e)?;
            }
        }

        if !self.data.traits.is_empty() {
            writeln!(f, "### Traits")?;
            for t in &self.data.traits {
                writeln!(f, "  {}", t)?;
            }
        }

        if !self.data.functions.is_empty() {
            writeln!(f, "### Functions")?;
            for func in &self.data.functions {
                writeln!(f, "  {}", func)?;
            }
        }

        if !self.data.type_alias.is_empty() {
            writeln!(f, "### Type Aliases")?;
            for ta in &self.data.type_alias {
                writeln!(f, "  {}", ta)?;
            }
        }

        Ok(())
    }
}
