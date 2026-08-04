use std::{collections::HashMap, sync::Arc};

use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::{config::Credentials, Client};
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};

use crate::{
    domain::{error::AppError, profile::ConnectionProfile, provider::AddressingStyle},
    infrastructure::credentials::ResolvedCredentials,
};

#[derive(Default)]
pub struct S3ClientManager {
    clients: RwLock<HashMap<String, Arc<Client>>>,
    request_limiters: RwLock<HashMap<String, Arc<Semaphore>>>,
}

impl S3ClientManager {
    /// Configure the per-profile request budget captured for a newly created
    /// transfer. Existing permits are adjusted conservatively; in-flight
    /// requests are never revoked.
    pub async fn configure_request_limit(&self, profile_id: &str, limit: u8) {
        let limit = usize::from(limit.clamp(1, 32));
        let mut limiters = self.request_limiters.write().await;
        if let Some(semaphore) = limiters.get(profile_id) {
            let available = semaphore.available_permits();
            if available < limit {
                semaphore.add_permits(limit - available);
            } else if available > limit {
                semaphore.forget_permits(available - limit);
            }
        } else {
            limiters.insert(profile_id.to_string(), Arc::new(Semaphore::new(limit)));
        }
    }

    /// Acquire one permit for a provider request belonging to a profile.
    /// Callers hold the guard for the lifetime of the SDK future.
    pub async fn acquire_request(
        &self,
        profile_id: &str,
    ) -> Result<OwnedSemaphorePermit, AppError> {
        self.acquire_requests(profile_id, 1).await
    }

    /// Reserve the maximum number of provider requests a transfer can issue
    /// concurrently.  Multipart workers hold one permit each, so reserving
    /// the per-job width prevents several jobs for the same profile from
    /// collectively exceeding the profile request budget.
    pub async fn acquire_requests(
        &self,
        profile_id: &str,
        permits: usize,
    ) -> Result<OwnedSemaphorePermit, AppError> {
        let semaphore = {
            let mut limiters = self.request_limiters.write().await;
            limiters
                .entry(profile_id.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(8)))
                .clone()
        };
        let permits = permits.clamp(1, 32) as u32;
        semaphore.acquire_many_owned(permits).await.map_err(|_| {
            AppError::TransferStateConflict("profile request limiter is unavailable".to_string())
        })
    }

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
        self.request_limiters
            .write()
            .await
            .remove(&profile_id.to_string());
    }
}

use secrecy::ExposeSecret;
