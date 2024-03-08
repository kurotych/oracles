use chrono::{DateTime, Utc};
use file_store::{
    file_info_poller::FileInfoStream,
    file_sink::FileSinkClient,
    mobile_radio_threshold::{
        RadioThresholdIngestReport, RadioThresholdReportReq, VerifiedRadioThresholdIngestReport,
    },
};
use futures::{StreamExt, TryStreamExt};
use futures_util::TryFutureExt;
use helium_crypto::PublicKeyBinary;
use helium_proto::services::{
    mobile_config::NetworkKeyRole,
    poc_mobile::{RadioThresholdReportVerificationStatus, VerifiedRadioThresholdIngestReportV1},
};
use mobile_config::client::authorization_client::AuthorizationVerifier;
use sqlx::{FromRow, PgPool, Postgres, Row, Transaction};
use std::{collections::HashSet, ops::Range};
use task_manager::ManagedTask;
use tokio::sync::mpsc::Receiver;

pub struct RadioThresholdIngestor<AV> {
    pool: PgPool,
    reports_receiver: Receiver<FileInfoStream<RadioThresholdIngestReport>>,
    verified_report_sink: FileSinkClient,
    authorization_verifier: AV,
}

impl<AV> ManagedTask for RadioThresholdIngestor<AV>
where
    AV: AuthorizationVerifier + Send + Sync + 'static,
{
    fn start_task(
        self: Box<Self>,
        shutdown: triggered::Listener,
    ) -> futures_util::future::LocalBoxFuture<'static, anyhow::Result<()>> {
        let handle = tokio::spawn(self.run(shutdown));
        Box::pin(
            handle
                .map_err(anyhow::Error::from)
                .and_then(|result| async move { result.map_err(anyhow::Error::from) }),
        )
    }
}

