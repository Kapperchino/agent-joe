use std::path::PathBuf;
use tokio::fs;
use tokio::fs::DirEntry;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReadDirStream;

pub struct Utils {}
impl Utils {
    pub async fn get_dir_files(dir: &PathBuf) -> anyhow::Result<Vec<DirEntry>> {
        let read_dir = fs::read_dir(dir).await?;
        let read_dir_stream = ReadDirStream::new(read_dir);
        let res = read_dir_stream
            .fold(vec![], |mut acc, item| {
                match item {
                    Ok(entry) => {
                        acc.push(entry);
                    }
                    Err(_) => {
                        println!("error with getting files")
                    }
                };
                acc
            })
            .await;
        Ok(res)
    }

    pub async fn get_file_content(dir: &PathBuf) -> anyhow::Result<String> {
        let str = fs::read_to_string(dir).await?;
        Ok(str)
    }
}
