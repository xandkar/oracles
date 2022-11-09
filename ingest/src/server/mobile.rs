use crate::{
    server::{GrpcResult, VerifyResult},
    Error, EventId, Result, Settings,
};
use chrono::Utc;
use file_store::traits::MsgVerify;
use file_store::{file_sink, file_sink_write, file_upload, FileType};
use futures_util::TryFutureExt;
use helium_crypto::{Network, PublicKey};
use helium_proto::services::poc_mobile::{
    self, CellHeartbeatIngestReportV1, CellHeartbeatReqV1, CellHeartbeatRespV1,
    SpeedtestIngestReportV1, SpeedtestReqV1, SpeedtestRespV1,
};
use std::path::Path;
use tonic::{metadata::MetadataValue, transport, Request, Response, Status};

struct Server {
    heartbeat_req_tx: file_sink::MessageSender,
    speedtest_req_tx: file_sink::MessageSender,
    heartbeat_report_tx: file_sink::MessageSender,
    speedtest_report_tx: file_sink::MessageSender,
    required_network: Network,
}

impl Server {
    fn decode_pub_key(&self, data: &[u8]) -> VerifyResult<PublicKey> {
        PublicKey::try_from(data).map_err(|_| Status::invalid_argument("invalid public key"))
    }

    fn verify_network(&self, public_key: PublicKey) -> VerifyResult<PublicKey> {
        (self.required_network == public_key.network)
            .then_some(public_key)
            .ok_or_else(|| Status::invalid_argument("invalid network"))
    }

    fn verify_signature(&self, pub_key: &PublicKey, event: impl MsgVerify) -> VerifyResult<()> {
        event
            .verify(pub_key)
            .map_err(|_| Status::invalid_argument("invalid signature"))
    }
}

#[tonic::async_trait]
impl poc_mobile::PocMobile for Server {
    async fn submit_speedtest(
        &self,
        request: Request<SpeedtestReqV1>,
    ) -> GrpcResult<SpeedtestRespV1> {
        let event = request.into_inner();

        self.decode_pub_key(event.pub_key.as_ref())
            .and_then(|pub_key| self.verify_network(pub_key))
            .and_then(|pub_key| self.verify_signature(&pub_key, event.clone()))?;

        let event_id = EventId::from(&event);
        let received_timestamp: u64 = Utc::now().timestamp_millis() as u64;

        let report = SpeedtestIngestReportV1 {
            report: Some(event.clone()),
            received_timestamp,
        };

        _ = file_sink_write!("speedtest_req", &self.speedtest_req_tx, event).await;
        _ = file_sink_write!("speedtest_report", &self.speedtest_report_tx, report).await;
        metrics::increment_counter!("ingest_server_speedtest_count");
        Ok(Response::new(event_id.into()))
    }

    async fn submit_cell_heartbeat(
        &self,
        request: Request<CellHeartbeatReqV1>,
    ) -> GrpcResult<CellHeartbeatRespV1> {
        let event = request.into_inner();

        self.decode_pub_key(event.pub_key.as_ref())
            .and_then(|pub_key| self.verify_network(pub_key))
            .and_then(|pub_key| self.verify_signature(&pub_key, event.clone()))?;

        let event_id = EventId::from(&event);
        let received_timestamp: u64 = Utc::now().timestamp_millis() as u64;

        let report = CellHeartbeatIngestReportV1 {
            report: Some(event.clone()),
            received_timestamp,
        };

        _ = file_sink_write!("heartbeat_req", &self.heartbeat_req_tx, event).await;
        _ = file_sink_write!("heartbeat_report", &self.heartbeat_report_tx, report).await;
        metrics::increment_counter!("ingest_server_heartbeat_count");
        Ok(Response::new(event_id.into()))
    }
}

pub async fn run(shutdown: triggered::Listener, settings: &Settings) -> Result {
    let grpc_addr = settings.listen_addr()?;

    // Initialize uploader
    let (file_upload_tx, file_upload_rx) = file_upload::message_channel();
    let file_upload =
        file_upload::FileUpload::from_settings(&settings.output, file_upload_rx).await?;

    let store_base_path = Path::new(&settings.cache);

    let (heartbeat_req_tx, heartbeat_req_rx) = file_sink::message_channel(50);
    let mut heartbeat_req_sink =
        file_sink::FileSinkBuilder::new(FileType::CellHeartbeat, store_base_path, heartbeat_req_rx)
            .deposits(Some(file_upload_tx.clone()))
            .create()
            .await?;

    let (heartbeat_report_tx, heartbeat_report_rx) = file_sink::message_channel(50);
    let mut heartbeat_report_sink = file_sink::FileSinkBuilder::new(
        FileType::CellHeartbeatIngestReport,
        store_base_path,
        heartbeat_report_rx,
    )
    .deposits(Some(file_upload_tx.clone()))
    .create()
    .await?;

    // speedtests
    let (speedtest_req_tx, speedtest_req_rx) = file_sink::message_channel(50);
    let mut speedtest_req_sink =
        file_sink::FileSinkBuilder::new(FileType::CellSpeedtest, store_base_path, speedtest_req_rx)
            .deposits(Some(file_upload_tx.clone()))
            .create()
            .await?;

    let (speedtest_report_tx, speedtest_report_rx) = file_sink::message_channel(50);
    let mut speedtest_report_sink = file_sink::FileSinkBuilder::new(
        FileType::CellSpeedtestIngestReport,
        store_base_path,
        speedtest_report_rx,
    )
    .deposits(Some(file_upload_tx.clone()))
    .create()
    .await?;

    let grpc_server = Server {
        heartbeat_req_tx,
        speedtest_req_tx,
        heartbeat_report_tx,
        speedtest_report_tx,
        required_network: settings.network,
    };

    let api_token = settings
        .token
        .as_ref()
        .map(|token| {
            format!("Bearer {}", token)
                .parse::<MetadataValue<_>>()
                .unwrap()
        })
        .ok_or_else(|| Error::not_found("expected api token in settings"))?;

    tracing::info!(
        "grpc listening on {grpc_addr} and server mode {:?}",
        settings.mode
    );

    //TODO start a service with either the poc mobile or poc lora endpoints only - not both
    //     use _server_mode (set above ) to decide
    let server = transport::Server::builder()
        .layer(poc_metrics::request_layer!("ingest_server_grpc_connection"))
        .add_service(poc_mobile::Server::with_interceptor(
            grpc_server,
            move |req: Request<()>| match req.metadata().get("authorization") {
                Some(t) if api_token == t => Ok(req),
                _ => Err(Status::unauthenticated("No valid auth token")),
            },
        ))
        .serve_with_shutdown(grpc_addr, shutdown.clone())
        .map_err(Error::from);

    tokio::try_join!(
        server,
        heartbeat_req_sink.run(&shutdown).map_err(Error::from),
        speedtest_req_sink.run(&shutdown).map_err(Error::from),
        heartbeat_report_sink.run(&shutdown).map_err(Error::from),
        speedtest_report_sink.run(&shutdown).map_err(Error::from),
        file_upload.run(&shutdown).map_err(Error::from),
    )
    .map(|_| ())
}
