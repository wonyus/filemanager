use std::sync::Arc;

#[cfg(not(windows))]
use std::collections::HashMap;

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
#[cfg(not(windows))]
use tokio::sync::RwLock;

use crate::domain::{error::AppError, profile::SecretReference, provider::CredentialMode};

#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn put(&self, key: &SecretReference, secret: SecretString) -> Result<(), AppError>;
    async fn get(&self, key: &SecretReference) -> Result<Option<SecretString>, AppError>;
    async fn delete(&self, key: &SecretReference) -> Result<(), AppError>;
}

#[derive(Debug)]
pub struct ResolvedCredentials {
    pub access_key_id: String,
    pub secret_access_key: SecretString,
    pub session_token: Option<SecretString>,
}

pub async fn resolve_profile_credentials(
    store: &dyn CredentialStore,
    profile: &crate::domain::profile::ConnectionProfile,
) -> Result<ResolvedCredentials, AppError> {
    let access_key_id = profile.access_key_id.clone().ok_or_else(|| {
        AppError::CredentialMissing("access key ID is not configured".to_string())
    })?;
    let secret_reference = profile.secret_reference.as_ref().ok_or_else(|| {
        AppError::CredentialMissing("secret access key is not configured".to_string())
    })?;
    let secret_access_key = store.get(secret_reference).await?.ok_or_else(|| {
        AppError::CredentialMissing("secret access key is unavailable".to_string())
    })?;
    let session_token = match profile.session_reference.as_ref() {
        Some(reference) => store.get(reference).await?,
        None => None,
    };
    if profile.credential_mode == CredentialMode::TemporarySession && session_token.is_none() {
        return Err(AppError::CredentialMissing(
            "temporary session token is unavailable".to_string(),
        ));
    }
    Ok(ResolvedCredentials {
        access_key_id,
        secret_access_key,
        session_token,
    })
}

#[cfg(not(windows))]
#[derive(Default)]
struct MemoryCredentialStore {
    values: RwLock<HashMap<String, SecretString>>,
}

#[cfg(not(windows))]
#[async_trait]
impl CredentialStore for MemoryCredentialStore {
    async fn put(&self, key: &SecretReference, secret: SecretString) -> Result<(), AppError> {
        self.values.write().await.insert(key.0.clone(), secret);
        Ok(())
    }

    async fn get(&self, key: &SecretReference) -> Result<Option<SecretString>, AppError> {
        Ok(self
            .values
            .read()
            .await
            .get(&key.0)
            .map(|value| SecretString::new(value.expose_secret().to_owned().into())))
    }

    async fn delete(&self, key: &SecretReference) -> Result<(), AppError> {
        self.values.write().await.remove(&key.0);
        Ok(())
    }
}

#[cfg(windows)]
struct WindowsCredentialStore;

#[cfg(windows)]
#[async_trait]
impl CredentialStore for WindowsCredentialStore {
    async fn put(&self, key: &SecretReference, secret: SecretString) -> Result<(), AppError> {
        let key = key.clone();
        let value = secret.expose_secret().to_owned();
        tokio::task::spawn_blocking(move || {
            keyring::Entry::new("com.s3filemanager.desktop", key.as_str())
                .map_err(|error| AppError::CredentialStore(error.to_string()))?
                .set_password(&value)
                .map_err(|error| AppError::CredentialStore(error.to_string()))
        })
        .await
        .map_err(|error| AppError::CredentialStore(error.to_string()))??;
        Ok(())
    }

    async fn get(&self, key: &SecretReference) -> Result<Option<SecretString>, AppError> {
        let key = key.clone();
        let result = tokio::task::spawn_blocking(move || {
            let entry = keyring::Entry::new("com.s3filemanager.desktop", key.as_str())
                .map_err(|error| AppError::CredentialStore(error.to_string()))?;
            match entry.get_password() {
                Ok(value) => Ok(Some(SecretString::new(value.into()))),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(error) => Err(AppError::CredentialStore(error.to_string())),
            }
        })
        .await
        .map_err(|error| AppError::CredentialStore(error.to_string()))??;
        Ok(result)
    }

    async fn delete(&self, key: &SecretReference) -> Result<(), AppError> {
        let key = key.clone();
        tokio::task::spawn_blocking(move || {
            let entry = keyring::Entry::new("com.s3filemanager.desktop", key.as_str())
                .map_err(|error| AppError::CredentialStore(error.to_string()))?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(AppError::CredentialStore(error.to_string())),
            }
        })
        .await
        .map_err(|error| AppError::CredentialStore(error.to_string()))??;
        Ok(())
    }
}

pub fn platform_credential_store() -> Arc<dyn CredentialStore> {
    #[cfg(windows)]
    {
        Arc::new(WindowsCredentialStore)
    }
    #[cfg(not(windows))]
    {
        Arc::new(MemoryCredentialStore::default())
    }
}
