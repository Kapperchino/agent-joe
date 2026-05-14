use crate::contexts::context::Context;
use crate::contexts::rust_context::{RustContext, RustContextLineIndexCreator};
use async_trait::async_trait;
use itertools::Itertools;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct RustEmptyContext {
    pub inner: RustContext,
}

impl RustEmptyContext {
    pub fn new(context: RustContext) -> RustEmptyContext {
        RustEmptyContext { inner: context }
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
        let init_prompt = self.inner.initial_prompt.clone();
        let stacked_prompt = self.inner.stacked_context.join("\n");
        format!("{init_prompt}\n{files}\n{stacked_prompt}")
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
}
