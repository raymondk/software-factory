use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};

pub async fn open(path: &Path) -> anyhow::Result<SqlitePool> {
    let opts = SqliteConnectOptions::new().filename(path).create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}
