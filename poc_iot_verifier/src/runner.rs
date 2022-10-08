use crate::poc::VerifyWitnessesResult;
use crate::{
    entropy::Entropy,
    last_beacon::LastBeacon,
    mk_db_pool,
    poc::Poc,
    poc::VerificationStatus,
    poc_report::{LoraStatus, Report},
    Result,
};
use file_store::{
    file_sink, file_sink::MessageSender, file_upload, lora_beacon_report::LoraBeaconIngestReport,
    lora_beacon_report::LoraBeaconReport, lora_invalid_poc::LoraInvalidBeaconReport,
    lora_invalid_poc::LoraInvalidWitnessReport, lora_valid_poc::LoraValidBeaconReport,
    lora_valid_poc::LoraValidPoc, lora_witness_report::LoraWitnessIngestReport, FileType,
};
use helium_proto::services::poc_lora::{
    InvalidParticipantSide, InvalidReason, LoraBeaconIngestReportV1, LoraInvalidBeaconReportV1,
    LoraInvalidWitnessReportV1, LoraValidPocV1, LoraWitnessIngestReportV1,
};
use std::path::Path;

use chrono::Utc;
use helium_proto::Message;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tokio::time;

const DB_POLL_TIME: time::Duration = time::Duration::from_secs(90);
const LOADER_WORKERS: usize = 10;
const LOADER_DB_POOL_SIZE: usize = 2 * LOADER_WORKERS;
const BEACON_INTERVAL: i64 = 60; //minutes

pub struct Runner {
    pool: PgPool,
    store_path: String,
}

impl Runner {
    pub async fn from_env() -> Result<Self> {
        let pool = mk_db_pool(LOADER_DB_POOL_SIZE as u32).await?;
        let store_path =
            std::env::var("VERIFIER_STORE").unwrap_or_else(|_| String::from("/var/data/verifier"));
        Ok(Self { pool, store_path })
    }

    pub async fn run(&self, shutdown: &triggered::Listener) -> Result {
        tracing::info!("starting runner");

        let mut db_timer = time::interval(DB_POLL_TIME);
        db_timer.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

        let store_base_path = Path::new(&self.store_path);
        let (lora_invalid_beacon_tx, lora_invalid_beacon_rx) = file_sink::message_channel(50);
        let (lora_invalid_witness_tx, lora_invalid_witness_rx) = file_sink::message_channel(50);
        let (lora_valid_poc_tx, lora_valid_poc_rx) = file_sink::message_channel(50);

        let (file_upload_tx, file_upload_rx) = file_upload::message_channel();
        let file_upload =
            file_upload::FileUpload::from_env_with_prefix("VERIFIER", file_upload_rx).await?;

        let mut lora_invalid_beacon_sink = file_sink::FileSinkBuilder::new(
            FileType::LoraInvalidBeaconReport,
            store_base_path,
            lora_invalid_beacon_rx,
        )
        .deposits(Some(file_upload_tx.clone()))
        .create()
        .await?;

        let mut lora_invalid_witness_sink = file_sink::FileSinkBuilder::new(
            FileType::LoraInvalidWitnessReport,
            store_base_path,
            lora_invalid_witness_rx,
        )
        .deposits(Some(file_upload_tx.clone()))
        .create()
        .await?;

        let mut lora_valid_poc_sink = file_sink::FileSinkBuilder::new(
            FileType::LoraValidPoc,
            store_base_path,
            lora_valid_poc_rx,
        )
        .deposits(Some(file_upload_tx.clone()))
        .create()
        .await?;

        // spawn off the file sinks
        // TODO: how to avoid all da cloning?
        let shutdown2 = shutdown.clone();
        let shutdown3 = shutdown.clone();
        let shutdown4 = shutdown.clone();
        let shutdown5 = shutdown.clone();
        tokio::spawn(async move { lora_invalid_beacon_sink.run(&shutdown2).await });
        tokio::spawn(async move { lora_invalid_witness_sink.run(&shutdown3).await });
        tokio::spawn(async move { lora_valid_poc_sink.run(&shutdown4).await });
        tokio::spawn(async move { file_upload.run(&shutdown5).await });

        loop {
            if shutdown.is_triggered() {
                break;
            }
            tokio::select! {
                _ = shutdown.clone() => break,
                _ = db_timer.tick() =>
                    match self.handle_db_tick(  shutdown.clone(),
                                                lora_invalid_beacon_tx.clone(),
                                                lora_invalid_witness_tx.clone(),
                                                lora_valid_poc_tx.clone()).await {
                    Ok(()) => (),
                    Err(err) => {
                        tracing::error!("fatal db runner error: {err:?}");
                        return Err(err)
                    }
                }
            }
        }
        tracing::info!("stopping runner");
        Ok(())
    }

