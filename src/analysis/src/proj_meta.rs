use crate::analysis::{AnalysisSession, Range};
use crate::rust_proj::RustProject;
use crate::symbol_info::SymbolInfo;
use crate::utils::RPath;
use futures::{StreamExt, future};
use itertools::Itertools;
use ra_ap_ide::LineIndex;
use ra_ap_ide_db::SymbolKind;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use utils::files::Files;
use utils::utils::FnvHashMap;

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
    pub files: FnvHashMap<String, FileMeta>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct EnumMeta {
    pub rpath: RPath,
    pub full_range: Range,
    pub name: String,
    pub docs: Option<String>,
    pub variants: Vec<EVariantMeta>,
    pub functions: Vec<FunctionMeta>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct EVariantMeta {
    pub rpath: RPath,
    pub full_range: Range,
    pub name: String,
    pub docs: Option<String>,
    pub functions: Vec<FunctionMeta>,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct StructMeta {
    pub rpath: RPath,
    pub full_range: Range,
    pub name: String,
    pub docs: Option<String>,
    pub fields: Vec<FieldMeta>,
    pub functions: Vec<FunctionMeta>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FunctionMeta {
    pub rpath: RPath,
    pub full_range: Range,
    pub name: String,
    pub docs: Option<String>,
    pub discription: Option<String>,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct FieldMeta {
    pub rpath: RPath,
    pub full_range: Range,
    pub name: String,
    pub docs: Option<String>,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct TypeAliasMeta {
    pub rpath: RPath,
    pub full_range: Range,
    pub name: String,
    pub docs: Option<String>,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct TraitMeta {
    pub rpath: RPath,
    pub full_range: Range,
    pub name: String,
    pub docs: Option<String>,
    pub functions: Vec<FunctionMeta>,
}
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImplMeta {
    pub rpath: RPath,
    pub full_range: Range,
    pub name: String,
    pub docs: Option<String>,
    pub functions: Vec<FunctionMeta>,
}

struct SymbolDisplayItem {
    start: u32,
    end: u32,
    kind_order: u8,
    name: String,
    header: String,
    details: Vec<String>,
}

impl From<SymbolInfo> for EnumMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
            docs: info.docs,
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
            docs: info.docs,
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
            docs: info.docs,
            fields: vec![],
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
            docs: info.docs,
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
            docs: info.docs,
            discription: info.description,
        }
    }
}

impl From<SymbolInfo> for FieldMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
            docs: info.docs,
        }
    }
}

impl From<SymbolInfo> for TypeAliasMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
            docs: info.docs,
        }
    }
}

impl From<SymbolInfo> for TraitMeta {
    fn from(info: SymbolInfo) -> Self {
        Self {
            rpath: info.rpath,
            full_range: info.full_range,
            name: info.name,
            docs: info.docs,
            functions: vec![],
        }
    }
}

