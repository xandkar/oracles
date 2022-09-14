extern crate iot_packet_verifier;

use iot_packet_verifier::run;
use poc_store::Result;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result {
    dotenv::dotenv()?;
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            dotenv::var("RUST_LOG").unwrap_or_else(|_| "iot_packet_verifier=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    run().await
}
