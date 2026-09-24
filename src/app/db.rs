use anyhow::{Context, bail};
use sqlx::{AnyPool, any::AnyPoolOptions};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct QueuedDelivery {
    pub id: String,
    pub event: String,
    pub payload: String,
    pub attempts: i32,
}

#[derive(Clone)]
pub struct DeliveryStore {
    pool: AnyPool,
    sqlite: bool,
}

impl DeliveryStore {
    pub async fn connect(database_url: &str) -> anyhow::Result<Self> {
        let sqlite = database_url.starts_with("sqlite:");
        if !sqlite
            && !database_url.starts_with("postgres://")
            && !database_url.starts_with("postgresql://")
        {
            bail!("CURURU_DATABASE_URL must be a sqlite: or postgres:// URL");
        }

        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await
            .context("failed to connect to Cururu event database")?;
        if sqlite {
            sqlx::query("PRAGMA journal_mode = WAL")
                .execute(&pool)
                .await
                .context("failed to enable SQLite WAL mode")?;
            sqlx::query("PRAGMA synchronous = FULL")
                .execute(&pool)
                .await
                .context("failed to set SQLite durability mode")?;
        }

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS github_deliveries (
                delivery_id TEXT PRIMARY KEY,
                event_type TEXT NOT NULL,
                payload TEXT NOT NULL,
                state TEXT NOT NULL,
                attempts BIGINT NOT NULL DEFAULT 0,
                available_at BIGINT NOT NULL,
                received_at BIGINT NOT NULL,
                updated_at BIGINT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .context("failed to initialize webhook delivery queue")?;
        sqlx::query("CREATE INDEX IF NOT EXISTS github_deliveries_ready ON github_deliveries(state, available_at, received_at)")
            .execute(&pool)
            .await
            .context("failed to index webhook delivery queue")?;

        let now = epoch_seconds();
        let recover_query = if sqlite {
            "UPDATE github_deliveries SET state = 'queued', updated_at = ? WHERE state = 'processing' AND updated_at < ?"
        } else {
            "UPDATE github_deliveries SET state = 'queued', updated_at = $1 WHERE state = 'processing' AND updated_at < $2"
        };
        sqlx::query(recover_query)
            .bind(now)
            .bind(now - 300)
            .execute(&pool)
            .await
            .context("failed to recover interrupted webhook deliveries")?;
        let prune_query = if sqlite {
            "DELETE FROM github_deliveries WHERE state IN ('done', 'failed') AND received_at < ?"
        } else {
            "DELETE FROM github_deliveries WHERE state IN ('done', 'failed') AND received_at < $1"
        };
        sqlx::query(prune_query)
            .bind(now - 7 * 24 * 60 * 60)
            .execute(&pool)
            .await
            .context("failed to prune old webhook deliveries")?;

        Ok(Self { pool, sqlite })
    }

    pub async fn enqueue(&self, id: &str, event: &str, payload: &str) -> anyhow::Result<bool> {
        let now = epoch_seconds();
        let query = self.sql(
            "INSERT INTO github_deliveries (delivery_id, event_type, payload, state, attempts, available_at, received_at, updated_at) VALUES (?, ?, ?, 'queued', 0, ?, ?, ?) ON CONFLICT(delivery_id) DO NOTHING",
            "INSERT INTO github_deliveries (delivery_id, event_type, payload, state, attempts, available_at, received_at, updated_at) VALUES ($1, $2, $3, 'queued', 0, $4, $5, $6) ON CONFLICT(delivery_id) DO NOTHING",
        );
        let result = sqlx::query(query)
            .bind(id)
            .bind(event)
            .bind(payload)
            .bind(now)
            .bind(now)
            .bind(now)
            .execute(&self.pool)
            .await
            .context("failed to enqueue GitHub webhook")?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn claim_next(&self) -> anyhow::Result<Option<QueuedDelivery>> {
        let now = epoch_seconds();
        let query = self.sql(
            "SELECT delivery_id, event_type, payload, attempts FROM github_deliveries WHERE state = 'queued' AND available_at <= ? ORDER BY received_at LIMIT 1",
            "SELECT delivery_id, event_type, payload, attempts FROM github_deliveries WHERE state = 'queued' AND available_at <= $1 ORDER BY received_at LIMIT 1",
        );
        let row = sqlx::query(query)
            .bind(now)
            .fetch_optional(&self.pool)
            .await
            .context("failed to find queued webhook")?;
        let Some(row) = row else {
            return Ok(None);
        };
        let id: String = sqlx::Row::try_get(&row, "delivery_id")?;
        let event: String = sqlx::Row::try_get(&row, "event_type")?;
        let payload: String = sqlx::Row::try_get(&row, "payload")?;
        let attempts: i64 = sqlx::Row::try_get(&row, "attempts")?;
        let query = self.sql(
            "UPDATE github_deliveries SET state = 'processing', attempts = attempts + 1, updated_at = ? WHERE delivery_id = ? AND state = 'queued'",
            "UPDATE github_deliveries SET state = 'processing', attempts = attempts + 1, updated_at = $1 WHERE delivery_id = $2 AND state = 'queued'",
        );
        let result = sqlx::query(query)
            .bind(now)
            .bind(&id)
            .execute(&self.pool)
            .await
            .context("failed to claim webhook delivery")?;
        if result.rows_affected() == 0 {
            return Ok(None);
        }
        Ok(Some(QueuedDelivery {
            id,
            event,
            payload,
            attempts: i32::try_from(attempts + 1).unwrap_or(i32::MAX),
        }))
    }

    pub async fn finish(&self, id: &str) -> anyhow::Result<()> {
        let query = self.sql(
            "UPDATE github_deliveries SET state = 'done', updated_at = ? WHERE delivery_id = ?",
            "UPDATE github_deliveries SET state = 'done', updated_at = $1 WHERE delivery_id = $2",
        );
        sqlx::query(query)
            .bind(epoch_seconds())
            .bind(id)
            .execute(&self.pool)
            .await
            .context("failed to mark webhook complete")?;
        Ok(())
    }

    pub async fn retry(&self, delivery: &QueuedDelivery) -> anyhow::Result<()> {
        let failed = delivery.attempts >= 5;
        let backoff = i64::from(delivery.attempts.min(8)).pow(2) * 5;
        let query = self.sql(
            "UPDATE github_deliveries SET state = ?, available_at = ?, updated_at = ? WHERE delivery_id = ?",
            "UPDATE github_deliveries SET state = $1, available_at = $2, updated_at = $3 WHERE delivery_id = $4",
        );
        sqlx::query(query)
            .bind(if failed { "failed" } else { "queued" })
            .bind(epoch_seconds() + backoff)
            .bind(epoch_seconds())
            .bind(&delivery.id)
            .execute(&self.pool)
            .await
            .context("failed to schedule webhook retry")?;
        Ok(())
    }

    pub async fn prune_old(&self) -> anyhow::Result<u64> {
        let query = self.sql(
            "DELETE FROM github_deliveries WHERE state IN ('done', 'failed') AND received_at < ?",
            "DELETE FROM github_deliveries WHERE state IN ('done', 'failed') AND received_at < $1",
        );
        let result = sqlx::query(query)
            .bind(epoch_seconds() - 7 * 24 * 60 * 60)
            .execute(&self.pool)
            .await
            .context("failed to prune old webhook deliveries")?;
        Ok(result.rows_affected())
    }

    pub async fn backup_sqlite(&self, destination: &str) -> anyhow::Result<()> {
        if !self.sqlite {
            bail!("online SQLite backups are only available when CURURU_DATABASE_URL uses sqlite:");
        }
        if std::path::Path::new(destination).exists() {
            bail!("backup destination already exists: {destination}");
        }
        sqlx::query("VACUUM INTO ?")
            .bind(destination)
            .execute(&self.pool)
            .await
            .context("SQLite online backup failed")?;
        Ok(())
    }

    const fn sql(&self, sqlite: &'static str, postgres: &'static str) -> &'static str {
        if self.sqlite { sqlite } else { postgres }
    }
}

fn epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn sqlite_queue_deduplicates_and_claims_once() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("cururu.db");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let store = DeliveryStore::connect(&url).await.unwrap();
        assert!(
            store
                .enqueue("delivery-1", "pull_request", "{}")
                .await
                .unwrap()
        );
        assert!(
            !store
                .enqueue("delivery-1", "pull_request", "{}")
                .await
                .unwrap()
        );