impl ProjMeta {
    // not proud of this
    pub async fn get_proj_meta_from_symbols(
        vec: Vec<SymbolInfo>,
        rust_proj: &RustProject,
    ) -> anyhow::Result<ProjMeta> {
        let joe_2: FnvHashMap<_, _> = vec.iter().map(|s| (s.name.clone(), s.clone())).collect();
        let joe: HashMap<_, _> = vec.into_iter().into_group_map_by(|x2| x2.kind.clone());
        let structs = Self::get_symbol_map(&joe, &SymbolKind::Struct);
        let enums = Self::get_symbol_map(&joe, &SymbolKind::Enum);
        let variants = Self::get_symbol_map(&joe, &SymbolKind::Variant);
        let traits = Self::get_symbol_map(&joe, &SymbolKind::Trait);
        let type_alias = Self::get_symbol_map(&joe, &SymbolKind::TypeAlias);
        let impls = Self::get_symbol_map(&joe, &SymbolKind::Impl);
        let fields = joe.get(&SymbolKind::Field).cloned().unwrap_or_default();

        let mut traits_metas: FnvHashMap<String, TraitMeta> = Self::into_meta(&traits);
        let type_alias_metas: FnvHashMap<String, TypeAliasMeta> = Self::into_meta(&type_alias);

        let (mut stand_alone, mut functions): (Vec<_>, _) = joe
            .get(&SymbolKind::Function)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .partition(|info| info.container_name.is_none());

        let (mut m_stand_alone, mut m_functions): (Vec<_>, _) = joe
            .get(&SymbolKind::Method)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .partition(|info| info.container_name.is_none());

        stand_alone.append(&mut m_stand_alone);
        functions.append(&mut m_functions);

        let stand_alone_func: Vec<_> = stand_alone.into_iter().map(|i| i.into()).collect();

        let mut struct_metas: FnvHashMap<String, StructMeta> = Self::into_meta(&structs);

        let mut enum_metas: FnvHashMap<String, EnumMeta> = Self::into_meta(&enums);

        let mut evariants_metas: FnvHashMap<String, EVariantMeta> = Self::into_meta(&variants);

        let mut impls_metas: FnvHashMap<String, ImplMeta> = Self::into_meta(&impls);

        let e_variants: FnvHashMap<String, Vec<EVariantMeta>> = variants
            .into_values()
            .into_group_map_by(|v| v.container_name.clone().unwrap())
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().map(|i| i.into()).collect()))
            .collect::<FnvHashMap<String, Vec<EVariantMeta>>>();

        let inner_funcs: FnvHashMap<String, Vec<FunctionMeta>> = functions
            .iter()
            .into_group_map_by(|v| v.container_name.clone().unwrap())
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().map(|i| i.clone().into()).collect()))
            .collect::<FnvHashMap<String, Vec<FunctionMeta>>>();

        let inner_fields: FnvHashMap<String, Vec<FieldMeta>> = fields
            .into_iter()
            .filter_map(|field| {
                field
                    .container_name
                    .clone()
                    .map(|container_name| (container_name, field.into()))
            })
            .into_group_map()
            .into_iter()
            .collect();

        inner_funcs.into_iter().for_each(|(k, mut v)| {
            if let Some(s_meta) = struct_metas.get_mut(&k) {
                s_meta.functions.append(&mut v)
            } else if let Some(t_meta) = traits_metas.get_mut(&k) {
                t_meta.functions.append(&mut v)
            } else if let Some(e_meta) = enum_metas.get_mut(&k) {
                e_meta.functions.append(&mut v);
            } else if let Some(ev_meta) = evariants_metas.get_mut(&k) {
                ev_meta.functions.append(&mut v)
            } else if let Some(impl_meta) = impls_metas.get_mut(&k) {
                impl_meta.functions.append(&mut v)
            } else {
                match joe_2.get(&k) {
                    None => {
                        let struct_split: Vec<_> = k.split("<").collect();
                        struct_split.first().and_then(|t| joe_2.get(*t)).map(|t1| {
                            let split_k = &t1.name;
                            if let Some(s_meta) = struct_metas.get_mut(split_k) {
                                s_meta.functions.append(&mut v)
                            } else if let Some(t_meta) = traits_metas.get_mut(split_k) {
                                t_meta.functions.append(&mut v)
                            } else if let Some(e_meta) = enum_metas.get_mut(split_k) {
                                e_meta.functions.append(&mut v)
                            } else if let Some(ev_meta) = evariants_metas.get_mut(split_k) {
                                ev_meta.functions.append(&mut v)
                            } else if let Some(impl_meta) = impls_metas.get_mut(split_k) {
                                impl_meta.functions.append(&mut v)
                            } else {
                                //println!("k:{:?}{:?}", k, v);
                            }
                        });
                    }
                    Some(_val) => {
                        // println!("k:{:?}{:?}", k, val);
                    }
                };
            }
        });

        inner_fields.into_iter().for_each(|(k, mut v)| {
            if let Some(s_meta) = struct_metas.get_mut(&k) {
                s_meta.fields.append(&mut v);
                return;
            }

            let struct_split: Vec<_> = k.split("<").collect();
            struct_split.first().and_then(|t| joe_2.get(*t)).map(|t1| {
                let split_k = &t1.name;
                if let Some(s_meta) = struct_metas.get_mut(split_k) {
                    s_meta.fields.append(&mut v);
                }
            });
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

    fn function_signature(func: &FunctionMeta) -> String {
        match &func.discription {
            None => format!("fn {}(...)", func.name),
            Some(desc) => {
                let desc = desc.split_whitespace().join(" ");
                if desc.is_empty() {
                    format!("fn {}(...)", func.name)
                } else if desc.starts_with("fn") {
                    let rest = desc.strip_prefix("fn").unwrap_or_default().trim_start();
                    if rest.starts_with(&func.name) {
                        desc
                    } else if rest.starts_with('(') || rest.starts_with('<') {
                        format!("fn {}{}", func.name, rest)
                    } else if rest.is_empty() {
                        format!("fn {}(...)", func.name)
                    } else {
                        format!("fn {} {}", func.name, rest)
                    }
                } else if desc.contains(&func.name) {
                    desc
                } else {
                    format!("fn {} {}", func.name, desc)
                }
            }
        }
    }

    fn compact_function(func: &FunctionMeta) -> String {
        format!(
            "{} @ {}:{}-{}",
            Self::function_signature(func),
            func.rpath,
            func.full_range.start,
            func.full_range.end
        )
    }

    fn range_suffix(range: &Range) -> String {
        format!("[{}-{}]", range.start, range.end)
    }

    fn docs_detail(docs: &Option<String>) -> Option<String> {
        docs.as_ref()
            .map(|docs| docs.split_whitespace().join(" "))
            .filter(|docs| !docs.is_empty())
            .map(|docs| format!("docs: {docs}"))
    }

    fn unique_names(names: impl IntoIterator<Item = String>) -> String {
        let mut unique = Vec::new();
        for name in names {
            if !unique.contains(&name) {
                unique.push(name);
            }
        }
        unique.join(", ")
    }

    fn add_symbol_entry(
        entries: &mut BTreeMap<String, Vec<SymbolDisplayItem>>,
        rpath: &RPath,
        range: &Range,
        kind_order: u8,
        name: &str,
        header: String,
        details: Vec<String>,
    ) {
        entries
            .entry(rpath.to_string())
            .or_default()
            .push(SymbolDisplayItem {
                start: range.start,
                end: range.end,
                kind_order,
                name: name.to_string(),
                header,
                details,
            });
    }

    fn add_function_entry(
        entries: &mut BTreeMap<String, Vec<SymbolDisplayItem>>,
        func: &FunctionMeta,
    ) {
        let mut details = Vec::new();
        if let Some(docs) = Self::docs_detail(&func.docs) {
            details.push(docs);
        }

        Self::add_symbol_entry(
            entries,
            &func.rpath,
            &func.full_range,
            0,
            &func.name,
            format!(
                "{} {}",
                Self::function_signature(func),
                Self::range_suffix(&func.full_range)
            ),
            details,
        );
    }

    fn symbol_display_entries(&self) -> BTreeMap<String, Vec<SymbolDisplayItem>> {
        let mut entries: BTreeMap<String, Vec<SymbolDisplayItem>> = BTreeMap::new();

        for func in &self.functions {
            Self::add_function_entry(&mut entries, func);
        }

        for s in &self.structs {
            let mut details = Vec::new();
            if let Some(docs) = Self::docs_detail(&s.docs) {
                details.push(docs);
            }

            let mut fields = s.fields.iter().collect::<Vec<_>>();
            fields.sort_by(|a, b| {
                a.full_range
                    .start
                    .cmp(&b.full_range.start)
                    .then(a.full_range.end.cmp(&b.full_range.end))
                    .then(a.name.cmp(&b.name))
            });
            let fields = Self::unique_names(fields.into_iter().map(|field| field.name.clone()));
            if !fields.is_empty() {
                details.push(format!("fields: {fields}"));
            }

            Self::add_symbol_entry(
                &mut entries,
                &s.rpath,
                &s.full_range,
                1,
                &s.name,
                format!("struct {} {}", s.name, Self::range_suffix(&s.full_range)),
                details,
            );

            for func in &s.functions {
                Self::add_function_entry(&mut entries, func);
            }
        }

        for e in &self.enums {
            let mut details = Vec::new();
            if let Some(docs) = Self::docs_detail(&e.docs) {
                details.push(docs);
            }

            let mut variants = e.variants.iter().collect::<Vec<_>>();
            variants.sort_by(|a, b| {
                a.full_range
                    .start
                    .cmp(&b.full_range.start)
                    .then(a.full_range.end.cmp(&b.full_range.end))
                    .then(a.name.cmp(&b.name))
            });
            let variants =
                Self::unique_names(variants.into_iter().map(|variant| variant.name.clone()));
            if !variants.is_empty() {
                details.push(format!("variants: {variants}"));
            }

            Self::add_symbol_entry(
                &mut entries,
                &e.rpath,
                &e.full_range,
                2,
                &e.name,
                format!("enum {} {}", e.name, Self::range_suffix(&e.full_range)),
                details,
            );

            for func in &e.functions {
                Self::add_function_entry(&mut entries, func);
            }

            for variant in &e.variants {
                for func in &variant.functions {
                    Self::add_function_entry(&mut entries, func);
                }
            }
        }

        for t in &self.traits {
            let mut details = Vec::new();
            if let Some(docs) = Self::docs_detail(&t.docs) {
                details.push(docs);
            }

            Self::add_symbol_entry(
                &mut entries,
                &t.rpath,
                &t.full_range,
                3,
                &t.name,
                format!("trait {} {}", t.name, Self::range_suffix(&t.full_range)),
                details,
            );

            for func in &t.functions {
                Self::add_function_entry(&mut entries, func);
            }
        }

        for ta in &self.type_alias {
            let mut details = Vec::new();
            if let Some(docs) = Self::docs_detail(&ta.docs) {
                details.push(docs);
            }

            Self::add_symbol_entry(
                &mut entries,
                &ta.rpath,
                &ta.full_range,
                4,
                &ta.name,
                format!("type {} {}", ta.name, Self::range_suffix(&ta.full_range)),
                details,
            );
        }

        for items in entries.values_mut() {
            items.sort_by(|a, b| {
                a.start
                    .cmp(&b.start)
                    .then(a.end.cmp(&b.end))
                    .then(a.kind_order.cmp(&b.kind_order))
                    .then(a.name.cmp(&b.name))
            });
        }

        entries
    }

    fn into_meta<T: From<SymbolInfo>>(
        map: &FnvHashMap<String, SymbolInfo>,
    ) -> FnvHashMap<String, T> {
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
    ) -> FnvHashMap<String, SymbolInfo> {
        symbols
            .get(kind)
            .map(|t| {
                let res: FnvHashMap<_, _> = t
                    .into_iter()
                    .cloned()
                    .map(|info| (info.name.clone(), info.clone()))
                    .collect();
                res
            })
            .unwrap_or_default()
    }

    pub async fn get_file_metas(proj: &RustProject) -> anyhow::Result<Vec<FileMeta>> {
        let file_inputs = {
            let analysis = proj.new_analysis().await;
            analysis
                .get_work_files()
                .into_iter()
                .flat_map(|file| {
                    file.path
                        .into_abs_path()
                        .map(|path| (PathBuf::from(path), file.id))
                })
                .map(|(path, file_id)| {
                    let line_index = analysis.get_line_indecies(file_id)?;
                    Ok((path, line_index, file_id.index()))
                })
                .collect::<anyhow::Result<Vec<_>>>()?
        };

        let mut hashes: FnvHashMap<_, _> = Self::get_file_hashes_for_paths(
            file_inputs
                .iter()
                .map(|(path, _, _)| path.clone())
                .collect(),
        )
        .await?
        .into_iter()
        .collect();

        file_inputs
            .into_iter()
            .map(|(pb, line_index, file_id)| {
                let path = pb.to_string_lossy().to_string();
                let hash = hashes
                    .remove(&pb)
                    .ok_or_else(|| anyhow::anyhow!("missing file hash for {path}"))?;
                let rpath = path
                    .strip_prefix(&format!("{}/", proj.root))
                    .unwrap_or(&path)
                    .to_string();

                Ok(FileMeta {
                    line_index,
                    rpath,
                    file_id,
                    hash,
                })
            })
            .collect()
    }

    pub async fn get_file_hashes(proj: &RustProject) -> anyhow::Result<Vec<(PathBuf, Vec<u8>)>> {
        let paths = {
            let analysis = proj.new_analysis().await;
            Self::work_file_paths(&analysis)
        };
        Self::get_file_hashes_for_paths(paths).await
    }

    async fn get_file_hashes_for_paths(
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

    fn work_file_paths(analysis: &AnalysisSession<'_>) -> Vec<PathBuf> {
        analysis
            .get_work_files()
            .into_iter()
            .flat_map(|file| file.path.into_abs_path().map(PathBuf::from))
            .collect()
    }

    async fn get_files_for_paths(paths: Vec<PathBuf>) -> anyhow::Result<Vec<(PathBuf, String)>> {
        future::join_all(paths.into_iter().map(|path| async move {
            Files::read_file(&path.clone())
                .await
                .map(|content| (path, content))
        }))
        .await
        .into_iter()
        .collect()
    }
}

impl Display for FileMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "- {}", self.rpath)
    }
}

