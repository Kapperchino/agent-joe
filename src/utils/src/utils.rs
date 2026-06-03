use crate::files::Files;
use anyhow::Context;
use fnv::FnvBuildHasher;
use std::collections::HashMap;
use std::path::PathBuf;
pub type FnvHashMap<K, V> = HashMap<K, V, FnvBuildHasher>;
pub const CONFIG_DIR_NAME: &str = ".turbo-code";
pub struct Utils {}
impl Utils {
    pub async fn get_file_hashes_for_paths(
        paths: Vec<PathBuf>,
    ) -> anyhow::Result<Vec<(PathBuf, Vec<u8>)>> {
        use futures::StreamExt;

        let contents = Files::get_files_for_paths(paths).await?;
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

    pub fn get_store_dir() -> anyhow::Result<PathBuf> {
        let home_dir = dirs::home_dir().context("Failed to determine the home directory")?;
        Ok(home_dir.join(CONFIG_DIR_NAME))
    }
}