impl<AV> RadioThresholdIngestor<AV>
where
    AV: AuthorizationVerifier,
{
    pub fn new(
        pool: sqlx::Pool<Postgres>,
        reports_receiver: Receiver<FileInfoStream<RadioThresholdIngestReport>>,
        verified_report_sink: FileSinkClient,
        authorization_verifier: AV,
    ) -> Self {
        Self {
            pool,
            reports_receiver,
            verified_report_sink,
            authorization_verifier,
        }
    }

    async fn run(mut self, shutdown: triggered::Listener) -> anyhow::Result<()> {
        tracing::info!("starting radio threshold ingestor");
        loop {
            tokio::select! {
                biased;
                _ = shutdown.clone() => break,
                Some(file) = self.reports_receiver.recv() => {
                    self.process_file(file).await?;
                }
            }
        }
        tracing::info!("stopping radio threshold ingestor");
        Ok(())
    }

    async fn process_file(
        &self,
        file_info_stream: FileInfoStream<RadioThresholdIngestReport>,
    ) -> anyhow::Result<()> {
        let mut transaction = self.pool.begin().await?;
        file_info_stream
            .into_stream(&mut transaction)
            .await?
            .map(anyhow::Ok)
            .try_fold(transaction, |mut transaction, ingest_report| async move {
                // verify the report
                let verified_report_status = self.verify_report(&ingest_report.report).await?;

                // if the report is valid then save to the db
                // and thus available to the rewarder
                match verified_report_status {
                    RadioThresholdReportVerificationStatus::ThresholdReportStatusValid
                    | RadioThresholdReportVerificationStatus::ThresholdReportStatusLegacyValid => {
                        save(&ingest_report, &mut transaction).await?;
                    }
                    _ => {}
                }

                // write out paper trail of verified report, valid or invalid
                let verified_report_proto: VerifiedRadioThresholdIngestReportV1 =
                    VerifiedRadioThresholdIngestReport {
                        report: ingest_report,
                        status: verified_report_status,
                        timestamp: Utc::now(),
                    }
                    .into();
                self.verified_report_sink
                    .write(
                        verified_report_proto,
                        &[("report_status", verified_report_status.as_str_name())],
                    )
                    .await?;
                Ok(transaction)
            })
            .await?
            .commit()
            .await?;
        self.verified_report_sink.commit().await?;
        Ok(())
    }

    async fn verify_report(
        &self,
        report: &RadioThresholdReportReq,
    ) -> anyhow::Result<RadioThresholdReportVerificationStatus> {
        let is_legacy = self
            .verify_legacy(&report.hotspot_pubkey, &report.cbsd_id)
            .await?;
        let report_validity = self.do_report_verifications(report).await;
        let final_validity = if is_legacy
            && report_validity == RadioThresholdReportVerificationStatus::ThresholdReportStatusValid
        {
            RadioThresholdReportVerificationStatus::ThresholdReportStatusLegacyValid
        } else {
            report_validity
        };
        Ok(final_validity)
    }

    async fn do_report_verifications(
        &self,
        report: &RadioThresholdReportReq,
    ) -> RadioThresholdReportVerificationStatus {
        if !self.verify_known_carrier_key(&report.carrier_pub_key).await {
            return RadioThresholdReportVerificationStatus::ThresholdReportStatusInvalidCarrierKey;
        };
        RadioThresholdReportVerificationStatus::ThresholdReportStatusValid
    }

    async fn verify_known_carrier_key(&self, public_key: &PublicKeyBinary) -> bool {
        match self
            .authorization_verifier
            .verify_authorized_key(public_key, NetworkKeyRole::MobileCarrier)
            .await
        {
            Ok(res) => res,
            Err(_err) => false,
        }
    }

    async fn verify_legacy(
        &self,
        hotspot_key: &PublicKeyBinary,
        cbsd_id: &Option<String>,
    ) -> Result<bool, sqlx::Error> {
        // check if the radio has been grandfathered in, meaning a radio which has received
        // boosted rewards prior to the data component of hip84 going live
        // if true then it is assigned a status reason of Legacy
        // TODO: remove this handling after the grandfathering period
        let row = sqlx::query(" select exists(select 1 from grandfathered_radio_threshold where hotspot_pubkey = $1 and (cbsd_id is null or cbsd_id = $2)) ")
        .bind(hotspot_key)
        .bind(cbsd_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(row.get("exists"))
    }
}

pub async fn save(
    ingest_report: &RadioThresholdIngestReport,
    db: &mut Transaction<'_, Postgres>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
            INSERT INTO radio_threshold (
            hotspot_pubkey,
            cbsd_id,
            bytes_threshold,
            subscriber_threshold,
            threshold_timestamp,
            threshold_met,
            recv_timestamp)
            VALUES ($1, $2, $3, $4, $5, true, $6)
            ON CONFLICT (hotspot_pubkey, COALESCE(cbsd_id, ''))
            DO UPDATE SET
            bytes_threshold = EXCLUDED.bytes_threshold,
            subscriber_threshold = EXCLUDED.subscriber_threshold,
            threshold_timestamp = EXCLUDED.threshold_timestamp,
            recv_timestamp = EXCLUDED.recv_timestamp,
            updated_at = NOW()
            "#,
    )
    .bind(ingest_report.report.hotspot_pubkey.to_string())
    .bind(ingest_report.report.cbsd_id.clone())
    .bind(ingest_report.report.bytes_threshold as i64)
    .bind(ingest_report.report.subscriber_threshold as i32)
    .bind(ingest_report.report.threshold_timestamp)
    .bind(ingest_report.received_timestamp)
    .execute(&mut *db)
    .await?;
    Ok(())
}

#[derive(FromRow, Debug)]
pub struct RadioThreshold {
    hotspot_pubkey: PublicKeyBinary,
    cbsd_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct VerifiedRadioThresholds {
    gateways: HashSet<(PublicKeyBinary, Option<String>)>,
}

impl VerifiedRadioThresholds {
    pub fn insert(&mut self, hotspot_key: PublicKeyBinary, cbsd_id: Option<String>) {
        self.gateways.insert((hotspot_key, cbsd_id));
    }

    pub fn is_verified(&self, key: PublicKeyBinary, cbsd_id: Option<String>) -> bool {
        self.gateways.contains(&(key, cbsd_id))
    }
}

pub async fn verified_radio_thresholds(
    pool: &sqlx::Pool<Postgres>,
    reward_period: &Range<DateTime<Utc>>,
) -> Result<VerifiedRadioThresholds, sqlx::Error> {
    let mut rows = sqlx::query_as::<_, RadioThreshold>(
        "SELECT hotspot_pubkey, cbsd_id
             FROM radio_threshold WHERE threshold_timestamp < $1",
    )
    .bind(reward_period.end)
    .fetch(pool);
    let mut map = VerifiedRadioThresholds::default();
    while let Some(row) = rows.try_next().await? {
        map.insert(row.hotspot_pubkey, row.cbsd_id);
    }
    Ok(map)
}