impl Display for FunctionMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", ProjMeta::compact_function(self))
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
            "- enum {} @ {}:{}-{}",
            self.name, self.rpath, self.full_range.start, self.full_range.end
        )?;
        if !self.variants.is_empty() {
            let variants: Vec<_> = self.variants.iter().map(|v| v.name.as_str()).collect();
            write!(f, "\n    variants: [{}]", variants.join(", "))?;
        }
        if !self.functions.is_empty() {
            write!(f, "\n    methods:")?;
            for func in &self.functions {
                write!(f, "\n      - {func}",)?;
            }
        }
        Ok(())
    }
}

impl Display for StructMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "- struct {} @ {}:{}-{}",
            self.name, self.rpath, self.full_range.start, self.full_range.end
        )?;
        if !self.functions.is_empty() {
            write!(f, "\n    methods:")?;
            for func in &self.functions {
                write!(f, "\n      - {func}",)?;
            }
        }
        Ok(())
    }
}

impl Display for TypeAliasMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "- type {} @ {}:{}-{}",
            self.name, self.rpath, self.full_range.start, self.full_range.end
        )
    }
}

impl Display for TraitMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "- trait {} @ {}:{}-{}",
            self.name, self.rpath, self.full_range.start, self.full_range.end
        )?;
        if !self.functions.is_empty() {
            write!(f, "\n    methods:")?;
            for func in &self.functions {
                write!(f, "\n      - {func}",)?;
            }
        }
        Ok(())
    }
}

