use crate::{
    gateway_info::{self, GatewayInfo},
    key_cache::KeyCache,
    telemetry, verify_public_key, GrpcResult, GrpcStreamResult,
};
use chrono::Utc;
use file_store::traits::{MsgVerify, TimestampEncode};
use futures::{
    stream::{Stream, StreamExt, TryStreamExt},
    TryFutureExt,
};
use helium_crypto::{Keypair, PublicKey, PublicKeyBinary, Sign};
use helium_proto::{
    services::mobile_config::{
        self, GatewayInfoBatchReqV1, GatewayInfoReqV1, GatewayInfoResV1, GatewayInfoStreamReqV1,
        GatewayInfoStreamResV1,
    },
    Message,
};
use sqlx::{Pool, Postgres};
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub struct GatewayService {
    key_cache: KeyCache,
    metadata_pool: Pool<Postgres>,
    signing_key: Arc<Keypair>,
}

impl GatewayService {
    pub fn new(key_cache: KeyCache, metadata_pool: Pool<Postgres>, signing_key: Keypair) -> Self {
        Self {
            key_cache,
            metadata_pool,
            signing_key: Arc::new(signing_key),
        }
    }

    fn verify_request_signature<R>(&self, signer: &PublicKey, request: &R) -> Result<(), Status>
    where
        R: MsgVerify,
    {
        if self.key_cache.verify_signature(signer, request).is_ok() {
            tracing::debug!(signer = signer.to_string(), "request authorized");
            return Ok(());
        }
        Err(Status::permission_denied("unauthorized request signature"))
    }

    fn verify_request_signature_for_info(&self, request: &GatewayInfoReqV1) -> Result<(), Status> {
        let signer = verify_public_key(&request.signer)?;
        let address = verify_public_key(&request.address)?;

        if address == signer && request.verify(&signer).is_ok() {
            tracing::debug!(%signer, "self authorized");
            return Ok(());
        }

        self.verify_request_signature(&signer, request)
    }

    fn sign_response(&self, response: &[u8]) -> Result<Vec<u8>, Status> {
        self.signing_key
            .sign(response)
            .map_err(|_| Status::internal("response signing error"))
    }
}

#[tonic::async_trait]
impl mobile_config::Gateway for GatewayService {
    async fn info(&self, request: Request<GatewayInfoReqV1>) -> GrpcResult<GatewayInfoResV1> {
        let request = request.into_inner();
        telemetry::count_request("gateway", "info");
        custom_tracing::record_b58("pub_key", &request.address);
        custom_tracing::record_b58("signer", &request.signer);

        self.verify_request_signature_for_info(&request)?;

        let pubkey: PublicKeyBinary = request.address.into();
        tracing::debug!(pubkey = pubkey.to_string(), "fetching gateway info");

        gateway_info::db::get_info(&self.metadata_pool, &pubkey)
            .await
            .map_err(|_| Status::internal("error fetching gateway info"))?
            .map_or_else(
                || {
                    telemetry::count_gateway_chain_lookup("not-found");
                    Err(Status::not_found(pubkey.to_string()))
                },
                |info| {
                    if info.metadata.is_some() {
                        telemetry::count_gateway_chain_lookup("asserted");
                    } else {
                        telemetry::count_gateway_chain_lookup("not-asserted");
                    };
                    let info = info
                        .try_into()
                        .map_err(|_| Status::internal("error serializing gateway info"))?;
                    let mut res = GatewayInfoResV1 {
                        info: Some(info),
                        timestamp: Utc::now().encode_timestamp(),
                        signer: self.signing_key.public_key().into(),
                        signature: vec![],
                    };
                    res.signature = self.sign_response(&res.encode_to_vec())?;
                    Ok(Response::new(res))
                },
            )
    }

    type info_batchStream = GrpcStreamResult<GatewayInfoStreamResV1>;
    async fn info_batch(
        &self,
        request: Request<GatewayInfoBatchReqV1>,
    ) -> GrpcResult<Self::info_streamStream> {
        let request = request.into_inner();
        telemetry::count_request("gateway", "info-batch");
        custom_tracing::record_b58("signer", &request.signer);

        let signer = verify_public_key(&request.signer)?;
        self.verify_request_signature(&signer, &request)?;

        tracing::debug!(
            batch = request.addresses.len(),
            "fetching gateways' info batch"
        );

        let pool = self.metadata_pool.clone();
        let signing_key = self.signing_key.clone();
        let batch_size = request.batch_size;
        let addresses = request
            .addresses
            .into_iter()
            .map(|key| key.into())
            .collect::<Vec<PublicKeyBinary>>();

        let (tx, rx) = tokio::sync::mpsc::channel(100);

        tokio::spawn(async move {
            let stream = gateway_info::db::batch_info_stream(&pool, &addresses)?;
            stream_multi_gateways_info(stream, tx.clone(), signing_key.clone(), batch_size).await
        });

        Ok(Response::new(GrpcStreamResult::new(rx)))
    }

    type info_streamStream = GrpcStreamResult<GatewayInfoStreamResV1>;
    async fn info_stream(
        &self,
        request: Request<GatewayInfoStreamReqV1>,
    ) -> GrpcResult<Self::info_streamStream> {
        let request = request.into_inner();
        telemetry::count_request("gateway", "info-stream");
        custom_tracing::record_b58("signer", &request.signer);

        let signer = verify_public_key(&request.signer)?;
        self.verify_request_signature(&signer, &request)?;

        tracing::debug!("fetching all gateways' info");

        let pool = self.metadata_pool.clone();
        let signing_key = self.signing_key.clone();
        let batch_size = request.batch_size;

        let (tx, rx) = tokio::sync::mpsc::channel(100);

        tokio::spawn(async move {
            let stream = gateway_info::db::all_info_stream(&pool);
            stream_multi_gateways_info(stream, tx.clone(), signing_key.clone(), batch_size).await
        });

        Ok(Response::new(GrpcStreamResult::new(rx)))
    }
}

async fn stream_multi_gateways_info(
    stream: impl Stream<Item = GatewayInfo>,
    tx: tokio::sync::mpsc::Sender<Result<GatewayInfoStreamResV1, Status>>,
    signing_key: Arc<Keypair>,
    batch_size: u32,
) -> anyhow::Result<()> {
    let timestamp = Utc::now().encode_timestamp();
    let signer: Vec<u8> = signing_key.public_key().into();
    Ok(stream
        .map(Ok::<GatewayInfo, sqlx::Error>)
        .try_filter_map(|info| async move {
            let result: Option<mobile_config::GatewayInfo> = info.try_into().ok();
            Ok(result)
        })
        .try_chunks(batch_size as usize)
        .map_ok(move |batch| {
            (
                GatewayInfoStreamResV1 {
                    gateways: batch,
                    timestamp,
                    signer: signer.clone(),
                    signature: vec![],
                },
                signing_key.clone(),
            )
        })
        .try_filter_map(|(res, keypair)| async move {
            let result = match keypair.sign(&res.encode_to_vec()) {
                Ok(signature) => Some(GatewayInfoStreamResV1 {
                    gateways: res.gateways,
                    timestamp: res.timestamp,
                    signer: res.signer,
                    signature,
                }),
                Err(_) => None,
            };
            Ok(result)
        })
        .map_err(|err| Status::internal(format!("info batch failed with reason: {err:?}")))
        .try_for_each(|res| {
            tx.send(Ok(res))
                .map_err(|err| Status::internal(format!("info batch send failed {err:?}")))
        })
        .or_else(|err| {
            tx.send(Err(Status::internal(format!(
                "info batch failed with reason: {err:?}"
            ))))
        })
        .await?)
}
