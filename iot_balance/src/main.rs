use clap::Parser;
use iot_balance::{
    cli::{generate, server},
    Result,
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Debug, clap::Subcommand)]
pub enum Cmd {
    Generate(generate::Cmd),
    Server(server::Cmd),
}

#[derive(Debug, clap::Parser)]
#[clap(version = env!("CARGO_PKG_VERSION"))]
#[clap(about = "Helium Iot Balance Server")]
pub struct Cli {
    #[clap(subcommand)]
    cmd: Cmd,
}

#[tokio::main]
async fn main() -> Result {
    dotenv::dotenv()?;
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "iot_balance=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cli = Cli::parse();

    match cli.cmd {
        Cmd::Generate(cmd) => cmd.run().await?,
        Cmd::Server(cmd) => cmd.run().await?,
    }
    Ok(())
}
