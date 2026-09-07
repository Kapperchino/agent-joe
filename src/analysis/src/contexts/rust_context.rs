use crate::{
    contexts::context::{Context, LineIndexCreator},
    instructions::Instructions,
    proj_meta::ProjMeta,
    rust_proj::RustProject,
};
use async_trait::async_trait;
use ra_ap_ide::LineIndex;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use utils::{inventory::Inventory, workspace::WorkspacePolicy};

pub struct RustContextLineIndexCreator {
    workspace: Arc<WorkspacePolicy>,
}

impl LineIndexCreator for RustContextLineIndexCreator {
    fn create_index(&self, file_path: &PathBuf) -> anyhow::Result<triomphe::Arc<LineIndex>> {
        Ok(triomphe::Arc::new(LineIndex::new(
            &self.workspace.read(file_path)?,
        )))
    }
}

#[async_trait]
impl Context for RustContext {
    type LineIndexCreator = RustContextLineIndexCreator;

    async fn get_ctx(&self) -> String {
        format!(
            "project_root: {}\nUse find_files, list_directory, grep, and read_file to inspect current repository files. Line numbers start at one; range ends are exclusive. Discovery honors .gitignore and .ignore; explicit reads can access allowed ignored files. Rust analysis is optional; there is no complete symbol map in this context. Use inspect_context to inspect active scoped instructions and inventory limits.",
            self.cur_dir.display()
        )
    }

    fn instructions(&self) -> &str {
        &self.initial_prompt
    }

    fn effective_instructions(&self) -> anyhow::Result<String> {
        self.guidance.operating(&self.initial_prompt)
    }

    fn discover_instructions(&self, paths: &[PathBuf]) -> anyhow::Result<()> {
        self.guidance.discover(paths)
    }

    fn prepare_edit(&self, paths: &[PathBuf]) -> anyhow::Result<()> {
        self.guidance.prepare_edit(paths)
    }

    async fn refresh_workspace(&self) -> anyhow::Result<()> {
        let project = self.rust_proj.clone();
        utils::execution::ExecutionScope::current()
            .tasks
            .spawn_blocking(move || project.refresh_from_disk())
            .await??;
        Ok(())
    }

    async fn inspect_context(&self) -> anyhow::Result<String> {
        let workspace = self.rust_proj.workspace();
        let inventory = utils::execution::ExecutionScope::current()
            .tasks
            .spawn_blocking(move || Inventory::scan(&workspace))
            .await??;
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "project_root": self.cur_dir,
            "instructions": self.guidance.sources()?,
            "built_in_instructions": self.initial_prompt,
            "precedence": "Built-in policy and explicit user requests; then deeper scoped AGENTS.md over repository AGENTS.md over global AGENTS.md",
            "instruction_truncated": false,
            "inventory": { "total_files": inventory.files.len(), "skipped_entries": inventory.skipped, "sample": inventory.files.iter().take(200).collect::<Vec<_>>(), "truncated": inventory.files.len() > 200 },
            "workspace_context": self.get_ctx().await,
            "symbol_map_injected": false,
            "worker_summaries_injected": false
        }))?)
    }

    fn initial_task(&self) -> Option<&str> {
        self.task_prompt.as_deref()
    }

    fn clear_task_context(&mut self) {
        self.task_prompt = None;
        self.guidance = self.guidance.reset();
    }

    fn get_root(&self) -> PathBuf {
        self.cur_dir.clone()
    }

    async fn get_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        let workspace = self.rust_proj.workspace();
        utils::execution::ExecutionScope::current()
            .tasks
            .spawn_blocking(move || Inventory::scan(&workspace).map(|inventory| inventory.files))
            .await?
    }

    async fn line_index_creator(&self) -> anyhow::Result<Box<Self::LineIndexCreator>> {
        Ok(Box::new(RustContextLineIndexCreator {
            workspace: self.rust_proj.workspace(),
        }))
    }

    fn gen_id(&self) -> u64 {
        self.id_gen.fetch_add(1, Ordering::AcqRel) + 1
    }
    fn get_id(&self) -> u64 {
        self.id_gen.load(Ordering::Acquire)
    }
}

#[derive(Clone)]
pub struct RustContext {
    pub cur_dir: PathBuf,
    pub rust_proj: RustProject,
    pub initial_prompt: String,
    pub task_prompt: Option<String>,
    pub id_gen: Arc<AtomicU64>,
    pub guidance: Instructions,
}

impl RustContext {
    pub async fn new(
        initial_prompt: String,
        id: u64,
        current_dir: PathBuf,
    ) -> anyhow::Result<Self> {
        let proj = RustProject::new(&current_dir)?;
        let guidance = Instructions::new(proj.workspace());
        guidance.sources()?;
        Ok(Self {
            cur_dir: proj.workspace().root().to_path_buf(),
            initial_prompt,
            task_prompt: None,
            guidance,
            rust_proj: proj,
            id_gen: Arc::new(AtomicU64::new(id)),
        })
    }

    pub async fn get_analytical_context(&self) -> anyhow::Result<String> {
        Ok(self.get_proj_meta().await?.to_string())
    }

    pub async fn get_proj_meta(&self) -> anyhow::Result<ProjMeta> {
        self.refresh_workspace().await?;
        let symbols = self.rust_proj.get_all_proj_symbols().await?;
        ProjMeta::get_proj_meta_from_symbols(symbols, &self.rust_proj).await
    }
}