    async fn handle_db_tick(
        &self,
        _shutdown: triggered::Listener,
        _lora_invalid_beacon_tx: MessageSender,
        lora_invalid_witness_tx: MessageSender,
        lora_valid_poc_tx: MessageSender,
    ) -> Result {
        let db_beacon_reports = Report::get_next_beacons(&self.pool).await?;
        if db_beacon_reports.is_empty() {
            tracing::info!("no beacons ready for verification");
            return Ok(());
        }
        // iterate over the beacons pulled from the db
        // for each get witnesses from the DB
        // if a beacon is invalid, then ev thing is invalid
        // but a beacon could be valid whilst witnesses
        // can be a mix of both valid and invalid
        let beacon_len = db_beacon_reports.len();
        tracing::info!("found {beacon_len} beacons ready for verification");
        for db_beacon in db_beacon_reports {
            let packet_data = &db_beacon.packet_data;
            let beacon_buf: &[u8] = &db_beacon.report_data;
            let beacon_report = LoraBeaconIngestReport::try_from(
                LoraBeaconIngestReportV1::decode(beacon_buf).unwrap(),
            )?;
            let beacon = &beacon_report.report;
            let beaconer_pub_key = &beacon.pub_key;

            let db_witnesses = Report::get_witnesses_for_beacon(&self.pool, packet_data).await?;
            // get the beacon and witness report PBs from the db reports
            let mut witnesses: Vec<LoraWitnessIngestReport> = Vec::new();
            for db_witness in db_witnesses {
                let witness_buf: &[u8] = &db_witness.report_data;
                witnesses.push(LoraWitnessIngestReport::try_from(
                    LoraWitnessIngestReportV1::decode(witness_buf).unwrap(),
                )?)
            }

            //
            // top level checks, dont proceed to validate POC reports if these fail
            //

            // is beaconer allowed to beacon at this time ?
            // any irregularily timed beacons will be rejected
            match LastBeacon::get(&self.pool, &beaconer_pub_key.to_vec()).await? {
                Some(last_beacon) => {
                    let beacon_received_ts = beacon_report.received_timestamp;
                    let interval_since_last_beacon = beacon_received_ts - last_beacon.timestamp;
                    if interval_since_last_beacon.num_minutes() < BEACON_INTERVAL {
                        tracing::debug!("beacon verification failed, reason: IrregularInterval");
                        self.handle_invalid_poc(
                            &beacon_report,
                            witnesses,
                            InvalidReason::IrregularInterval,
                            &lora_valid_poc_tx,
                            &lora_invalid_witness_tx,
                        )
                        .await?;
                        continue;
                    }
                }
                None => {
                    tracing::debug!(
                        "no last beacon timestamp available for this hotspot, ignoring "
                    );
                }
            }

            // Do we have recognised entropy included in the beacon report ?
            // if not then go no further, await next tick
            // if we never recognise it, the report will eventually be purged
            let entropy_hash = Sha256::digest(&beacon.remote_entropy).to_vec();
            tracing::debug!("beacon entropy {:?}", &beacon.remote_entropy);
            tracing::debug!("beacon entropy id {:?}", &entropy_hash);
            let entropy_info = match Entropy::get(&self.pool, &entropy_hash).await? {
                Some(res) => res,
                None => {
                    tracing::debug!("beacon verification failed, reason: EntropyNotFound");
                    continue;
                }
            };

            // tmp hack below when testing locally with no entropy server
            // replace entropy_info declaration above with that below
            // let entropy_info = Entropy {
            //     id: entropy_hash.clone(),
            //     data: entropy_hash.clone(),
            //     timestamp: Utc::now() - Duration::seconds(40000),
            //     created_at: Utc::now(),
            // };

            //
            // top level checks complete, verify the POC reports
            //

            // TODO: must be a better approach with this POC struct...
            let mut poc = Poc::new(beacon_report.clone(), witnesses.clone(), entropy_info).await?;

            // verify beacon
            let beacon_verify_result = poc.verify_beacon().await?;
            match beacon_verify_result.result {
                VerificationStatus::Valid => {
                    // beacon is valid, verify the witnesses
                    let beacon_info = beacon_verify_result.gateway_info.unwrap();
                    let verified_witnesses_result = poc.verify_witnesses(&beacon_info).await?;
                    // check if there are any failed witnesses
                    // if so update the DB attempts count
                    // and halt here, let things be reprocessed next tick
                    if !verified_witnesses_result.failed_witnesses.is_empty() {
                        for failed_witness in verified_witnesses_result.failed_witnesses {
                            // something went wrong whilst verifying witnesses
                            // halt here and allow things to be reprocessed next tick
                            let failed_witness = failed_witness.report;
                            // TODO: maybe this ID construction can be pushed out to a trait or part of the report struct ?
                            let failed_witness_public_key = failed_witness.pub_key;
                            let mut failed_witness_id: Vec<u8> = failed_witness.data;
                            failed_witness_id.append(&mut failed_witness_public_key.to_vec());
                            let failed_witness_id_hash =
                                Sha256::digest(&failed_witness_id).to_vec();
                            Report::update_attempts(
                                &self.pool,
                                &failed_witness_id_hash,
                                Utc::now(),
                            )
                            .await?;
                        }
                        continue;
                    };

                    let valid_beacon_report = LoraValidBeaconReport {
                        received_timestamp: beacon_report.received_timestamp,
                        location: beacon_info.location,
                        hex_scale: beacon_verify_result.hex_scale.unwrap(),
                        report: beacon.clone(),
                    };
                    self.handle_valid_poc(
                        beacon,
                        valid_beacon_report,
                        verified_witnesses_result,
                        &lora_valid_poc_tx,
                        &lora_invalid_witness_tx,
                    )
                    .await?;
                }
                VerificationStatus::Invalid => {
                    // the beacon is invalid, which in turn renders all witnesses invalid
                    self.handle_invalid_poc(
                        &beacon_report,
                        witnesses,
                        InvalidReason::BadEntropy,
                        &lora_valid_poc_tx,
                        &lora_invalid_witness_tx,
                    )
                    .await?;
                }
                VerificationStatus::Failed => {
                    // something went wrong whilst verifying the beacon report
                    // halt here and allow things to be reprocessed next tick
                    tracing::info!("failure whilst verifying beacon");
                    // TODO: maybe this ID construction can be pushed out to a trait or part of the report struct ?
                    let failed_beacon_public_key = &beacon.pub_key;
                    let mut failed_beacon_id: Vec<u8> = beacon.data.clone();
                    failed_beacon_id.append(&mut failed_beacon_public_key.to_vec());
                    let failed_beacon_id_hash = Sha256::digest(&failed_beacon_id).to_vec();
                    Report::update_attempts(&self.pool, &failed_beacon_id_hash, Utc::now()).await?;
                }
            }
        }
        Ok(())
    }

