use crate::analysis::{AnalysisSession, Range, SymbolInfo};
use crate::cache::{TypedCache, TypedCacheDb};
use crate::rust_proj::RustProject;
use crate::utils::Utils;
use anyhow::anyhow;
use itertools::Itertools;
use ra_ap_ide::{AnalysisHost, LineIndex, TextSize};
use ra_ap_ide_db::SymbolKind;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_vfs::{FileId, Vfs, VfsPath};
use std::collections::HashMap;
use std::env;
use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::fs::DirEntry;

pub struct CurContext {
    pub cur_dir: PathBuf,
    cur_files: Vec<DirEntry>,
    pub rust_proj: RustProject,
    pub symbol_cache: TypedCache<SymbolInfo, SymbolInfo>,
    pub file_metas: HashMap<String, FileMeta>,
}

pub struct FileMeta {
    pub rpath: String,
    pub file_id: FileId,
    pub enums: Vec<EnumMeta>,
    pub structs: Vec<StructMeta>,
    pub functions: Vec<FunctionMeta>,
    pub type_alias: Vec<TypeAliasMeta>,
    pub traits: Vec<TraitMeta>,
    pub line_index: triomphe::Arc<LineIndex>,
}

pub struct EnumMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
    pub variants: Vec<EVariantMeta>,
}
pub struct EVariantMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
}
pub struct StructMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
    pub functions: Vec<FunctionMeta>,
}
pub struct FunctionMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
}
pub struct TypeAliasMeta {
    pub rpath: String,
    pub full_range: Range,
    pub name: String,
}
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
        writeln!(f, "## {}", self.rpath)?;

        if !self.structs.is_empty() {
            writeln!(f, "### Structs")?;
            for s in &self.structs {
                writeln!(f, "  {}", s)?;
            }
        }

        if !self.enums.is_empty() {
            writeln!(f, "### Enums")?;
            for e in &self.enums {
                writeln!(f, "  {}", e)?;
            }
        }

        if !self.traits.is_empty() {
            writeln!(f, "### Traits")?;
            for t in &self.traits {
                writeln!(f, "  {}", t)?;
            }
        }

        if !self.functions.is_empty() {
            writeln!(f, "### Functions")?;
            for func in &self.functions {
                writeln!(f, "  {}", func)?;
            }
        }

        if !self.type_alias.is_empty() {
            writeln!(f, "### Type Aliases")?;
            for ta in &self.type_alias {
                writeln!(f, "  {}", ta)?;
            }
        }

        Ok(())
    }
}

impl CurContext {
    pub async fn new() -> Result<CurContext, anyhow::Error> {
        let current_dir = env::current_dir()?;
        let files = Utils::get_dir_files(&current_dir).await?;
        let proj = RustProject::new(&current_dir)?;
        let mut symbol_cache = TypedCache::new(None).await;
        let file_metas = Self::get_file_metas(&mut symbol_cache, &proj).await?;
        Ok(CurContext {
            cur_dir: current_dir,
            cur_files: files,
            rust_proj: proj,
            symbol_cache,
            file_metas,
        })
    }

    pub async fn get_ctx(&self) -> String {
        let dir = self.cur_dir.to_str().unwrap_or("");
        let analytical_ctx = self.get_analytical_context().await.unwrap_or_default();
        format!(
            "# Current Context: \ncurrent directory: {dir}\n## Analytical Context: \nThe offset range are given along with the symbol information\n{analytical_ctx}"
        )
    }

    pub async fn get_analytical_context(&self) -> anyhow::Result<String> {
        Ok(Self::format_file_metas(&self.file_metas))
    }

    async fn get_file_metas(
        cache: &mut TypedCache<SymbolInfo, SymbolInfo>,
        rust_proj: &RustProject,
    ) -> anyhow::Result<HashMap<String, FileMeta>> {
        let symbols = rust_proj.get_all_proj_symbols(cache).await?;
        let session = rust_proj.new_analysis().await;
        let grouped = symbols.into_iter().into_group_map_by(|x| x.rpath.clone());
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
                    }
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

                let line_index = session.get_line_indecies(file_id)?;

                Ok((
                    k.clone(),
                    FileMeta {
                        rpath: k.clone(),
                        enums: enum_metas.into_values().collect(),
                        structs: struct_metas.into_values().collect(),
                        functions: stand_alone_func,
                        type_alias: type_alias_metas.into_values().collect(),
                        traits: traits_metas.into_values().collect(),
                        line_index,
                        file_id,
                    },
                ))
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

    pub fn format_file_metas(metas: &HashMap<String, FileMeta>) -> String {
        metas
            .values()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_analytical_context() {
        let mut ctx = CurContext::new()
            .await
            .expect("Failed to create CurContext");
        let context = ctx.get_ctx().await;
        println!("\n=== Analytical Context ===\n{}", context);
    }
}
