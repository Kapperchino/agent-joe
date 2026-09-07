use crate::contexts::context::Context;
use crate::contexts::rust_context::{RustContext, RustContextLineIndexCreator};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone)]
pub struct RustEmptyContext {
    pub inner: RustContext,
    pub id_gen: Arc<AtomicU64>,
}

impl RustEmptyContext {
    pub fn new(mut context: RustContext, id: u64) -> RustEmptyContext {
        context.guidance = context.guidance.fork();
        RustEmptyContext {
            inner: context,
            id_gen: Arc::new(AtomicU64::new(id)),
        }
    }
}

#[async_trait]
impl Context for RustEmptyContext {
    type LineIndexCreator = RustContextLineIndexCreator;

    async fn get_ctx(&self) -> String {
        self.inner.get_ctx().await
    }

    fn effective_instructions(&self) -> anyhow::Result<String> {
        self.inner.effective_instructions()
    }
    fn discover_instructions(&self, paths: &[PathBuf]) -> anyhow::Result<()> {
        self.inner.discover_instructions(paths)
    }
    fn prepare_edit(&self, paths: &[PathBuf]) -> anyhow::Result<()> {
        self.inner.prepare_edit(paths)
    }
    async fn refresh_workspace(&self) -> anyhow::Result<()> {
        self.inner.refresh_workspace().await
    }
    async fn inspect_context(&self) -> anyhow::Result<String> {
        self.inner.inspect_context().await
    }

    fn instructions(&self) -> &str {
        self.inner.instructions()
    }

    fn initial_task(&self) -> Option<&str> {
        self.inner.initial_task()
    }

    fn clear_task_context(&mut self) {
        self.inner.clear_task_context();
    }

    fn get_root(&self) -> PathBuf {
        self.inner.cur_dir.clone()
    }

    async fn get_files(&self) -> anyhow::Result<Vec<PathBuf>> {
        self.inner.get_files().await
    }

    async fn line_index_creator(&self) -> anyhow::Result<Box<Self::LineIndexCreator>> {
        self.inner.line_index_creator().await
    }

    fn gen_id(&self) -> u64 {
        self.id_gen.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn get_id(&self) -> u64 {
        self.id_gen.load(Ordering::Acquire)
    }
}
