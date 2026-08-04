use std::sync::Arc;

#[cfg(not(windows))]
use std::collections::HashMap;

use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
#[cfg(not(windows))]
use tokio::sync::RwLock;

use crate::domain::{error::AppError, profile::SecretReference};

#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn put(&self, key: &SecretReference, secret: SecretString) -> Result<(), AppError>;
    async fn get(&self, key: &SecretReference) -> Result<Option<SecretString>, AppError>;
    async fn delete(&self, key: &SecretReference) -> Result<(), AppError>;
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
            keyring::Entry::new("com.s3filemanager.desktop", key.as_str())
                .map_err(|error| AppError::CredentialStore(error.to_string()))?
                .delete_credential()
                .map_err(|error| AppError::CredentialStore(error.to_string()))
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
