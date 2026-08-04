use std::sync::Arc;

use crate::infrastructure::{
    credentials::{platform_credential_store, CredentialStore},
    database::Database,
};

pub struct AppState {
    pub database: Database,
    pub credentials: Arc<dyn CredentialStore>,
}

impl AppState {
    pub fn new(database: Database) -> Self {
        Self {
            database,
            credentials: platform_credential_store(),
        }
    }
}
