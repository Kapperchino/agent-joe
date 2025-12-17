use futures::future;
use std::env;
use std::path::PathBuf;
use tokio::fs;
use tokio::fs::DirEntry;
use tokio_stream::wrappers::ReadDirStream;
use tokio_stream::StreamExt;

pub struct CurContext {
    cur_dir: PathBuf,
    cur_files: Vec<DirEntry>,
}
impl CurContext {
    pub async fn get_cur_context() -> Result<CurContext, anyhow::Error> {
        let current_dir = env::current_dir()?;
        let read_dir = fs::read_dir(current_dir.clone()).await?;
        let read_dir_stream = ReadDirStream::new(read_dir);
        let files = read_dir_stream
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
        Ok(CurContext {
            cur_dir: current_dir,
            cur_files: files,
        })
    }

    pub async fn to_string(&self) -> String {
        let dir = self.cur_dir.to_str().unwrap_or("");
        let files: Vec<_> = self
            .cur_files
            .iter()
            .map(async |file| {
                let path = file.path();
                let file_type = match file.file_type().await {
                    Ok(f_type) => {
                        if f_type.is_dir() {
                            Some("type: dir")
                        } else if f_type.is_file() {
                            Some("type: file")
                        } else if f_type.is_symlink() {
                            Some("type: symlink")
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                };
                if let Some(f_type) = file_type
                    && let Some(p_str) = path.to_str()
                {
                    Some(format!("path: {p_str}, {f_type}\n").to_string())
                } else {
                    None
                }
            })
            .collect();
        let res: String = future::join_all(files)
            .await
            .into_iter()
            .flatten()
            .fold(String::new(), |acc, s| format!("{acc}{s}").to_string());
        format!("Current Context: \ncurrent directory: {dir}\ncurrent files:\n{res}")
    }
}