    async fn handle_invalid_poc(
        &self,
        beacon_report: &LoraBeaconIngestReport,
        witness_reports: Vec<LoraWitnessIngestReport>,
        invalid_reason: InvalidReason,
        lora_valid_poc_tx: &MessageSender,
        lora_invalid_witness_tx: &MessageSender,
    ) -> Result {
        // the beacon is invalid, which in turn renders all witnesses invalid
        let beacon = &beacon_report.report;
        let beacon_id = beacon.data.clone();
        let invalid_poc: LoraInvalidBeaconReport = LoraInvalidBeaconReport {
            received_timestamp: beacon_report.received_timestamp,
            reason: invalid_reason,
            report: beacon.clone(),
        };
        let invalid_poc_proto: LoraInvalidBeaconReportV1 = invalid_poc.into();
        file_sink::write(lora_valid_poc_tx, invalid_poc_proto).await?;
        for witness_report in witness_reports {
            let invalid_witness_report: LoraInvalidWitnessReport = LoraInvalidWitnessReport {
                received_timestamp: witness_report.received_timestamp,
                report: witness_report.report,
                reason: invalid_reason,
                participant_side: InvalidParticipantSide::Beaconer,
            };
            let invalid_witness_report_proto: LoraInvalidWitnessReportV1 =
                invalid_witness_report.into();
            file_sink::write(lora_invalid_witness_tx, invalid_witness_report_proto).await?;
        }
        // update beacon and all witness reports in the db for this beacon id to invalid
        Report::update_status_all(&self.pool, &beacon_id, LoraStatus::Invalid, Utc::now()).await?;
        Ok(())
    }

