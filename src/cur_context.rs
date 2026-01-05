use crate::analysis::{AnalysisSession, Range, SymbolInfo};
use crate::cache::{TypedCache, TypedCacheDb};
use crate::utils::Utils;
use anyhow::anyhow;
use itertools::Itertools;
use ra_ap_ide::{AnalysisHost, TextSize};
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
    cur_dir: PathBuf,
    cur_files: Vec<DirEntry>,
    rust_proj: RustProject,
    symbol_cache: TypedCache<SymbolInfo, SymbolInfo>,
}
pub struct RustProject {
    pub analysis_host: Arc<Mutex<AnalysisHost>>,
    pub vfs: Vfs,
    pub files: HashMap<FileId, VfsPath>,
}

pub struct FileMeta {
    pub rpath: String,
    pub enums: Vec<EnumMeta>,
    pub structs: Vec<StructMeta>,
    pub functions: Vec<FunctionMeta>,
    pub type_alias: Vec<TypeAliasMeta>,
    pub traits: Vec<TraitMeta>,
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
        let symbol_cache = TypedCache::new(None).await;
        Ok(CurContext {
            cur_dir: current_dir,
            cur_files: files,
            rust_proj: proj,
            symbol_cache,
        })
    }

    pub async fn get_ctx(&mut self) -> String {
        let dir = self.cur_dir.to_str().unwrap_or("");
        let analytical_ctx = Self::get_analytical_context(&mut self.symbol_cache, &self.rust_proj)
            .await
            .unwrap_or_default();
        format!("Current Context: \ncurrent directory: {dir}\n{analytical_ctx}")
    }

    pub async fn get_analytical_context(
        cache: &mut TypedCache<SymbolInfo, SymbolInfo>,
        rust_proj: &RustProject,
    ) -> anyhow::Result<String> {
        let symbols = rust_proj.get_all_proj_symbols(cache).await?;
        let grouped = symbols.into_iter().into_group_map_by(|x| x.rpath.clone());
        let res_vec: Vec<_> = Self::get_file_metas(grouped);
        Ok(Self::format_file_metas(res_vec))
    }

    fn get_file_metas(grouped: HashMap<String, Vec<SymbolInfo>>) -> Vec<FileMeta> {
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

                let total = v
                    .iter()
                    .map(|s| {
                        let SymbolInfo {
                            rpath,
                            full_range,
                            name,
                            kind,
                            container_name,
                            docs,
                            focus_range,
                        } = s;
                        format!(
                            "     {name}  {:?}  {:?}  {:?}",
                            kind, container_name, focus_range
                        )
                    })
                    .reduce(|acc, x1| format!("{acc}\n{x1}"))
                    .unwrap();
                format!("path:{k}\n{total}\n");

                FileMeta {
                    rpath: k.clone(),
                    enums: enum_metas.into_values().collect(),
                    structs: struct_metas.into_values().collect(),
                    functions: stand_alone_func,
                    type_alias: type_alias_metas.into_values().collect(),
                    traits: traits_metas.into_values().collect(),
                }
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

    pub fn format_file_metas(metas: Vec<FileMeta>) -> String {
        metas
            .iter()
            .map(|m| m.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl RustProject {
    pub(crate) fn new(cur_dir: &PathBuf) -> Result<RustProject, anyhow::Error> {
        let cargo_config = CargoConfig::default();
        let load_config = LoadCargoConfig {
            load_out_dirs_from_check: false,
            with_proc_macro_server: ProcMacroServerChoice::Sysroot,
            prefill_caches: false,
        };

        let (db, vfs, _proc_macro_client) =
            load_workspace_at(cur_dir, &cargo_config, &load_config, &|msg| {})?;

        let anal_host = AnalysisHost::with_database(db.clone());

        let anal_host = Arc::new(Mutex::new(anal_host));

        let proj = RustProject {
            analysis_host: anal_host,
            vfs,
            files: HashMap::new(),
        };

        Ok(proj)
    }

    pub(crate) async fn new_analysis(&'_ self) -> AnalysisSession<'_> {
        let analysis = self.analysis_host.lock().unwrap().analysis();
        AnalysisSession::new(analysis, self).await
    }

    fn modify_file(&self) {
        let joe = self.analysis_host.lock().unwrap();
    }

    pub async fn get_all_proj_symbols(
        &self,
        symbol_cache: &mut TypedCache<SymbolInfo, SymbolInfo>,
    ) -> anyhow::Result<Vec<SymbolInfo>> {
        let session = self.new_analysis().await;
        symbol_cache.transaction(|db: &mut TypedCacheDb<_, _>| {
            if db.is_empty()? {
                let symboles = session.get_symboles()?;
                let impls = Self::get_all_trait_impls(&self.vfs, &symboles, &session);
                let combined = vec![symboles, impls].concat();
                let (_, errs): (Vec<_>, Vec<_>) = combined
                    .iter()
                    .map(|i| db.put(i, i))
                    .partition(|r| r.is_ok());
                // report on this
                let errs: Vec<_> = errs.into_iter().flat_map(|x1| x1.err()).collect();
                println!("{:?}", errs);
                if !errs.is_empty() {
                    return Err(anyhow!("Error while wrriting to DB! {:?}", errs));
                }
                Ok(combined)
            } else {
                let res: Vec<_> = db.iter()?.collect();
                Ok(res)
            }
        })
    }

    fn get_all_trait_impls(
        vfs: &Vfs,
        symboles: &Vec<SymbolInfo>,
        session: &AnalysisSession<'_>,
    ) -> Vec<SymbolInfo> {
        let traits: Vec<_> = symboles
            .iter()
            .filter(|info| info.kind == SymbolKind::Trait)
            .cloned()
            .collect();

        traits
            .into_iter()
            .flat_map(|info| {
                info.focus_range.clone().map(|t| {
                    session
                        .go_to_impl(
                            vfs.file_id(&VfsPath::new_real_path(info.rpath)).unwrap().0,
                            TextSize::new(t.start),
                        )
                        .ok()
                })
            })
            .flatten()
            .flatten()
            .map(|info| info)
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_analytical_context() {
        // let mut ctx = CurContext::new()
        //     .await
        //     .expect("Failed to create CurContext");
        // let context = ctx
        //     .get_analytical_context()
        //     .await
        //     .expect("Failed to get analytical context");
        // println!("\n=== Analytical Context ===\n{}", context);
    }
}
