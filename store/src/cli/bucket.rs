use crate::{cli::print_json, FileStore, Result};

/// Commands on remote buckets
#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[clap(subcommand)]
    cmd: BucketCmd,
}

#[derive(Debug, clap::Subcommand)]
pub enum BucketCmd {
    List(List),
}

impl Cmd {
    pub async fn run(&self) -> Result {
        self.cmd.run().await
    }
}

impl BucketCmd {
    pub async fn run(&self) -> Result {
        match self {
            Self::List(cmd) => cmd.run().await,
        }
    }
}

#[derive(Debug, clap::Args)]
pub struct List {
    bucket: String,
}

impl List {
    pub async fn run(&self) -> Result {
        let file_store = FileStore::from_env().await?;
        let file_names = file_store.list(&self.bucket).await?;
        print_json(&file_names)
    }
}