    async fn handle_valid_poc(
        &self,
        beacon: &LoraBeaconReport,
        valid_beacon_report: LoraValidBeaconReport,
        witnesses_result: VerifyWitnessesResult,
        lora_valid_poc_tx: &MessageSender,
        lora_invalid_witness_tx: &MessageSender,
    ) -> Result {
        let beacon_id = &beacon.data;
        let valid_poc: LoraValidPoc = LoraValidPoc {
            poc_id: beacon_id.clone(),
            beacon_report: valid_beacon_report,
            witness_reports: witnesses_result.valid_witnesses.clone(),
        };
        let valid_poc_proto: LoraValidPocV1 = valid_poc.into();
        file_sink::write(lora_valid_poc_tx, valid_poc_proto).await?;

        // update db for this beacon, pk is a hash of the poc id and the beaconer pub key
        // TODO: maybe this ID construction can be pushed out to a trait or part of the report struct ?
        let beacon_public_key = beacon.pub_key.clone();
        let mut beaconer_id: Vec<u8> = beacon.data.clone();
        beaconer_id.append(&mut beacon_public_key.to_vec());
        let beaconer_id_hash = Sha256::digest(&beaconer_id).to_vec();
        Report::update_status(&self.pool, &beaconer_id_hash, LoraStatus::Valid, Utc::now()).await?;
        // update last beacon time for the beaconer
        LastBeacon::update_last_timestamp(&self.pool, &beacon_public_key.to_vec(), Utc::now())
            .await?;
        // write out any invalid witnesses
        for invalid_witness_report in witnesses_result.invalid_witnesses {
            let invalid_witness_report_proto: LoraInvalidWitnessReportV1 =
                invalid_witness_report.clone().into();
            file_sink::write(lora_invalid_witness_tx, invalid_witness_report_proto).await?;
            // let invalid_witness = invalid_witness_report.report;
            // update the witness record in the db
            // TODO: maybe this ID construction can be pushed out to a trait or part of the report struct ?
            let invalid_witness_public_key = &invalid_witness_report.report.pub_key;
            let mut invalid_witness_id: Vec<u8> = invalid_witness_report.report.data;
            invalid_witness_id.append(&mut invalid_witness_public_key.to_vec());
            let invalid_witness_id_hash = Sha256::digest(&invalid_witness_id).to_vec();
            Report::update_status(
                &self.pool,
                &invalid_witness_id_hash,
                LoraStatus::Invalid,
                Utc::now(),
            )
            .await?;
        }
        // update DB status of valid witnesses
        for valid_witness_report in witnesses_result.valid_witnesses {
            let valid_witness = valid_witness_report.report;
            // TODO: maybe this ID construction can be pushed out to a trait or part of the report struct ?
            let valid_witness_public_key = valid_witness.pub_key;
            let mut valid_witness_id: Vec<u8> = valid_witness.data;
            valid_witness_id.append(&mut valid_witness_public_key.to_vec());
            let valid_witness_id_hash = Sha256::digest(&valid_witness_id).to_vec();
            Report::update_status(
                &self.pool,
                &valid_witness_id_hash,
                LoraStatus::Valid,
                Utc::now(),
            )
            .await?;
        }

        Ok(())
    }
}
