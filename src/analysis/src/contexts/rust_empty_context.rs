use crate::contexts::context::Context;
use crate::contexts::rust_context::{RustContext, RustContextLineIndexCreator};
use async_trait::async_trait;
use itertools::Itertools;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone)]
pub struct RustEmptyContext {
    pub inner: RustContext,
    pub stack_context: bool,
    pub id_gen: Arc<AtomicU64>,
}

impl RustEmptyContext {
    pub fn new(context: RustContext, stack_context: bool, id: u64) -> RustEmptyContext {
        RustEmptyContext {
            inner: context,
            stack_context,
            id_gen: Arc::new(AtomicU64::new(id)),
        }
    }
}

#[async_trait]
impl Context for RustEmptyContext {
    type LineIndexCreator = RustContextLineIndexCreator;

    async fn get_ctx(&self) -> String {
        let files = self
            .inner
            .get_proj_meta()
            .await
            .map(|t| t.files)
            .unwrap_or_default()
            .values()
            .join("\n");
        let stacked_prompt = if self.stack_context {
            self.inner.stacked_context.join("\n")
        } else {
            "".to_owned()
        };
        format!(
            "project_root: {}\n{files}\n{stacked_prompt}",
            self.inner.cur_dir.display()
        )
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
        Ok(self
            .inner
            .get_proj_meta()
            .await?
            .files
            .keys()
            .map(PathBuf::from)
            .collect())
    }

    async fn line_index_creator(&self) -> anyhow::Result<Box<Self::LineIndexCreator>> {
        let proj = self.inner.get_proj_meta().await?;
        Ok(Box::new(RustContextLineIndexCreator {
            proj_meta: Arc::new(proj),
        }))
    }

    fn gen_id(&self) -> u64 {
        self.id_gen.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn get_id(&self) -> u64 {
        self.id_gen.load(Ordering::Acquire)
    }
}
