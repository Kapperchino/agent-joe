use crate::{OpenAIConfig, openai::OpenAIClient};
use std::sync::Arc;
use std::time::Duration;
#[cfg(not(unix))]
use tokio::net::TcpListener as Listener;
#[cfg(unix)]
use tokio::net::UnixListener as Listener;

pub(crate) struct HttpFixture {
    pub listener: Arc<Listener>,
    pub url: String,
    #[cfg(unix)]
    directory: tempfile::TempDir,
}

impl HttpFixture {
    pub async fn new() -> Self {
        #[cfg(unix)]
        {
            let directory = tempfile::tempdir().unwrap();
            let listener = Listener::bind(directory.path().join("http.sock")).unwrap();
            Self {
                listener: Arc::new(listener),
                url: "http://localhost".into(),
                directory,
            }
        }
        #[cfg(not(unix))]
        {
            let listener = Listener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            Self {
                listener: Arc::new(listener),
                url,
            }
        }
    }

    pub fn client(&self, config: OpenAIConfig) -> OpenAIClient {
        let builder = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5));
        #[cfg(unix)]
        let builder = builder.unix_socket(self.directory.path().join("http.sock"));
        OpenAIClient::with_client_builder(config, builder).unwrap()
    }
}
