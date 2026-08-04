use std::path::Path;

use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};

use crate::{
    domain::{error::AppError, provider::ProviderType},
    dto::profile::ProfileSummary,
};

#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    pub async fn connect(path: &Path) -> Result<Self, AppError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn list_profiles(&self) -> Result<Vec<ProfileSummary>, AppError> {
        let rows = sqlx::query(
            "SELECT id, name, provider, region, default_bucket, favorite, last_connected_at
             FROM connection_profiles ORDER BY favorite DESC, name COLLATE NOCASE ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(ProfileSummary {
                    id: row.try_get("id")?,
                    name: row.try_get("name")?,
                    provider: ProviderType::parse(row.try_get::<String, _>("provider")?.as_str()),
                    region: row.try_get("region")?,
                    default_bucket: row.try_get("default_bucket")?,
                    favorite: row.try_get::<i64, _>("favorite")? != 0,
                    last_connected_at: row.try_get("last_connected_at")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()
            .map_err(AppError::from)
    }
}
