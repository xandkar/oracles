use crate::{cli::print_json, FileInfo, FileStore, Result};
use std::{path::PathBuf, str::FromStr};

/// Commands on remote buckets
#[derive(Debug, clap::Args)]
pub struct Cmd {
    #[clap(subcommand)]
    cmd: BucketCmd,
}

#[derive(Debug, clap::Subcommand)]
pub enum BucketCmd {
    Ls(List),
    Rm(Remove),
    Put(Put),
}

impl Cmd {
    pub async fn run(&self) -> Result {
        self.cmd.run().await
    }
}

impl BucketCmd {
    pub async fn run(&self) -> Result {
        match self {
            Self::Ls(cmd) => cmd.run().await,
            Self::Rm(cmd) => cmd.run().await,
            Self::Put(cmd) => cmd.run().await,
        }
    }
}

/// List keys in a given bucket
#[derive(Debug, clap::Args)]
pub struct List {
    bucket: String,
}

impl List {
    pub async fn run(&self) -> Result {
        let file_store = FileStore::from_env().await?;
        let file_names = file_store.list(&self.bucket).await?;
        let file_infos = file_names
            .iter()
            .map(|file_name| FileInfo::from_str(file_name))
            .collect::<Vec<Result<FileInfo>>>()
            .into_iter()
            .collect::<Result<Vec<FileInfo>>>()?;
        print_json(&file_infos)
    }
}

/// Put one or more files in a given bucket
#[derive(Debug, clap::Args)]
pub struct Put {
    bucket: String,
    files: Vec<PathBuf>,
}

impl Put {
    pub async fn run(&self) -> Result {
        let file_store = FileStore::from_env().await?;
        for file in self.files.iter() {
            file_store.put(&self.bucket, file).await?;
        }
        Ok(())
    }
}

/// Remove one or more keys from a given bucket
#[derive(Debug, clap::Args)]
pub struct Remove {
    bucket: String,
    keys: Vec<String>,
}

impl Remove {
    pub async fn run(&self) -> Result {
        let file_store = FileStore::from_env().await?;
        for key in self.keys.iter() {
            file_store.remove(&self.bucket, key).await?;
        }
        Ok(())
    }
}
