use std::str::FromStr;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub type StoreTx = Transaction<'static, Postgres>;

#[derive(Clone)]
pub struct StoreDb {
    pool: PgPool,
}

impl StoreDb {
    pub fn connect_lazy(database_url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        let options = PgConnectOptions::from_str(database_url)?.application_name("aion_session_store");
        let pool = PgPoolOptions::new().max_connections(max_connections).connect_lazy_with(options);
        Ok(Self { pool })
    }

    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Transaction restricted by RLS to `workspace_id`.
    pub async fn for_workspace(&self, workspace_id: Uuid) -> Result<StoreTx, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT set_config('app.workspace_id', $1, true)")
            .bind(workspace_id.to_string())
            .execute(&mut *tx)
            .await?;
        Ok(tx)
    }

    /// Platform / unconstrained transaction.
    pub async fn for_platform(&self) -> Result<StoreTx, sqlx::Error> {
        self.pool.begin().await
    }

    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        sqlx::query("SELECT 1").execute(&self.pool).await.map(|_| ())
    }
}

pub async fn migrate(owner_database_url: &str) -> Result<(), sqlx::Error> {
    let pool = PgPoolOptions::new().max_connections(1).connect(owner_database_url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    pool.close().await;
    Ok(())
}
