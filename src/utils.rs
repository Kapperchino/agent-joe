use std::path::PathBuf;
use tokio::fs;
use tokio::fs::DirEntry;
use tokio_stream::wrappers::ReadDirStream;
use tokio_stream::StreamExt;

pub struct Utils {}
impl Utils {
    pub async fn get_dir_files(dir: &PathBuf) -> Result<Vec<DirEntry>, anyhow::Error> {
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
}
