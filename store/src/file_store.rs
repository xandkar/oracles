use crate::{env_var, error::DecodeError, Result};
use aws_config::meta::region::RegionProviderChain;
use aws_sdk_s3::{Client, Endpoint, Error as SdkError, Region};
use http::Uri;
use std::str::FromStr;

pub struct FileStore {
    client: Client,
}

impl FileStore {
    pub async fn from_env() -> Result<Self> {
        let endpoint: Option<Endpoint> = env_var("BUCKET_ENDPOINT")?
            .map_or_else(
                || Ok(None),
                |str| Uri::from_str(&str).map(Endpoint::immutable).map(Some),
            )
            .map_err(DecodeError::from)?;
        Self::new(endpoint).await
    }

    pub async fn new(endpoint: Option<Endpoint>) -> Result<Self> {
        let region_provider =
            RegionProviderChain::default_provider().or_else(Region::new("us-west-2"));

        let mut config = aws_config::from_env().region(region_provider);
        if let Some(endpoint) = endpoint {
            config = config.endpoint_resolver(endpoint);
        }
        let config = config.load().await;

        let client = Client::new(&config);
        Ok(Self { client })
    }

    pub async fn list(&self, bucket: &str) -> Result<Vec<String>> {
        let resp = self
            .client
            .list_objects_v2()
            .bucket(bucket)
            .delimiter("/")
            .send()
            .await
            .map_err(SdkError::from)?;

        let result = resp
            .contents()
            .unwrap_or_default()
            .iter()
            .map(|obj| obj.key().unwrap_or_default().to_string())
            .collect();
        Ok(result)
    }
}