        let first = store.claim_next().await.unwrap().unwrap();
        assert_eq!(first.id, "delivery-1");
        assert_eq!(first.attempts, 1);
        assert!(store.claim_next().await.unwrap().is_none());
        store.finish(&first.id).await.unwrap();
        assert!(store.claim_next().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn sqlite_online_backup_creates_a_consistent_database_copy() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("cururu.db");
        let backup_path = dir.path().join("backup.db");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let store = DeliveryStore::connect(&url).await.unwrap();
        store
            .enqueue("delivery-2", "issue_comment", "{}")
            .await
            .unwrap();
        store
            .backup_sqlite(backup_path.to_str().unwrap())
            .await
            .unwrap();

        let backup_url = format!("sqlite://{}?mode=ro", backup_path.display());
        sqlx::any::install_default_drivers();
        let backup = AnyPoolOptions::new()
            .max_connections(1)
            .connect(&backup_url)
            .await
            .unwrap();
        let row = sqlx::query(
            "SELECT delivery_id FROM github_deliveries WHERE delivery_id = 'delivery-2'",
        )
        .fetch_optional(&backup)
        .await
        .unwrap();
        assert!(row.is_some());
    }

    #[tokio::test]
    async fn postgres_delivery_queue_round_trips_when_test_database_is_configured() {
        let Ok(database_url) = std::env::var("CURURU_TEST_POSTGRES_URL") else {
            return;
        };
        let store = DeliveryStore::connect(&database_url).await.unwrap();
        let id = format!(
            "integration-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        assert!(store.enqueue(&id, "pull_request", "{}").await.unwrap());
        assert!(!store.enqueue(&id, "pull_request", "{}").await.unwrap());
        let delivery = store.claim_next().await.unwrap().unwrap();
        assert_eq!(delivery.id, id);
        store.finish(&delivery.id).await.unwrap();
        sqlx::query(store.sql(
            "DELETE FROM github_deliveries WHERE delivery_id = ?",
            "DELETE FROM github_deliveries WHERE delivery_id = $1",
        ))
        .bind(&delivery.id)
        .execute(&store.pool)
        .await
        .unwrap();
    }
}
