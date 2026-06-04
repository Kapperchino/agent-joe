use async_trait::async_trait;
use ra_ap_ide::LineIndex;
use std::path::PathBuf;

#[async_trait]
pub trait Context: Send + Sync {
    type LineIndexCreator: LineIndexCreator;
    async fn get_ctx(&self) -> String;
    fn get_root(&self) -> PathBuf;
    async fn get_files(&self) -> anyhow::Result<Vec<PathBuf>>;
    async fn line_index_creator(&self) -> anyhow::Result<Box<Self::LineIndexCreator>>;
    fn gen_id(&self) -> u64;
}

pub trait LineIndexCreator: Send + Sync {
    fn create_index(&self, file_path: &PathBuf) -> anyhow::Result<triomphe::Arc<LineIndex>>;
}
