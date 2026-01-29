use crate::analysis::{Range, SymbolInfo};
use crate::cache::{TypedCache, TypedCacheDb};
use crate::rust_proj::RustProject;
use crate::utils::Utils;
use anyhow::anyhow;
use futures::{StreamExt, future};
use itertools::Itertools;
use ra_ap_ide::LineIndex;
use ra_ap_ide_db::SymbolKind;
use ra_ap_vfs::{FileId, VfsPath};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::env;
use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;
use tokio::fs::DirEntry;

pub struct CurContext {
    pub cur_dir: PathBuf,
    cur_files: Vec<DirEntry>,
    pub rust_proj: RustProject,
    pub file_cache: TypedCache<FileMetaData, FileMetaData>,
    pub file_metas: HashMap<String, FileMeta>,
}

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

impl CurContext {
    pub async fn new() -> Result<CurContext, anyhow::Error> {
        let current_dir = env::current_dir()?;
        let files = Utils::get_dir_files(&current_dir).await?;
        let proj = RustProject::new(&current_dir)?;
        let mut file_cache = TypedCache::new(None).await;
        let hashes: HashMap<_, _> = Self::get_file_hashes(&proj).await?.into_iter().collect();
        let mut file_metas = Self::get_file_metas(&mut file_cache, &proj, &hashes).await?;
        let _ = Self::validate_and_update_cache(hashes, &mut file_metas, &mut file_cache, &proj)
            .await?;

        Ok(CurContext {
            cur_dir: current_dir,
            cur_files: files,
            rust_proj: proj,
            file_cache,
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

    pub async fn validate_and_update_cache(
        hashes: HashMap<PathBuf, Vec<u8>>,
        file_metas: &mut HashMap<String, FileMeta>,
        cache: &mut TypedCache<FileMetaData, FileMetaData>,
        proj: &RustProject,
    ) -> anyhow::Result<()> {
        let files_to_redo: HashSet<_> = hashes
            .iter()
            .flat_map(|(k, v)| {
                let p = k.to_string_lossy();
                match file_metas.get(&p.to_string()) {
                    Some(file) => {
                        if &file.data.hash != v {
                            Some(k.clone())
                        } else {
                            None
                        }
                    }
                    None => Some(k.clone()),
                }
            })
            .collect();

        let hashes: HashMap<_, _> = hashes
            .into_iter()
            .filter(|(k, _)| files_to_redo.contains(k))
            .collect();

        let file_meta_datas = Self::invalidate_and_redo_cache(
            files_to_redo.into_iter().collect_vec(),
            &proj,
            hashes,
            cache,
        )
        .await?;

        file_metas.extend(Self::get_file_metas_inner(&proj, file_meta_datas).await?);
        Ok(())
    }

    pub async fn invalidate_and_redo_cache(
        paths: Vec<PathBuf>,
        proj: &RustProject,
        hashes: HashMap<PathBuf, Vec<u8>>,
        cache: &mut TypedCache<FileMetaData, FileMetaData>,
    ) -> anyhow::Result<HashMap<String, FileMetaData>> {
        let session = proj.new_analysis().await;

        let meta = paths.into_iter().try_fold(HashMap::new(), |mut acc, p| {
            let nodes = if let Some(f_id) = proj.get_file_id(p.clone()) {
                let file_structs = session.get_file_structure(f_id);
                SymbolInfo::from_file_structs(f_id, file_structs, p.clone())
            } else {
                Vec::new()
            };
            let meta_data = CurContext::get_file_meta_datas_cache_miss(nodes, proj, &hashes)?;
            acc.extend(meta_data);
            Ok::<HashMap<String, FileMetaData>, anyhow::Error>(acc)
        })?;

        cache.transaction(|db| {
            meta.iter().try_for_each(|(_, s)| db.put(s, s))?;
            Ok(())
        })?;

        Ok(meta)
    }

    pub async fn get_file_meta_datas(
        vec: Vec<SymbolInfo>,
        rust_proj: &RustProject,
        hashes: &HashMap<PathBuf, Vec<u8>>,
        cache: &mut TypedCache<FileMetaData, FileMetaData>,
    ) -> anyhow::Result<HashMap<String, FileMetaData>> {
        cache.transaction(|db: &mut TypedCacheDb<_, _>| {
            if db.is_empty()? {
                let metas = Self::get_file_meta_datas_cache_miss(vec, rust_proj, hashes)?;
                let (_, errs): (Vec<_>, Vec<_>) = metas
                    .iter()
                    .map(|(_, v)| db.put(v, v))
                    .partition(|r| r.is_ok());
                // report on this
                let errs: Vec<_> = errs.into_iter().flat_map(|x1| x1.err()).collect();
                println!("{:?}", errs);
                if !errs.is_empty() {
                    return Err(anyhow!("Error while wrriting to DB! {:?}", errs));
                }
                Ok(metas)
            } else {
                let res: Vec<_> = db.iter()?.collect();
                Ok(res
                    .into_iter()
                    .map(|data| (data.rpath.clone(), data))
                    .collect())
            }
        })
    }

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

    pub async fn get_file_metas_inner(
        rust_proj: &RustProject,
        file_meta_datas: HashMap<String, FileMetaData>,
    ) -> anyhow::Result<HashMap<String, FileMeta>> {
        let session = rust_proj.new_analysis().await;
        let res: Result<HashMap<_, _>, _> = file_meta_datas
            .into_iter()
            .map(|(k, v)| {
                session
                    .get_line_indecies(FileId::from_raw(v.file_id))
                    .map(|line_index| {
                        let meta = FileMeta {
                            line_index,
                            data: v,
                        };
                        (k, meta)
                    })
            })
            .collect();
        let res = res?;
        Ok(res)
    }

    async fn get_file_metas(
        cache: &mut TypedCache<FileMetaData, FileMetaData>,
        rust_proj: &RustProject,
        hashes: &HashMap<PathBuf, Vec<u8>>,
    ) -> anyhow::Result<HashMap<String, FileMeta>> {
        let symbols = rust_proj.get_all_proj_symbols().await?;
        let datas = Self::get_file_meta_datas(symbols, rust_proj, hashes, cache).await?;
        Self::get_file_metas_inner(rust_proj, datas).await
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

    pub async fn get_file_hashes_for_paths(
        paths: Vec<PathBuf>,
    ) -> anyhow::Result<Vec<(PathBuf, Vec<u8>)>> {
        let contents = Self::get_files_for_paths(paths).await?;
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

    async fn get_files_for_paths(paths: Vec<PathBuf>) -> anyhow::Result<Vec<(PathBuf, String)>> {
        future::join_all(paths.into_iter().map(async |file| {
            Utils::get_file_content(&file)
                .await
                .map(|content| (file, content))
        }))
        .await
        .into_iter()
        .collect()
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
