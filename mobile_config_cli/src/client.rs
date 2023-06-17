use crate::{
    current_timestamp,
    cmds::gateway::GatewayInfo, KeyRole, Result,
};
use helium_crypto::{Keypair, PublicKey, Sign, Verify};
use helium_proto::{
    services::mobile_config::{
        admin_client, authorization_client, entity_client, gateway_client,
        AdminAddKeyReqV1, AdminKeyResV1, AdminRemoveKeyReqV1, AuthorizationVerifyReqV1,
        AuthorizationVerifyResV1, AuthorizationListReqV1, AuthorizationListResV1,
        EntityVerifyReqV1, EntityVerifyResV1, GatewayInfoReqV1, GatewayInfoResV1,
    },
    Message,
};
use std::str::FromStr;

pub struct AdminClient {
    client: admin_client::AdminClient<helium_proto::services::Channel>,
    server_pubkey: PublicKey,
}

pub struct AuthClient {
    client: authorization_client::AuthorizationClient<helium_proto::services::Channel>,
    server_pubkey: PublicKey,
}

pub struct EntityClient {
    client: entity_client::EntityClient<helium_proto::services::Channel>,
    server_pubkey: PublicKey,
}

pub struct GatewayClient {
    client: gateway_client::GatewayClient<helium_proto::services::Channel>,
    server_pubkey: PublicKey,
}

impl AdminClient {
    pub async fn new(host: &str, server_pubkey: &str) -> Result<Self> {
        Ok(Self {
            client: admin_client::AdminClient::connect(host.to_owned()).await?,
            server_pubkey: PublicKey::from_str(server_pubkey)?,
        })
    }

    pub async fn add_key(
        &mut self,
        pubkey: &PublicKey,
        key_role: KeyRole,
        keypair: &Keypair,
    ) -> Result {
        let mut request = AdminAddKeyReqV1 {
            pubkey: pubkey.into(),
            role: key_role.into(),
            signer: keypair.public_key().into(),
            signature: vec![],
            timestamp: current_timestamp()?,
        };
        request.signature = request.sign(keypair)?;
        self.client.add_key(request).await?.into_inner().verify(&self.server_pubkey)
    }

    pub async fn remove_key(
        &mut self,
        pubkey: &PublicKey,
        key_role: KeyRole,
        keypair: &Keypair,
    ) -> Result {
        let mut request = AdminRemoveKeyReqV1 {
            pubkey: pubkey.into(),
            role: key_role.into(),
            signer: keypair.public_key().into(),
            signature: vec![],
            timestamp: current_timestamp()?,
        };
        request.signature = request.sign(keypair)?;
        self.client.remove_key(request).await?.into_inner().verify(&self.server_pubkey)
    }
}

impl AuthClient {
    pub async fn new(host: &str, server_pubkey: &str) -> Result<Self> {
        Ok(Self {
            client: authorization_client::AuthorizationClient::connect(host.to_owned()).await?,
            server_pubkey: PublicKey::from_str(server_pubkey)?,
        })
    }

    pub async fn verify(&mut self, pubkey: &PublicKey, role: KeyRole, keypair: &Keypair) -> Result<bool> {
        let mut request = AuthorizationVerifyReqV1 {
            pubkey,
            role: role.try_into()?,
            signer: keypair.public_key().into(),
            signature: vec![],
        };
        request.signature = request.sign(keypair)?;
        if let Ok(response) = self.client.verify(request).await {
            response.into_inner().verify(&self.server_pubkey).map_err(|_| anyhow::anyhow!("invalid response signature"))?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn list(&mut self, role: KeyRole, keypair: &Keypair) -> Result<Vec<PublicKey>> {
        let mut request = AuthorizationListReqV1 {
            role: role.try_into()?,
            signer: keypair.public_key().into(),
            signature: vec![],
        };
        request.signature = request.sign(keypair)?;
        let response = self.client.verify(request).await?.into_inner();
        response.verify(&self.server_pubkey)?;
        Ok(response.pubkeys.into_iter().map(|pubkeybin| PublicKey::try_from).collect())
    }
}

impl EntityClient {
    pub async fn new(host: &str, server_pubkey: &str) -> Result<Self> {
        Ok(Self {
            client: entity_client::EntityClient::connect(host.to_owned()).await?,
            server_pubkey: PublicKey::from_str(server_pubkey)?,
        })
    }

    pub async fn verify(&mut self, entity: &str) -> Result<bool> {
        let mut request = EntityVerifyReqV1 {
            entity_id: base64::engine::general_purpose::STANDARD.decode(entity)?,
            signer: Keypair.public_key().into(),
            signature: vec![],
        };
        request.signature = request.sign(keypair)?;
        match self.client.verify(request).await {
            Ok(response) => {
                response.into_inner().verify(&self.server_pubkey)?;
                Ok(true)
            }
            Err(status) if status.code() == tonic::Code::NotFound => Ok(false),
            Err(error) => Err(error)?
        }
    }
}

impl GatewayClient {
    pub async fn new(host: &str, server_pubkey: &str) -> Result<Self> {
        Ok(Self {
            client: gateway_client::GatewayClient::connect(host.to_owned()).await?,
            server_pubkey: PublicKey::from_str(server_pubkey)?,
        })
    }

    pub async fn info(&mut self, gateway: &PublicKey, keypair: &Keypair) -> Result<GatewayInfo> {
        let mut request = GatewayInfoReqV1 {
            address: gateway.into(),
            signer: keypair.public_key().into(),
            signature: vec![],
        };
        request.signature = request.sign(keypair)?;
        let response = self.client.info(request).await?.into_inner();
        response.verify(&self.server_pubkey)?;
        let info = response.info.ok_or_else(|| anyhow::anyhow!("gateway not found"))?;
        info.try_into()
    }
}

pub trait MsgSign: Message + std::clone::Clone {
    fn sign(&self, keypair: &Keypair) -> Result<Vec<u8>>
    where
        Self: std::marker::Sized;
}

macro_rules! impl_sign {
    ($msg_type:ty, $( $sig: ident ),+ ) => {
        impl MsgSign for $msg_type {
            fn sign(&self, keypair: &Keypair) -> Result<Vec<u8>> {
                let mut msg = self.clone();
                $(msg.$sig = vec![];)+
                Ok(keypair.sign(&msg.encode_to_vec())?)
            }
        }
    }
}

impl_sign!(AdminAddKeyReqV1, signature);
impl_sign!(AdminRemoveKeyReqV1, signature);
impl_sign!(AuthorizationVerifyReqV1, signature);
impl_sign!(AuthorizationListReqV1, signature);
impl_sign!(EntityVerifyReqV1, signature);
impl_sign!(GatewayInfoReqV1, signature);

pub trait MsgVerify: Message + std::clone::Clone {
    fn verify(&self, verifier: &PublicKey) -> Result
    where
        Self: std::marker::Sized;
}

macro_rules! impl_verify {
    ($msg_type:ty, $sig: ident) => {
        impl MsgVerify for $msg_type {
            fn verify(&self, verifier: &PublicKey) -> Result {
                let mut buf = vec![];
                let mut msg = self.clone();
                msg.$sig = vec![];
                msg.encode(&mut buf)?;
                verifier.verify(&buf, &self.$sig)
                        .map_err(anyhow::Error::from)
            }
        }
    };
}

impl_verify!(AdminKeyResV1, signature);
impl_verify!(AuthorizationVerifyResV1, signature);
impl_verify!(AuthorizationListResV1, signature);
impl_verify!(EntityVerifyResV1, signature);
impl_verify!(GatewayInfoResV1, signature);
