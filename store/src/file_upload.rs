use crate::{env_var, Error, FileStore, Result};
use futures_util::stream::StreamExt;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{fs, sync::mpsc, time};
use tokio_stream::wrappers::ReceiverStream;

pub type MessageSender = mpsc::Sender<PathBuf>;
pub type MessageReceiver = mpsc::Receiver<PathBuf>;

pub fn message_channel(size: usize) -> (MessageSender, MessageReceiver) {
    mpsc::channel(size)
}

pub async fn upload_file(tx: &MessageSender, file: &Path) -> Result {
    tx.send(file.to_path_buf())
        .await
        .map_err(|_| Error::channel())
}

pub struct FileUpload {
    enabled: bool,
    messages: ReceiverStream<PathBuf>,
    bucket: String,
    store: FileStore,
}

impl FileUpload {
    pub async fn from_env<B: ToString>(messages: MessageReceiver, bucket: B) -> Result<Self> {
        let enabled = env_var("FILE_UPLOAD_ENABLED")?.map_or_else(|| true, |str| str == "true");

        Ok(Self {
            enabled,
            messages: ReceiverStream::new(messages),
            store: FileStore::from_env().await?,
            bucket: bucket.to_string(),
        })
    }

    pub async fn run(self, shutdown: triggered::Listener) -> Result {
        tracing::info!("starting file uploader");

        let enabled = self.enabled;
        self.messages
            .map(|msg| {
                (
                    shutdown.clone(),
                    self.store.clone(),
                    self.bucket.clone(),
                    msg,
                )
            })
            .for_each_concurrent(5, |(shutdown, store, bucket, path)| async move {
                let path_str = path.display();
                if !enabled {
                    tracing::info!("file upload disabled for {path_str} to {bucket}");
                    return;
                }
                let mut retry = 0;
                const MAX_RETRIES: u8 = 5;
                const RETRY_WAIT: Duration = Duration::from_secs(10);
                while retry <= MAX_RETRIES {
                    if shutdown.is_triggered() {
                        return;
                    }
                    tracing::debug!("storing {path_str} in {bucket} retry {retry}");
                    match store.put(&bucket, &path).await {
                        Ok(()) => {
                            match fs::remove_file(&path).await {
                                Ok(()) => {
                                    tracing::info!("stored {path_str} in {bucket}");
                                }
                                Err(err) => {
                                    tracing::error!(
                                        "failed to remove uploaded file {path_str}: {err:?}"
                                    );
                                }
                            }
                            return;
                        }
                        Err(err) => {
                            tracing::error!(
                                "failed to store {path_str} in {bucket} retry: {retry}: {err:?}"
                            );
                            retry += 1;
                            time::sleep(RETRY_WAIT).await;
                        }
                    }
                }
            })
            .await;
        Ok(())
    }
}