impl Display for ProjMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "# Repo Symbols")?;

        for (rpath, items) in self.symbol_display_entries() {
            writeln!(f)?;
            writeln!(f, "## {rpath}")?;

            for item in items {
                writeln!(f)?;
                writeln!(f, "- {}", item.header)?;

                for detail in item.details {
                    writeln!(f, "  {detail}")?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rpath(path: &str) -> RPath {
        RPath {
            inner: path.to_string(),
        }
    }

    fn range(start: u32, end: u32) -> Range {
        Range { start, end }
    }

    #[test]
    fn display_groups_symbols_by_file() {
        let proj_meta = ProjMeta {
            enums: vec![],
            structs: vec![StructMeta {
                rpath: rpath("src/app/src/utils/draw_line.rs"),
                full_range: range(332, 360),
                name: "LineStyle".to_string(),
                docs: None,
                fields: vec![
                    FieldMeta {
                        rpath: rpath("src/app/src/utils/draw_line.rs"),
                        full_range: range(333, 333),
                        name: "thickness".to_string(),
                        docs: None,
                    },
                    FieldMeta {
                        rpath: rpath("src/app/src/utils/draw_line.rs"),
                        full_range: range(334, 334),
                        name: "color".to_string(),
                        docs: None,
                    },
                    FieldMeta {
                        rpath: rpath("src/app/src/utils/draw_line.rs"),
                        full_range: range(335, 335),
                        name: "pattern".to_string(),
                        docs: None,
                    },
                ],
                functions: vec![],
            }],
            functions: vec![
                FunctionMeta {
                    rpath: rpath("src/app/src/utils/draw_line.rs"),
                    full_range: range(1, 239),
                    name: "draw_line".to_string(),
                    docs: Some("Draws a line segment into the terminal buffer.".to_string()),
                    discription: Some("fn draw_line(...) -> Result<()>".to_string()),
                },
                FunctionMeta {
                    rpath: rpath("src/app/src/utils/draw_line.rs"),
                    full_range: range(240, 331),
                    name: "clip_line".to_string(),
                    docs: None,
                    discription: Some("fn clip_line(...) -> Option<Line>".to_string()),
                },
                FunctionMeta {
                    rpath: rpath("src/app/src/utils/draw_table.rs"),
                    full_range: range(1, 220),
                    name: "draw_table".to_string(),
                    docs: None,
                    discription: Some("fn draw_table(...) -> Result<()>".to_string()),
                },
            ],
            type_alias: vec![],
            traits: vec![],
            files: FnvHashMap::default(),
        };

        assert_eq!(
            proj_meta.to_string(),
            concat!(
                "# Repo Symbols\n",
                "\n",
                "## src/app/src/utils/draw_line.rs\n",
                "\n",
                "- fn draw_line(...) -> Result<()> [1-239]\n",
                "  docs: Draws a line segment into the terminal buffer.\n",
                "\n",
                "- fn clip_line(...) -> Option<Line> [240-331]\n",
                "\n",
                "- struct LineStyle [332-360]\n",
                "  fields: thickness, color, pattern\n",
                "\n",
                "## src/app/src/utils/draw_table.rs\n",
                "\n",
                "- fn draw_table(...) -> Result<()> [1-220]\n",
            )
        );
    }
}
