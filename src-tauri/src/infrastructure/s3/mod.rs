use std::{collections::HashMap, sync::Arc};

use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::{config::Credentials, Client};
use tokio::sync::RwLock;

use crate::{
    domain::{error::AppError, profile::ConnectionProfile, provider::AddressingStyle},
    infrastructure::credentials::ResolvedCredentials,
};

#[derive(Default)]
pub struct S3ClientManager {
    clients: RwLock<HashMap<String, Arc<Client>>>,
}

impl S3ClientManager {
    pub async fn get_or_create(
        &self,
        profile: &ConnectionProfile,
        credentials: &ResolvedCredentials,
    ) -> Result<Arc<Client>, AppError> {
        let cache_key = format!(
            "{}:{}:{}",
            profile.id,
            profile.revision,
            profile
                .secret_reference
                .as_ref()
                .map(|value| value.as_str())
                .unwrap_or("none")
        );
        if let Some(client) = self.clients.read().await.get(&cache_key).cloned() {
            return Ok(client);
        }

        self.build_client(profile, credentials, Some(cache_key))
            .await
    }

    pub async fn build_temporary(
        &self,
        profile: &ConnectionProfile,
        credentials: &ResolvedCredentials,
    ) -> Result<Arc<Client>, AppError> {
        self.build_client(profile, credentials, None).await
    }

    async fn build_client(
        &self,
        profile: &ConnectionProfile,
        credentials: &ResolvedCredentials,
        cache_key: Option<String>,
    ) -> Result<Arc<Client>, AppError> {
        let credentials = Credentials::new(
            credentials.access_key_id.clone(),
            credentials.secret_access_key.expose_secret().to_owned(),
            credentials
                .session_token
                .as_ref()
                .map(|value| value.expose_secret().to_owned()),
            None,
            "s3-file-manager",
        );
        let loader = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(profile.region.clone()))
            .credentials_provider(credentials);
        let shared_config = loader.load().await;
        let mut builder = aws_sdk_s3::config::Builder::from(&shared_config);
        if let Some(endpoint) = &profile.endpoint {
            builder = builder.endpoint_url(endpoint);
        }
        builder = builder.force_path_style(profile.addressing_style == AddressingStyle::Path);
        let client = Arc::new(Client::from_conf(builder.build()));
        if let Some(cache_key) = cache_key {
            self.clients.write().await.insert(cache_key, client.clone());
        }
        Ok(client)
    }

    pub async fn invalidate(&self, profile_id: uuid::Uuid) {
        self.clients
            .write()
            .await
            .retain(|key, _| !key.starts_with(&profile_id.to_string()));
    }
}

use secrecy::ExposeSecret;
