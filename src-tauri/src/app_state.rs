use std::path::PathBuf;
use std::sync::Arc;

use crate::application::{profile_service::ProfileService, transfer_service::TransferService};
use crate::dto::settings::SettingsSnapshot;
use crate::infrastructure::{
    credentials::{platform_credential_store, CredentialStore},
    database::Database,
    logging::DiagnosticsService,
    s3::S3ClientManager,
};
use crate::transfer::{settings::SettingsService, TransferManager};

pub struct AppState {
    pub database: Database,
    pub credentials: Arc<dyn CredentialStore>,
    pub clients: Arc<S3ClientManager>,
    pub profiles: Arc<ProfileService>,
    pub transfers: Arc<TransferManager>,
    pub settings: Arc<SettingsService>,
    pub transfer_service: Arc<TransferService>,
    pub diagnostics: Arc<DiagnosticsService>,
}

impl AppState {
    pub fn new(database: Database) -> Self {
        Self::new_with_settings(database, SettingsSnapshot::default())
    }

    pub fn new_with_settings(database: Database, initial_settings: SettingsSnapshot) -> Self {
        Self::new_with_settings_and_data_dir(
            database,
            initial_settings,
            std::env::temp_dir().join("s3-file-manager"),
        )
    }

    pub fn new_with_settings_and_data_dir(
        database: Database,
        initial_settings: SettingsSnapshot,
        data_dir: PathBuf,
    ) -> Self {
        let initial_settings = initial_settings.normalized();
        let initial_settings = if initial_settings.validate().is_ok() {
            initial_settings
        } else {
            SettingsSnapshot::default()
        };
        let credentials = platform_credential_store();
        let clients = Arc::new(S3ClientManager::default());
        let settings = Arc::new(SettingsService::new(initial_settings.clone()));
        // The scheduler is created from the persisted setting so a restart
        // cannot silently fall back to the hard-coded default of four jobs.
        let transfers = Arc::new(TransferManager::new_with_database(
            usize::from(initial_settings.concurrent_jobs),
            database.clone(),
        ));
        let diagnostics = Arc::new(DiagnosticsService::new(data_dir));
        let profiles = Arc::new(ProfileService::new(
            database.clone(),
            credentials.clone(),
            clients.clone(),
            transfers.clone(),
        ));
        let transfer_service = Arc::new(TransferService::new(
            profiles.clone(),
            credentials.clone(),
            clients.clone(),
            transfers.clone(),
            settings.clone(),
        ));
        Self {
            database,
            credentials,
            clients,
            profiles,
            transfers,
            settings,
            transfer_service,
            diagnostics,
        }
    }
}
